use crate::model::{Device, State, identify};
use anyhow::Result;
use std::{collections::HashMap, fs, path::Path, time::Duration};
use zbus::{blocking::{Connection, Proxy, connection::Builder}, zvariant::{OwnedObjectPath, OwnedValue}};

type Properties = HashMap<String, OwnedValue>;
type Objects = HashMap<OwnedObjectPath, HashMap<String, Properties>>;

fn text(props: &Properties, key: &str) -> String {
    props.get(key).and_then(|v| <&str>::try_from(v).ok()).unwrap_or_default().into()
}
fn boolean(props: &Properties, key: &str) -> Option<bool> { props.get(key).and_then(|v| bool::try_from(v).ok()) }
fn byte(props: &Properties, key: &str) -> Option<u8> { props.get(key).and_then(|v| u8::try_from(v).ok()) }
fn uint(props: &Properties, key: &str) -> Option<u32> { props.get(key).and_then(|v| u32::try_from(v).ok()) }

pub fn scan(rows: &mut Vec<Device>, warnings: &mut Vec<String>) {
    sysfs(rows);
    let connection = Builder::system().and_then(|b| b.method_timeout(Duration::from_secs(3)).build());
    match connection {
        Ok(c) => {
            if let Err(e) = bluez(&c, rows) { warnings.push(format!("BlueZ: {e}")); }
            if let Err(e) = upower(&c, rows) { warnings.push(format!("UPower: {e}")); }
        }
        Err(e) => warnings.push(format!("Системный D-Bus недоступен: {e}")),
    }
}

fn bluez(c: &Connection, rows: &mut Vec<Device>) -> Result<()> {
    let manager = Proxy::new(c, "org.bluez", "/", "org.freedesktop.DBus.ObjectManager")?;
    let objects: Objects = manager.call("GetManagedObjects", &())?;
    for interfaces in objects.values() {
        let Some(device) = interfaces.get("org.bluez.Device1") else { continue; };
        if boolean(device, "Connected") != Some(true) { continue; }
        let name = text(device, "Name");
        let alias = text(device, "Alias");
        let percent = interfaces.get("org.bluez.Battery1")
            .and_then(|props| byte(props, "Percentage"))
            .filter(|p| *p <= 100);
        if let Some(id) = identify(&format!("{name} {alias}")) {
            let Some(row) = rows.iter_mut().find(|r| r.id == id) else { continue; };
            if row.state != State::Ready {
                row.state = State::Unavailable;
                row.detail = "Bluetooth подключён, но устройство не сообщает заряд через BlueZ".into();
            }
            if let Some(p) = percent {
                row.reading(Some(p), None, None, id == "logitech", "BlueZ · Bluetooth");
            }
        } else if let Some(percent) = percent
            && let Some(id) = bluetooth_id(&text(device, "Address"))
        {
            let name = if alias.trim().is_empty() { name.trim() } else { alias.trim() };
            let name = if name.is_empty() || bluetooth_id(name).is_some() { "Устройство Bluetooth" } else { name };
            let row = system_device(rows, &id, name);
            row.reading(Some(percent), None, None, false, "BlueZ · Bluetooth");
        }
    }
    Ok(())
}

fn upower(c: &Connection, rows: &mut Vec<Device>) -> Result<()> {
    let root = Proxy::new(c, "org.freedesktop.UPower", "/org/freedesktop/UPower", "org.freedesktop.UPower")?;
    let paths: Vec<OwnedObjectPath> = root.call("EnumerateDevices", &())?;
    for path in paths {
        let proxy = Proxy::new(c, "org.freedesktop.UPower", path.as_str(), "org.freedesktop.DBus.Properties")?;
        let Ok(props) = proxy.call::<_, _, Properties>("GetAll", &("org.freedesktop.UPower.Device",)) else {
            // A device can disappear between enumeration and GetAll.
            continue;
        };
        if boolean(&props, "IsPresent") == Some(false) { continue; }
        let name = format!("{} {}", text(&props, "Vendor"), text(&props, "Model"));
        let name = name.trim();
        let known_id = identify(name);
        let level_code = uint(&props, "BatteryLevel").unwrap_or(0);
        let coarse = match level_code { 3 => Some("Низкий"), 4 => Some("Критический"), 6 => Some("Средний"), 7 => Some("Высокий"), 8 => Some("Полный"), _ => None };
        let percent = (level_code == 1)
            .then(|| props.get("Percentage").and_then(|v| f64::try_from(v).ok()))
            .flatten()
            .filter(|p| (0.0..=100.0).contains(p))
            .map(|p| p.round() as u8);
        if percent.is_none() && coarse.is_none() { continue; }
        let row = if let Some(id) = known_id {
            let Some(row) = rows.iter_mut().find(|r| r.id == id) else { continue; };
            row
        } else {
            if boolean(&props, "PowerSupply") != Some(false)
                || !matches!(uint(&props, "Type"), Some(5 | 6 | 12..=14 | 17..=19 | 21 | 22 | 28))
            {
                continue;
            }
            let serial = text(&props, "Serial");
            let native = text(&props, "NativePath");
            let native_name = native.rsplit('/').next().unwrap_or_default();
            let native_address = native_name.strip_prefix("hid-")
                .and_then(|s| s.strip_suffix("-battery"))
                .or_else(|| native_name.strip_prefix("dev_"))
                .unwrap_or(native_name);
            let id = bluetooth_id(&serial).or_else(|| bluetooth_id(native_address)).unwrap_or_else(|| {
                let identity = if serial.is_empty() { path.as_str().to_owned() } else { format!("{name}:{serial}") };
                detected_id("upower", &identity)
            });
            let name = if name.is_empty() || bluetooth_id(name).is_some() { "Беспроводное устройство" } else { name };
            let row = system_device(rows, &id, name);
            if row.state == State::Ready { continue; }
            row
        };
        let charging = match uint(&props, "State") { Some(1) => Some(true), Some(2 | 4) => Some(false), _ => None };
        row.reading(percent, coarse.map(str::to_owned), charging, coarse.is_some() || known_id == Some("logitech"), "UPower");
    }
    Ok(())
}

fn system_device<'a>(rows: &'a mut Vec<Device>, id: &str, name: &str) -> &'a mut Device {
    if let Some(index) = rows.iter().position(|row| row.id == id) {
        return &mut rows[index];
    }
    rows.push(Device::new(id, name, ""));
    rows.last_mut().expect("just inserted a device")
}

fn bluetooth_id(address: &str) -> Option<String> {
    let mut parts = address.split([':', '_']);
    let mut canonical = String::with_capacity(12);
    for _ in 0..6 {
        let part = parts.next()?;
        if part.len() != 2 || !part.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        canonical.push_str(&part.to_ascii_lowercase());
    }
    parts.next().is_none().then(|| detected_id("bluetooth", &canonical))
}

fn detected_id(source: &str, identity: &str) -> String {
    // FNV-1a keeps the on-disk key stable across upgrades without storing raw addresses.
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in identity.bytes() {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
    }
    format!("auto-{source}-{hash:016x}")
}

fn read(path: &Path, name: &str) -> String { fs::read_to_string(path.join(name)).unwrap_or_default().trim().into() }

fn sysfs(rows: &mut [Device]) {
    let Ok(entries) = fs::read_dir("/sys/class/power_supply") else { return; };
    for entry in entries.flatten() {
        let p = entry.path();
        let name = format!("{} {}", read(&p, "manufacturer"), read(&p, "model_name"));
        let Some(id) = identify(&name) else { continue; };
        let Some(row) = rows.iter_mut().find(|r| r.id == id) else { continue; };
        if read(&p, "present") == "0" { continue; }
        let charging = match read(&p, "status").as_str() { "Charging" => Some(true), "Discharging" | "Full" => Some(false), _ => None };
        let coarse = read(&p, "capacity_level");
        let level = match coarse.as_str() { "Critical" => Some("Критический"), "Low" => Some("Низкий"), "Normal" => Some("Средний"), "High" => Some("Высокий"), "Full" => Some("Полный"), _ => None };
        if let Some(level) = level {
            row.reading(None, Some(level.into()), charging, true, "Linux power_supply");
        } else if let Ok(percent) = read(&p, "capacity").parse::<u8>() {
            row.reading(Some(percent), None, charging, id == "logitech", "Linux power_supply");
        }
    }
}
