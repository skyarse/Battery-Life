use crate::{model::{Device, State, identify}, protocols::{self, Reading}};
use hidapi::{BusType, DeviceInfo, HidApi, HidDevice};
use serde::Serialize;
use std::{collections::HashSet, fs, path::Path};

fn target(info: &DeviceInfo) -> Option<&'static str> {
    let vendor = info.vendor_id();
    let pid = info.product_id();
    if vendor == 0x3554 && [0xf517, 0xf524, 0xf50d, 0xf527, 0xf526, 0xf50f].contains(&pid) && info.usage_page() == 0xff02 && info.usage() == 2 { Some("zaopin") }
    else if vendor == 0x291d && pid == 0x385d && info.usage_page() == 0xff90 && info.usage() == 1 { Some("mchose") }
    else if vendor == 0x046d && pid == 0xc53f && info.usage_page() >= 0xff00 { Some("logitech") }
    else if matches!(info.bus_type(), BusType::Bluetooth) && (
        (vendor == 0x2dc8 && identify(info.product_string().unwrap_or_default()) == Some("8bitdo")) ||
        (vendor == 0x045e && [0x02e0, 0x02fd, 0x0b13].contains(&pid))
    ) { Some("8bitdo") }
    else { None }
}

pub fn scan(rows: &mut [Device], warnings: &mut Vec<String>) {
    let api = match HidApi::new() { Ok(a) => a, Err(e) => { warnings.push(format!("HID: {e}")); return; } };
    let mut attempted = HashSet::new();
    for info in api.device_list() {
        let Some(id) = target(info) else { continue; };
        let Some(row) = rows.iter_mut().find(|r| r.id == id) else { continue; };
        if row.state == State::Ready || !attempted.insert(info.path().to_bytes().to_vec()) { continue; }
        row.state = State::Unavailable;
        let result = info.open_device(&api).map_err(|e| e.to_string()).and_then(|dev| query(&dev, id).map_err(|e| e.to_string()));
        match result {
            Ok(r) => row.reading(r.percent, r.level, r.charging, r.approximate, match id { "zaopin" => "USB HID · Compx", "mchose" => "USB HID · MCHOSE", "logitech" => "USB HID · HID++", _ => "Bluetooth HID · XInput" }),
            Err(e) => {
                if e.to_lowercase().contains("permission denied") || e.contains("os error 13") {
                    row.state = State::PermissionDenied;
                    row.detail = "Установите правила udev по инструкции в README и переподключите устройство".into();
                } else { row.detail = e; }
            }
        }
    }
    // Sysfs can still identify connected receivers when /dev is hidden by a sandbox.
    if let Ok(entries) = fs::read_dir("/sys/class/hidraw") {
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(uevent) = fs::read_to_string(path.join("device/uevent")) else { continue; };
            let id = if uevent.contains("0000291D:0000385D") { Some("mchose") }
                else if uevent.contains("00003554:0000F517") { Some("zaopin") }
                else { identify(&uevent) };
            if let Some(row) = id.and_then(|id| rows.iter_mut().find(|r| r.id == id)) {
                if row.state == State::Missing {
                    row.state = State::Unavailable;
                    row.detail = "Приёмник обнаружен, но HID-интерфейс недоступен. Проверьте права и ограничения среды запуска".into();
                }
            }
        }
    }
}

fn query(dev: &HidDevice, id: &str) -> anyhow::Result<Reading> {
    match id {
        "zaopin" => protocols::zaopin(dev),
        "mchose" => protocols::mchose(dev),
        "logitech" => protocols::logitech(dev),
        "8bitdo" => protocols::xbox(dev),
        _ => anyhow::bail!("Протокол не реализован"),
    }
}

#[derive(Serialize)]
struct Interface {
    path: String,
    vendor_id: String,
    product_id: String,
    product: String,
    interface: i32,
    usage_page: String,
    usage: String,
    bus: String,
    provider: Option<String>,
    node_exists: bool,
}

/// Does not include serial numbers or Bluetooth MAC addresses.
pub fn inventory() -> serde_json::Value {
    let mut interfaces = Vec::new();
    let mut errors = Vec::new();
    match HidApi::new() {
        Ok(api) => for i in api.device_list() {
            let raw_path = i.path().to_string_lossy();
            // hidraw paths are safe; avoid emitting other transports' identifiers.
            let path = if raw_path.starts_with("/dev/hidraw") { raw_path.to_string() } else { "[скрыто]".into() };
            interfaces.push(Interface {
                node_exists: Path::new(raw_path.as_ref()).exists(), path,
                vendor_id: format!("{:04x}", i.vendor_id()), product_id: format!("{:04x}", i.product_id()),
                product: i.product_string().unwrap_or_default().into(), interface: i.interface_number(),
                usage_page: format!("{:04x}", i.usage_page()), usage: format!("{:04x}", i.usage()),
                bus: format!("{:?}", i.bus_type()), provider: target(i).map(String::from),
            });
        },
        Err(e) => errors.push(e.to_string()),
    }
    let sysfs: Vec<_> = fs::read_dir("/sys/class/hidraw").into_iter().flatten().flatten().filter_map(|e| {
        let uevent = fs::read_to_string(e.path().join("device/uevent")).ok()?;
        let public: Vec<_> = uevent.lines().filter(|s| s.starts_with("HID_ID=") || s.starts_with("HID_NAME=")).collect();
        Some(serde_json::json!({"node": e.file_name().to_string_lossy(), "device": public}))
    }).collect();
    serde_json::json!({"hid_interfaces": interfaces, "sysfs_devices": sysfs, "errors": errors})
}
