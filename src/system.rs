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

pub fn scan(rows: &mut [Device], warnings: &mut Vec<String>) {
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

fn bluez(c: &Connection, rows: &mut [Device]) -> Result<()> {
    let manager = Proxy::new(c, "org.bluez", "/", "org.freedesktop.DBus.ObjectManager")?;
    let objects: Objects = manager.call("GetManagedObjects", &())?;
    for interfaces in objects.values() {
        let Some(device) = interfaces.get("org.bluez.Device1") else { continue; };
        let name = format!("{} {}", text(device, "Name"), text(device, "Alias"));
        let Some(id) = identify(&name) else { continue; };
        let Some(row) = rows.iter_mut().find(|r| r.id == id) else { continue; };
        if boolean(device, "Connected") != Some(true) { continue; }
        if row.state != State::Ready {
            row.state = State::Unavailable;
            row.detail = "Bluetooth подключён, но устройство не сообщает заряд через BlueZ".into();
        }
        if let Some(p) = interfaces.get("org.bluez.Battery1").and_then(|p| byte(p, "Percentage")) {
            row.reading(Some(p), None, None, id == "logitech", "BlueZ · Bluetooth");
        }
    }
    Ok(())
}

fn upower(c: &Connection, rows: &mut [Device]) -> Result<()> {
    let root = Proxy::new(c, "org.freedesktop.UPower", "/org/freedesktop/UPower", "org.freedesktop.UPower")?;
    let paths: Vec<OwnedObjectPath> = root.call("EnumerateDevices", &())?;
    for path in paths {
        let proxy = Proxy::new(c, "org.freedesktop.UPower", path.as_str(), "org.freedesktop.DBus.Properties")?;
        let props: Properties = proxy.call("GetAll", &("org.freedesktop.UPower.Device",))?;
        if boolean(&props, "IsPresent") == Some(false) { continue; }
        let name = format!("{} {}", text(&props, "Vendor"), text(&props, "Model"));
        let Some(id) = identify(&name) else { continue; };
        let Some(row) = rows.iter_mut().find(|r| r.id == id) else { continue; };
        let level_code = uint(&props, "BatteryLevel").unwrap_or(0);
        let coarse = match level_code { 3 => Some("Низкий"), 4 => Some("Критический"), 6 => Some("Средний"), 7 => Some("Высокий"), 8 => Some("Полный"), _ => None };
        if let Some(level) = coarse {
            row.reading(None, Some(level.into()), None, true, "UPower");
        } else if level_code == 1 {
            if let Some(p) = props.get("Percentage").and_then(|v| f64::try_from(v).ok()).filter(|p| p.is_finite() && (0.0..=100.0).contains(p)) {
                row.reading(Some(p.round() as u8), None, match uint(&props, "State") { Some(1) => Some(true), Some(2 | 4) => Some(false), _ => None }, id == "logitech", "UPower");
            }
        }
    }
    Ok(())
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
