use crate::model::{Device, State};
use std::collections::{HashMap, HashSet};
use zbus::{blocking::Connection, zvariant::Value};

const RECOVERY_PERCENT: u8 = 25;

#[derive(Default)]
pub struct LowBatteryAlerts {
    notified: HashSet<String>,
}

impl LowBatteryAlerts {
    pub fn update(&mut self, devices: &[Device], session: &Connection) {
        self.check(devices, |device| send(session, device));
    }

    fn check(&mut self, devices: &[Device], mut notify: impl FnMut(&Device) -> zbus::Result<()>) {
        for device in devices {
            if device.state != State::Ready {
                continue;
            }

            // Keep the alert latched through small fluctuations near the threshold.
            let recovered = device.charging == Some(true)
                || device.percent.is_some_and(|percent| percent >= RECOVERY_PERCENT)
                || matches!(device.level.as_deref(), Some("Средний" | "Высокий" | "Полный"));
            if recovered {
                self.notified.remove(&device.id);
            }

            if device.is_low_battery() && !self.notified.contains(&device.id) {
                match notify(device) {
                    Ok(()) => { self.notified.insert(device.id.clone()); }
                    Err(error) => eprintln!("Не удалось отправить уведомление для {}: {error}", device.name),
                }
            }
        }
    }
}

fn send(session: &Connection, device: &Device) -> zbus::Result<()> {
    let hints = HashMap::from([
        ("desktop-entry", Value::from("battery-life")),
        ("urgency", Value::from(1u8)),
    ]);
    let body = format!("{} — {}", device.name, device.label());
    session.call_method(
        Some("org.freedesktop.Notifications"),
        "/org/freedesktop/Notifications",
        Some("org.freedesktop.Notifications"),
        "Notify",
        &(
            "Battery Life", 0u32, "battery-caution-symbolic", "Низкий заряд",
            body, Vec::<&str>::new(), hints, -1i32,
        ),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_battery_alert_survives_reconnects_and_small_fluctuations() {
        let mut alerts = LowBatteryAlerts::default();
        let mut device = Device::new("mouse", "Mouse", "");
        let mut sent = 0;
        let mut notify = |_: &Device| { sent += 1; Ok(()) };
        for (percent, charging, state) in [
            (15, false, State::Stale),
            (15, false, State::Ready),
            (15, false, State::Missing),
            (20, false, State::Ready),
            (15, false, State::Ready),
            (15, true, State::Ready),
            (15, false, State::Ready),
        ] {
            device.reading(Some(percent), None, Some(charging), false, "test");
            device.state = state;
            alerts.check(std::slice::from_ref(&device), &mut notify);
        }
        assert_eq!(sent, 2);
    }
}
