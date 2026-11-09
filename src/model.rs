use chrono::{DateTime, Local};
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

const LOW_BATTERY_PERCENT: u8 = 15;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Missing,
    Unavailable,
    PermissionDenied,
    Ready,
    Stale,
}

#[derive(Clone, Debug, Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub percent: Option<u8>,
    pub level: Option<String>,
    pub charging: Option<bool>,
    pub approximate: bool,
    pub state: State,
    pub source: String,
    pub detail: String,
    pub updated_at: Option<u64>,
}

impl Device {
    pub fn new(id: &str, name: &str, detail: &str) -> Self {
        Self {
            id: id.into(), name: name.into(), percent: None, level: None, charging: None,
            approximate: false, state: State::Missing, source: String::new(),
            detail: detail.into(), updated_at: None,
        }
    }

    pub fn reading(&mut self, percent: Option<u8>, level: Option<String>, charging: Option<bool>, approximate: bool, source: &str) {
        let charging_only = percent.is_none() && level.is_none() && charging == Some(true);
        if percent.is_some_and(|p| p > 100) || (percent.is_none() && level.is_none() && !charging_only) {
            return;
        }
        self.percent = percent;
        self.level = level;
        self.charging = charging;
        self.approximate = approximate;
        self.source = source.into();
        self.state = State::Ready;
        self.updated_at = Some(now());
        self.detail = if charging_only {
            "Во время зарядки приёмник не сообщает текущий процент"
        } else if approximate {
            "Приблизительный уровень, сообщённый устройством"
        } else {
            "Значение, сообщённое устройством"
        }.into();
    }

    pub fn reading_time(&self) -> Option<String> {
        let timestamp = i64::try_from(self.updated_at?).ok()?;
        let datetime = DateTime::from_timestamp(timestamp, 0)?.with_timezone(&Local);
        Some(datetime.format("%d.%m.%Y %H:%M:%S").to_string())
    }

    pub fn is_low_battery(&self) -> bool {
        self.state == State::Ready && self.charging != Some(true)
            && (self.percent.is_some_and(|percent| percent <= LOW_BATTERY_PERCENT)
                || matches!(self.level.as_deref(), Some("Критический" | "Низкий")))
    }

    pub fn label(&self) -> String {
        if self.state == State::Ready && self.charging == Some(true) && self.percent.is_none() && self.level.is_none() {
            return "Заряжается".into();
        }
        let value = match self.state {
            State::Ready | State::Stale => {
                if let Some(p) = self.percent {
                    format!("{}{p}%", if self.approximate { "≈" } else { "" })
                } else { self.level.clone().unwrap_or_else(|| "Нет данных".into()) }
            }
            State::Missing => "Не обнаружено".into(),
            State::Unavailable => "Заряд недоступен".into(),
            State::PermissionDenied => "Нужны права доступа".into(),
        };
        let status = if self.charging == Some(true) {
            " · зарядка"
        } else if self.state == State::Stale {
            " · устарело"
        } else if self.percent.is_some() && self.is_low_battery() {
            " · низкий заряд"
        } else {
            ""
        };
        format!("{value}{status}")
    }
}

pub fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() }

pub fn expected() -> Vec<Device> {
    vec![
        Device::new("zaopin", "ZAOPIN Z2 Max", "Подключите мышь и её USB-приёмник"),
        Device::new("logitech", "Logitech G304", "Подключите мышь и её LIGHTSPEED-приёмник"),
        Device::new("8bitdo", "8BitDo Pro 2", "Подключите геймпад по Bluetooth в режиме X"),
        Device::new("ciy", "CIY Tester 68", "Для 2,4 ГГц протокол заряда не подтверждён; проверяются системные источники"),
        Device::new("mchose", "MCHOSE V9 PRO", "Подключите наушники и их USB-приёмник"),
    ]
}

pub fn identify(name: &str) -> Option<&'static str> {
    let n = name.to_lowercase();
    if n.contains("zaopin") || n.contains("z2 max") { Some("zaopin") }
    else if n.contains("g304") || n.contains("g305") { Some("logitech") }
    else if n.contains("8bitdo") || n.contains("8bit do") { Some("8bitdo") }
    else if n.contains("tester68") || n.contains("tester 68") || n.contains("ciy68") || n.contains("ciy 68") { Some("ciy") }
    else if n.contains("mchose") && n.contains("v9") { Some("mchose") }
    else { None }
}

/// An unsuccessful refresh never presents a previous value as a fresh measurement.
pub fn retain_recent(rows: &mut [Device], previous: &[Device], timestamp: u64) {
    for row in rows {
        if matches!(row.state, State::Ready | State::Missing) { continue; }
        if let Some(old) = previous.iter().find(|p| {
            p.id == row.id && (p.percent.is_some() || p.level.is_some())
                && p.updated_at.is_some_and(|t| timestamp.saturating_sub(t) <= 300)
        }) {
            let reason = row.detail.clone();
            row.percent = old.percent;
            row.level = old.level.clone();
            row.approximate = old.approximate;
            row.updated_at = old.updated_at;
            row.charging = None;
            row.source = old.source.clone();
            row.state = State::Stale;
            row.detail = format!("Последние данные, не текущее измерение. {reason}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn charging_without_a_level_is_visible_but_not_cached_as_a_level() {
        let mut old = expected();
        old[4].reading(None, None, Some(true), false, "USB HID · MCHOSE");
        old[4].updated_at = Some(100);
        assert_eq!(old[4].label(), "Заряжается");
        assert_eq!(old[4].state, State::Ready);

        let mut rows = expected();
        rows[4].state = State::Unavailable;
        retain_recent(&mut rows, &old, 110);
        assert_eq!(rows[4].state, State::Unavailable);
        assert_eq!(rows[4].percent, None);
    }
    #[test]
    fn expired_and_failed_readings_do_not_turn_into_zero() {
        let mut old = expected();
        old[0].reading(Some(73), None, Some(true), false, "test");
        old[0].updated_at = Some(100);
        let mut rows = expected();
        rows[0].state = State::Unavailable;
        retain_recent(&mut rows, &old, 110);
        assert_eq!(rows[0].state, State::Stale);
        assert_eq!(rows[0].percent, Some(73));
        assert_eq!(rows[0].charging, None);
        assert!(rows[0].label().contains("устарело"));
        let mut rows = expected();
        retain_recent(&mut rows, &old, 110);
        assert_eq!(rows[0].percent, None);
        assert_eq!(rows[0].state, State::Missing);
        rows[0].state = State::Unavailable;
        retain_recent(&mut rows, &old, 401);
        assert_eq!(rows[0].percent, None);
        assert_eq!(rows[0].state, State::Unavailable);
    }
    #[test]
    fn unknown_keyboard_is_not_misidentified_as_ciy() {
        assert_eq!(identify("MonsGeek Keyboard"), None);
        assert_eq!(identify("G304 Lightspeed Wireless Gaming Mouse"), Some("logitech"));
    }
}
