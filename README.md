# Battery Life

Wireless device battery levels in the KDE Plasma system tray on Wayland. Left-click the battery icon to open a compact popup panel and check for updated readings. Device names appear on the left, with charge levels or connection status on the right. Click a device row to expand its data source and last-reading time inside the panel.

The **Настройки** (Settings) page contains **Открыть окно** (Open window), **Ориентация значка** (Icon orientation), and **Стиль значка** (Icon style). The Settings button is next to the panel title. A Language selector offers Russian and English. Orientation, style, and language preferences are saved between sessions. Use `--window` to open the details window at startup.

The panel closes when you click outside it, press Escape, or use its close button. A right-click on the tray icon opens Plasma's context menu as a fallback.

Devices are checked automatically every 30 seconds. Battery Life supports dedicated protocols for the receivers listed below and discovers other wireless peripherals that expose battery information through BlueZ or UPower.

Only devices detected at least once appear in the list. Disconnected devices retain their last known charge, marked as disconnected. If a receiver is present but the device does not respond, the saved reading is marked as stale. History persists in `$XDG_STATE_HOME/battery-life/devices.json`, which defaults to `~/.local/state/battery-life/devices.json`.

At 15% or below, or when a device reports a low or critical level, Battery Life displays a tray warning and sends a desktop notification. Reminders repeat approximately once per hour while fresh readings remain low. Charging or recovery to 25% resets the reminder timer.

## Running on Fedora KDE

1. Download **Battery-Life-x86_64.AppImage** from the [latest release](https://github.com/skyarse/Battery-Life/releases/latest).
2. Run the commands below in the directory containing the AppImage. Expand Plasma's system tray if the icon is hidden.

```sh
chmod +x Battery-Life-x86_64.AppImage
./Battery-Life-x86_64.AppImage
```

If FUSE is unavailable, use `./Battery-Life-x86_64.AppImage --appimage-extract-and-run`.

### Receiver permissions

If Battery Life reports that access permissions are required, install the udev rules once for the active local user. Then reconnect the USB receiver or the Bluetooth controller.

```sh
./Battery-Life-x86_64.AppImage --print-udev-rules > 70-battery-life.rules
sudo install -m 0644 70-battery-life.rules /etc/udev/rules.d/70-battery-life.rules
sudo udevadm control --reload-rules
```

Run Battery Life as your regular user. The `--diagnose` option prints device status and HID interfaces without serial numbers or Bluetooth MAC addresses.

### Updates with Gear Lever

AppImages from v0.1.13 onward include GitHub Releases update information. Add the AppImage to Gear Lever to check for newer versions. If you use an older build, replace it with a current AppImage once to enable this.

## Devices

| Device | Battery source |
| --- | --- |
| ZAOPIN Z2 Max | Compx 2.4 GHz receiver. Protocol support is based on published documentation and receiver diagnostics. |
| Logitech G304 | LIGHTSPEED HID++. The reported percentage is approximate. |
| 8BitDo Pro 2 | Bluetooth in X mode. Uses system battery information or levels from compatible Xbox HID reports. |
| CIY Tester 68 | System battery information only. Battery queries through its 2.4 GHz receiver have not been confirmed. |
| MCHOSE V9 PRO | The 2.4 GHz receiver reports approximate levels in 10% steps. When a cable is connected, Battery Life displays the charging state. |

Implementation details and references are available in the [protocol notes](docs/PROTOCOLS.md).

## Building

Local builds on Fedora require Rust, a linker, `pkgconf-pkg-config`, and `fontconfig-devel`. The Slint window uses Wayland. Plasma renders the tray menu through StatusNotifierItem and `ksni`.

```sh
cargo test --locked
cargo run --locked -- --diagnose
cargo run --locked
```

GitHub Actions builds and smoke-tests the x86_64 AppImage. A successful build on `main` publishes the package version from `Cargo.toml` with a checksum and `.zsync` update metadata.

For a new release, increase the package version, update `Cargo.lock`, and add an English `docs/releases/v<version>.md` file. Group release notes under **Added**, **Changed**, and **Fixed**, using the sections that apply. The release workflow publishes these notes and synchronizes the descriptions of existing releases.
