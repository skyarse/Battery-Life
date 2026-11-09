use crate::model::{Device, State};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, btree_map::Entry},
    env, fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

#[derive(Default, Deserialize, Serialize)]
#[serde(transparent)]
pub struct DeviceHistory(BTreeMap<String, Option<SavedReading>>);

#[derive(Deserialize, PartialEq, Serialize)]
struct SavedReading {
    percent: Option<u8>,
    level: Option<String>,
    approximate: bool,
    source: String,
    updated_at: u64,
}

impl SavedReading {
    fn from_device(device: &Device) -> Option<Self> {
        if device.percent.is_none() && device.level.is_none() {
            return None;
        }
        Some(Self {
            percent: device.percent,
            level: device.level.clone(),
            approximate: device.approximate,
            source: device.source.clone(),
            updated_at: device.updated_at?,
        })
    }

    fn apply_to(&self, device: &mut Device) {
        device.percent = self.percent;
        device.level.clone_from(&self.level);
        device.approximate = self.approximate;
        device.source.clone_from(&self.source);
        device.updated_at = Some(self.updated_at);
        device.charging = None;
    }
}

pub fn state_path() -> Option<PathBuf> {
    let base = env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|path| path.join(".local/state"))
        })?;
    Some(base.join("battery-life/devices.json"))
}

impl DeviceHistory {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        match fs::File::open(path) {
            Ok(file) => Ok(serde_json::from_reader(file)?),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let dir = path.parent().expect("the state file has a parent directory");
        fs::create_dir_all(dir)?;
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        let file = fs::File::create_new(&temporary)?;
        let result = serde_json::to_writer(file, self)
            .map_err(anyhow::Error::from)
            .and_then(|()| Ok(fs::rename(&temporary, path)?));
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    pub fn update(&mut self, devices: &mut [Device]) -> bool {
        let mut changed = false;
        for device in devices {
            let saved = match self.0.entry(device.id.clone()) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(_) if device.state == State::Missing => continue,
                Entry::Vacant(entry) => {
                    changed = true;
                    entry.insert(None)
                }
            };
            match device.state {
                State::Ready => {
                    if let Some(reading) = SavedReading::from_device(device)
                        && saved.as_ref() != Some(&reading)
                    {
                        *saved = Some(reading);
                        changed = true;
                    }
                }
                State::Missing | State::Offline => {
                    if device.state == State::Missing {
                        device.state = State::Offline;
                        device.detail = "Устройство отключено".into();
                    }
                    if let Some(reading) = saved {
                        reading.apply_to(device);
                        device.detail.push_str(". Показан последний известный заряд");
                    }
                }
                State::Unavailable => {
                    if let Some(reading) = saved {
                        reading.apply_to(device);
                        device.state = State::Stale;
                        device.detail = format!("Показан последний известный заряд. {}", device.detail);
                    }
                }
                State::PermissionDenied | State::Stale => {}
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::expected;

    #[test]
    fn disconnect_after_restart_restores_reading_and_hides_unseen_devices() -> anyhow::Result<()> {
        let dir = env::temp_dir().join(format!(
            "battery-life-history-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos()
        ));
        fs::create_dir(&dir)?;
        let path = dir.join("battery-life/devices.json");
        let mut history = DeviceHistory::load(&path)?;
        let mut devices = expected();
        assert!(!history.update(&mut devices));
        devices[0].reading(Some(12), None, Some(true), true, "USB HID");
        devices[0].updated_at = Some(100);
        assert!(history.update(&mut devices));
        history.save(&path)?;

        let mut history = DeviceHistory::load(&path)?;
        fs::remove_dir_all(dir)?;
        let mut devices = expected();
        assert!(!history.update(&mut devices));
        assert_eq!(devices[0].state, State::Offline);
        assert_eq!(devices[0].label(), "≈12% · отключено");
        assert_eq!(devices[0].updated_at, Some(100));
        assert_eq!(devices[0].charging, None);
        assert!(!devices[0].is_low_battery());
        assert!(devices[1..].iter().all(|device| device.state == State::Missing));
        Ok(())
    }

    #[test]
    fn charging_without_a_level_keeps_the_previous_reading_until_reconnected() {
        let mut history = DeviceHistory::default();
        let mut devices = expected();
        devices[4].reading(Some(60), None, Some(false), true, "USB HID");
        assert!(history.update(&mut devices));

        let mut devices = expected();
        devices[4].reading(None, None, Some(true), false, "USB HID");
        assert!(!history.update(&mut devices));
        assert_eq!(devices[4].label(), "Заряжается");

        let mut devices = expected();
        devices[4].state = State::Unavailable;
        assert!(!history.update(&mut devices));
        assert_eq!(devices[4].label(), "≈60% · устарело");

        let mut devices = expected();
        devices[4].state = State::Offline;
        assert!(!history.update(&mut devices));
        assert_eq!(devices[4].label(), "≈60% · отключено");

        devices[4].reading(Some(80), None, Some(false), true, "USB HID");
        assert!(history.update(&mut devices));
        assert_eq!(devices[4].label(), "≈80%");
    }

    #[test]
    fn detected_device_without_a_reading_can_go_offline() {
        let mut history = DeviceHistory::default();
        let mut devices = expected();
        devices[3].state = State::Unavailable;
        assert!(history.update(&mut devices));
        assert_eq!(devices[3].label(), "Заряд недоступен");

        let mut devices = expected();
        assert!(!history.update(&mut devices));
        assert_eq!(devices[3].label(), "Отключено");
        assert_eq!(devices[3].updated_at, None);
    }
}
