use crate::model::{Device, State};
use std::{collections::HashMap, time::{Duration, Instant}};
use zbus::{blocking::Connection, zvariant::Value};

const RECOVERY_PERCENT: u8 = 25;
const REMINDER_INTERVAL: Duration = Duration::from_secs(60 * 60);

#[derive(Default)]
pub struct LowBatteryAlerts {
    last_notified: HashMap<String, Instant>,
}

impl LowBatteryAlerts {
    pub fn update(&mut self, devices: &[Device], session: &Connection) {
        self.check(devices, Instant::now(), |device| send(session, device));
    }

    fn check(
        &mut self,
        devices: &[Device],
        now: Instant,
        mut notify: impl FnMut(&Device) -> zbus::Result<()>,
    ) {
        for device in devices {
            if device.state != State::Ready {
                continue;
            }

            // Keep the reminder timer through small fluctuations near the threshold.
            let recovered = device.charging == Some(true)
                || device.percent.is_some_and(|percent| percent >= RECOVERY_PERCENT)
                || matches!(device.level.as_deref(), Some("Средний" | "Высокий" | "Полный"));
            if recovered {
                self.last_notified.remove(&device.id);
            }

            let due = self.last_notified.get(&device.id)
                .is_none_or(|last| now.saturating_duration_since(*last) >= REMINDER_INTERVAL);
            if device.is_low_battery() && due {
                match notify(device) {
                    Ok(()) => { self.last_notified.insert(device.id.clone(), now); }
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
    let language = crate::tray_icon::Appearance::load().language;
    let body = language.translate(&format!("{} — {}", device.name, device.label()));
    session.call_method(
        Some("org.freedesktop.Notifications"),
        "/org/freedesktop/Notifications",
        Some("org.freedesktop.Notifications"),
        "Notify",
        &(
            "Battery Life", 0u32, "battery-caution-symbolic", language.translate("Низкий заряд"),
            body, Vec::<&str>::new(), hints, -1i32,
        ),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reminders_repeat_hourly_with_fresh_readings_and_reset_after_charging() {
        let mut alerts = LowBatteryAlerts::default();
        let mut device = Device::new("mouse", "Mouse", "");
        let start = Instant::now();
        let mut sent = 0;
        for (elapsed, percent, charging, state, expected) in [
            (0, 15, false, State::Stale, 0),
            (1, 15, false, State::Ready, 1),
            (2, 15, false, State::Missing, 1),
            (3, 20, false, State::Ready, 1),
            (3600, 15, false, State::Ready, 1),
            (3601, 15, false, State::Stale, 1),
            (3602, 15, false, State::Ready, 2),
            (3603, 15, false, State::Ready, 2),
            (3604, 15, true, State::Ready, 2),
            (3605, 15, false, State::Ready, 3),
        ] {
            device.reading(Some(percent), None, Some(charging), false, "test");
            device.state = state;
            alerts.check(std::slice::from_ref(&device), start + Duration::from_secs(elapsed), |_| {
                sent += 1;
                Ok(())
            });
            assert_eq!(sent, expected, "at {elapsed} seconds");
        }
    }
}
