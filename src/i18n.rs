use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    #[default]
    Russian,
    English,
}

impl Language {
    pub fn translate(self, text: &str) -> String {
        if self == Self::Russian { return text.into(); }
        let mut result = text.to_owned();
        for (russian, english) in TRANSLATIONS {
            result = result.replace(russian, english);
        }
        result
    }
}

// Longer phrases precede the individual status words they may contain.
const TRANSLATIONS: &[(&str, &str)] = &[
    ("Если нужны права доступа, установите правила udev по инструкции в README. После этого переподключите приёмник. Запускать приложение через sudo не нужно.", "If access is denied, install the udev rules from the README and reconnect the receiver. Run Battery Life as your regular user."),
    ("Во время зарядки приёмник не сообщает текущий процент", "The receiver does not report a percentage while charging"),
    ("Приблизительный уровень, сообщённый устройством", "Approximate level reported by the device"),
    ("Значение, сообщённое устройством", "Reading reported by the device"),
    ("Приёмник сообщает, что наушники выключены", "The receiver reports that the headset is off"),
    ("Мышь спит или отключена от приёмника", "The mouse is asleep or disconnected from the receiver"),
    ("Нет ответа: устройство выключено, спит или протокол не поддерживается", "No response: the device is off, asleep, or its protocol is unsupported"),
    ("Установите правила udev по инструкции в README и переподключите устройство", "Install the udev rules from the README and reconnect the device"),
    ("Приёмник обнаружен, но HID-интерфейс недоступен. Проверьте права и ограничения среды запуска", "Receiver detected, but its HID interface is inaccessible. Check permissions and sandbox restrictions"),
    ("Bluetooth подключён, но устройство не сообщает заряд через BlueZ", "Bluetooth is connected, but BlueZ has no battery reading"),
    ("Показан последний известный заряд", "Showing the last known charge"),
    ("Устройство отключено", "Device disconnected"),
    ("Для 2,4 ГГц протокол заряда не подтверждён; проверяются системные источники", "The 2.4 GHz battery protocol is unconfirmed; checking system sources"),
    ("Подключите мышь и её USB-приёмник", "Connect the mouse and its USB receiver"),
    ("Подключите мышь и её LIGHTSPEED-приёмник", "Connect the mouse and its LIGHTSPEED receiver"),
    ("Подключите геймпад по Bluetooth в режиме X", "Connect the controller over Bluetooth in X mode"),
    ("Подключите наушники и их USB-приёмник", "Connect the headset and its USB receiver"),
    ("Приёмник не подтвердил модель", "Receiver did not confirm the model"),
    ("Ответ MCHOSE не содержит статуса батареи", "MCHOSE reply has no battery status"),
    ("Неизвестный ответ MCHOSE о батарее", "Unknown MCHOSE battery reply"),
    ("Мышь не сообщает", "Mouse does not report"),
    ("Неполная отправка запроса", "Incomplete query write"),
    ("Протокол не реализован", "Protocol not implemented"),
    ("запрос идентификации", "identification query"), ("ответ идентификации", "identification reply"),
    ("запрос связи", "connection query"), ("ответ о связи", "connection reply"),
    ("запрос заряда", "battery query"), ("ответ о заряде", "battery reply"),
    ("Системный D-Bus недоступен", "System D-Bus unavailable"),
    ("Не удалось сохранить историю заряда", "Could not save battery history"),
    ("Последние данные, не текущее измерение", "Last known reading, not a current measurement"),
    ("Автообновление каждые 30 секунд. Недоступный заряд не означает 0%.", "Checked every 30 seconds. Unavailable charge does not mean 0%."),
    ("Автообновление: 30 с.", "Automatic check: 30 s."),
    ("Устройства не обнаружены", "No devices detected"),
    ("Нужны права доступа", "Access required"), ("Заряд недоступен", "Charge unavailable"),
    ("Не обнаружено", "Not detected"), ("Нет данных", "No data"),
    ("Низкий заряд", "Low battery"), ("низкий заряд", "low battery"),
    ("Заряжается", "Charging"), ("зарядка", "charging"),
    ("Отключено", "Disconnected"), ("отключено", "disconnected"), ("устарело", "stale"),
    ("Критический", "Critical"), ("Низкий", "Low"), ("Средний", "Medium"), ("Высокий", "High"), ("Полный", "Full"),
    ("Беспроводное устройство", "Wireless device"), ("Устройство Bluetooth", "Bluetooth device"),
    ("Ориентация значка", "Icon orientation"), ("Стиль значка", "Icon style"),
    ("Вверх ногами", "Upside down"), ("Набок влево", "Rotate left"), ("Набок вправо", "Rotate right"),
    ("Обычная", "Default"), ("Обычный", "Regular"), ("Монохром", "Monochrome"),
    ("Открыть окно", "Open window"), ("Настройки", "Settings"), ("Выход", "Quit"),
    ("Последний ответ", "Last response"), ("Источник", "Source"),
    ("Устройство", "Device"), ("Заряд", "Charge"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_preferences_without_language_remain_russian() {
        let settings: crate::tray_icon::Appearance = serde_json::from_str(r#"{"orientation":"left","style":"color"}"#).unwrap();
        assert!(settings.language == Language::Russian);
        let settings = crate::tray_icon::Appearance { language: Language::English, ..settings };
        let saved = serde_json::to_string(&settings).unwrap();
        let restored: crate::tray_icon::Appearance = serde_json::from_str(&saved).unwrap();
        assert!(restored.language == Language::English);
        assert!(restored.orientation == crate::tray_icon::Orientation::Left);
    }

    #[test]
    fn translation_does_not_change_the_charge_value() {
        assert_eq!(Language::English.translate("отключено · ≈45%"), "disconnected · ≈45%");
        assert_eq!(Language::English.translate("Приблизительный уровень, сообщённый устройством"), "Approximate level reported by the device");
    }
}
