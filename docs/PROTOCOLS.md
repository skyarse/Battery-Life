# Battery data sources

Battery Life first checks Linux power_supply, BlueZ, and UPower. For devices whose battery is not reported by the system, it uses the queries below. USB receiver communication is limited to status and battery queries. VID:PID and HID usage values restrict which interfaces are queried.

| Device | Implementation and reference |
| --- | --- |
| MCHOSE V9 PRO | Receiver `291d:385d`, collection `ff90:0001`. Sends `55 65 01` in a 64-byte report. The third reply byte contains the level in 10% steps, and the fourth contains the power state. `02` means battery power, `00` and `03` indicate a connected cable, and `1a` means the headset is off. The reported level can freeze while a cable is connected. [Captured receiver responses](https://github.com/PyFlat/Device-Battery-Info/blob/main/tests/DeviceBatteryInfo.Tests/MchoseProtocolTests.cs), [interface definition](https://github.com/Tensyd/mchose-battery/blob/main/src/device/spec.rs). |
| ZAOPIN Z2 Max | Compx receivers, collection `ff02:0002`. Command `01` reads model information, `03` checks the connection, and `04` requests the battery level. Receiver `3554:f517` may report model codes other than `CID 7, MID 5`. Connection status and reply format are still validated. [hrtz protocol documentation](https://github.com/azaek/hrtz/blob/main/docs/PROTOCOL.md). |
| Logitech G304 | LIGHTSPEED `046d:c53f`. Discovers HID++ feature `1000` and requests the level of the device in the first slot. Battery levels are reported in steps. [Solaar G304 device report](https://github.com/pwr-Solaar/Solaar/blob/master/docs/devices/G304%20Lightspeed%20Wireless%20Gaming%20Mouse%204074.txt). |
| 8BitDo Pro 2 | Uses system battery data when available over Bluetooth in X mode. The fallback passively reads compatible Xbox HID battery reports with report ID `04` when the controller sends them. Four battery levels are shown as text. [SDL report handling](https://github.com/libsdl-org/SDL/blob/main/src/joystick/hidapi/SDL_hidapi_xboxone.c). |
| CIY Tester 68 | Checks system battery readings for known model names. A 2.4 GHz battery query has not been confirmed. Unknown receivers are not sent HID queries. |

System battery interfaces are described in the [BlueZ Battery1 documentation](https://github.com/bluez/bluez/blob/master/doc/org.bluez.Battery.rst) and the [UPower Device documentation](https://upower.freedesktop.org/docs/Device.html).
