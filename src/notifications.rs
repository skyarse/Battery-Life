use crate::model::{Device, State};
use std::{collections::HashMap, path::{Path, PathBuf}, time::{Duration, Instant}};
use zbus::{blocking::Connection, zvariant::Value};

const NOTIFICATION_PERCENT: u8 = 25;
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

            let due = self.last_notified.get(&device.id)
                .is_none_or(|last| now.saturating_duration_since(*last) >= REMINDER_INTERVAL);
            let low = device.charging != Some(true)
                && (device.percent.is_some_and(|percent| percent <= NOTIFICATION_PERCENT)
                    || device.is_low_battery());
            if low && due {
                match notify(device) {
                    Ok(()) => { self.last_notified.insert(device.id.clone(), now); }
                    Err(error) => eprintln!("Не удалось отправить уведомление для {}: {error}", device.name),
                }
            }
        }
    }
}

fn send(session: &Connection, device: &Device) -> zbus::Result<()> {
    let language = crate::tray_icon::Appearance::load().language;
    send_notification(session, language, "Низкий заряд", &format!("{} — {}", device.name, device.label()))
}

pub fn send_test(session: &Connection, language: crate::i18n::Language) -> zbus::Result<()> {
    send_notification(session, language, "Тестовое уведомление", "Проверка уведомления и звука.")
}

fn send_notification(session: &Connection, language: crate::i18n::Language, summary: &str, body: &str) -> zbus::Result<()> {
    let sound = sound_path();
    let mut hints = HashMap::from([
        ("desktop-entry", Value::from("battery-life")),
        ("urgency", Value::from(1u8)),
    ]);
    if let Some(path) = sound.as_deref().and_then(Path::to_str) {
        hints.insert("sound-file", Value::from(path));
    } else {
        eprintln!("Battery Life: файл звука уведомления не найден");
    }
    session.call_method(
        Some("org.freedesktop.Notifications"),
        "/org/freedesktop/Notifications",
        Some("org.freedesktop.Notifications"),
        "Notify",
        &(
            "Battery Life", 0u32, "battery-caution-symbolic", language.translate(summary),
            language.translate(body), Vec::<&str>::new(), hints, -1i32,
        ),
    )?;
    Ok(())
}

fn sound_path() -> Option<PathBuf> {
    let installed = std::env::current_exe().ok()
        .and_then(|exe| exe.parent()?.join("../share/battery-life/sounds/low-battery.wav").canonicalize().ok());
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("packaging/sounds/low-battery.wav");
    installed.or(Some(source)).filter(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_notification_sound_is_a_short_pcm_wave() {
        let bytes = std::fs::read(sound_path().expect("notification sound must be bundled")).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes(bytes[20..22].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(bytes[22..24].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(bytes[34..36].try_into().unwrap()), 16);
        let sample_rate = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
        let duration = (bytes.len() - 44) as f64 / (sample_rate as f64 * 2.0);
        assert!((0.1..1.0).contains(&duration));
        assert!(bytes[44..].iter().any(|byte| *byte != 0));
    }

    #[test]
    fn alerts_start_at_twenty_five_and_keep_the_interval_across_threshold_fluctuations() {
        let mut alerts = LowBatteryAlerts::default();
        let mut device = Device::new("mouse", "Mouse", "");
        let start = Instant::now();
        let mut sent = 0;
        for (elapsed, percent, charging, state, expected) in [
            (0, 26, false, State::Ready, 0),
            (1, 25, false, State::Stale, 0),
            (2, 25, true, State::Ready, 0),
            (3, 25, false, State::Ready, 1),
            (60, 26, false, State::Ready, 1),
            (120, 25, false, State::Ready, 1),
            (150, 30, false, State::Ready, 1),
            (180, 24, false, State::Ready, 1),
            (3602, 25, false, State::Ready, 1),
            (3603, 25, false, State::Ready, 2),
            (3604, 26, false, State::Ready, 2),
            (3605, 25, false, State::Ready, 2),
        ] {
            device.reading(Some(percent), None, Some(charging), false, "test");
            device.state = state;
            if percent == 25 {
                assert!(!device.is_low_battery());
            }
            alerts.check(std::slice::from_ref(&device), start + Duration::from_secs(elapsed), |_| {
                sent += 1;
                Ok(())
            });
            assert_eq!(sent, expected, "at {elapsed} seconds");
        }
    }

    #[test]
    fn charging_and_unavailable_readings_keep_the_hourly_interval() {
        let mut alerts = LowBatteryAlerts::default();
        let mut device = Device::new("mouse", "Mouse", "");
        let start = Instant::now();
        let mut sent = 0;
        for (elapsed, percent, charging, state, expected) in [
            (0, 15, false, State::Stale, 0),
            (1, 15, false, State::Ready, 1),
            (2, 15, false, State::Missing, 1),
            (60, 15, true, State::Ready, 1),
            (120, 20, false, State::Ready, 1),
            (3600, 15, false, State::Ready, 1),
            (3601, 15, false, State::Stale, 1),
            (3602, 15, false, State::Ready, 2),
            (3603, 15, false, State::Ready, 2),
            (3604, 15, true, State::Ready, 2),
            (3605, 15, false, State::Ready, 2),
            (7202, 15, true, State::Ready, 2),
            (7203, 15, false, State::Ready, 3),
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
