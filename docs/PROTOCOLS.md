# Источники показаний

Battery Life сначала проверяет Linux power_supply, BlueZ и UPower. Для устройств, у которых система не сообщает заряд, приложение использует описанные ниже запросы. Обмен с USB-приёмниками ограничен запросами статуса и батареи. Номера `VID:PID` и HID usage ограничивают выбор интерфейса.

| Устройство | Реализация и источник |
| --- | --- |
| MCHOSE V9 PRO | Приёмник `291d:385d`, коллекция `ff90:0001`. Запрос `55 65 01` в 64-байтном отчёте. Уровень в третьем байте ответа `55 65`. Режим зарядки через этот протокол не подтверждён. [Linux-реализация](https://github.com/JoaoKSS/MCHOSE_v9_PRO_Controller/blob/main/mchose_qt.py), [описание интерфейса](https://github.com/Tensyd/mchose-battery/blob/main/src/device/spec.rs). |
| ZAOPIN Z2 Max | Приёмники Compx, коллекция `ff02:0002`. Команда `01` читает модель, `03` проверяет связь, `04` запрашивает батарею. Для `3554:f517` допустимы отличающиеся от `CID 7, MID 5` коды модели. На этом приёмнике проверка связи и формата ответа обязательна. [Описание hrtz](https://github.com/azaek/hrtz/blob/main/docs/PROTOCOL.md). |
| Logitech G304 | LIGHTSPEED `046d:c53f`, поиск HID++ feature `1000` и запрос уровня у устройства в первом слоте. Мышь сообщает заряд ступенями. [Отчёт Solaar по G304](https://github.com/pwr-Solaar/Solaar/blob/master/docs/devices/G304%20Lightspeed%20Wireless%20Gaming%20Mouse%204074.txt). |
| 8BitDo Pro 2 | В режиме X через Bluetooth используется системный заряд, когда он доступен. Запасной путь пассивно читает совместимые Xbox HID-отчёты `04`, если контроллер их присылает. Четыре уровня отображаются текстом. [Обработка отчёта в SDL](https://github.com/libsdl-org/SDL/blob/main/src/joystick/hidapi/SDL_hidapi_xboxone.c). |
| CIY Tester 68 | Проверяются системные показания для известных названий модели. Подтверждённого запроса заряда по 2,4 ГГц пока нет. HID-запросы неизвестным приёмникам не отправляются. |

Системные интерфейсы описаны в [документации BlueZ Battery1](https://github.com/bluez/bluez/blob/master/doc/org.bluez.Battery.rst) и [документации UPower Device](https://upower.freedesktop.org/docs/Device.html).
