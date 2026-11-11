mod hardware;
mod i18n;
mod history;
mod model;
mod notifications;
mod panel;
mod panel_ui;
mod protocols;
mod system;
mod tray_icon;
mod tray_service;
mod ui;

use anyhow::{Context, Result};
use history::DeviceHistory;
use ksni::Tray;
use tray_service::Handle;
use model::{Device, State};
use notifications::LowBatteryAlerts;
use serde::Serialize;
use std::{sync::{Arc, Mutex, mpsc}, time::{Duration, Instant}};
use tray_icon::{Appearance, Orientation, Style};
use zbus::blocking::{Connection, connection::Builder};

const RULES: &str = include_str!("../packaging/70-battery-life.rules");
const NO_DEVICES: &str = "Устройства не обнаружены";

#[derive(Clone, Serialize)]
pub struct Snapshot {
    version: &'static str,
    checked_at: u64,
    devices: Vec<Device>,
    warnings: Vec<String>,
}

impl Snapshot {
    fn visible_devices(&self) -> impl Iterator<Item = &Device> {
        self.devices.iter().filter(|device| device.state != State::Missing)
    }
}

fn scan() -> Snapshot {
    let mut devices = model::expected();
    let known_count = devices.len();
    let mut warnings = Vec::new();
    system::scan(&mut devices, &mut warnings);
    hardware::scan(&mut devices, &mut warnings);
    devices[known_count..].sort_by(|left, right| left.name.cmp(&right.name).then_with(|| left.id.cmp(&right.id)));
    Snapshot { version: env!("CARGO_PKG_VERSION"), checked_at: model::now(), devices, warnings }
}

#[derive(Clone, Copy)]
enum Command {
    Refresh, OpenWindow, SetOrientation(Orientation), SetStyle(Style), SetLanguage(i18n::Language),
    TogglePanel(i32, i32), ContextMenu(i32, i32), PanelUnavailable, Quit,
}

enum WindowEvent<'a> { Update(&'a Snapshot), Show }

struct BatteryTray {
    snapshot: Snapshot,
    tx: mpsc::Sender<Command>,
    appearance: Appearance,
    icon_theme_path: String,
    has_panel: bool,
}

impl Tray for BatteryTray {
    fn activate(&mut self, x: i32, y: i32) {
        let command = if self.has_panel { Command::TogglePanel(x, y) } else { Command::OpenWindow };
        let _ = self.tx.send(command);
    }
    fn id(&self) -> String { "battery-life".into() }
    fn title(&self) -> String { "Battery Life".into() }
    fn icon_theme_path(&self) -> String {
        self.icon_theme_path.clone()
    }
    fn icon_name(&self) -> String {
        let low = self.snapshot.devices.iter().any(Device::is_low_battery);
        if self.icon_theme_path.is_empty() {
            return if low { "battery-caution-symbolic" } else { "battery-good-symbolic" }.into();
        }
        self.appearance.icon_name(low)
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        let description = self.snapshot.visible_devices()
            .map(|device| format!("{}: {}", device.name, device.label()))
            .collect::<Vec<_>>()
            .join("\n");
        ksni::ToolTip {
            title: "Battery Life".into(),
            description: self.appearance.language.translate(if description.is_empty() { NO_DEVICES } else { &description }),
            ..Default::default()
        }
    }
}

fn main() {
    if let Err(e) = run() { eprintln!("Battery Life: {e:#}"); std::process::exit(1); }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Battery Life {}\n\nБез аргументов: значок в системном трее KDE.\n  --window             Также открыть окно подробностей\n  --once               Прочитать заряд и вывести JSON, без GUI\n  --diagnose           Вывести JSON со снимком и HID-инвентаризацией\n  --print-udev-rules   Вывести правила доступа для поддерживаемых устройств\n  --version            Версия\n\nИнтервал обновления: 30 секунд. Отсутствие данных не означает 0%.", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args == ["--version"] { println!("{}", env!("CARGO_PKG_VERSION")); return Ok(()); }
    if args == ["--print-udev-rules"] { print!("{RULES}"); return Ok(()); }
    for a in &args {
        anyhow::ensure!(["--once", "--diagnose", "--window"].contains(&a.as_str()), "Неизвестный аргумент: {a}. Используйте --help");
    }
    if args.iter().any(|a| a == "--once" || a == "--diagnose") {
        let snapshot = scan();
        let output = if args.iter().any(|a| a == "--diagnose") {
            serde_json::json!({"snapshot": snapshot, "hardware": hardware::inventory()})
        } else { serde_json::to_value(snapshot)? };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }
    anyhow::ensure!(std::env::var_os("WAYLAND_DISPLAY").is_some(), "Нужен сеанс Wayland. Для диагностики без GUI используйте --diagnose");
    let instance = Builder::session()
        .and_then(|builder| builder.method_timeout(Duration::from_secs(3)).build())
        .context("Не удалось подключиться к D-Bus сеанса")?;
    instance.request_name("io.github.skyarse.BatteryLife").context("Приложение уже запущено или имя D-Bus недоступно")?;

    let (tx, rx) = mpsc::channel();
    let initial = Snapshot { version: env!("CARGO_PKG_VERSION"), checked_at: model::now(), devices: model::expected(), warnings: vec![] };
    let show_window = args.iter().any(|a| a == "--window");
    let shared = Arc::new(Mutex::new(initial.clone()));
    let has_window = match slint::BackendSelector::new().select() {
        Ok(()) => true,
        Err(error) if show_window => return Err(error.into()),
        Err(error) => {
            eprintln!("Battery Life: окно подробностей недоступно: {error:#}");
            false
        }
    };
    let handle = tray_service::spawn(BatteryTray {
        snapshot: initial, tx: tx.clone(), appearance: Appearance::load(),
        icon_theme_path: tray_icon::theme_path(), has_panel: true,
    }, instance.clone())
        .context("Не удалось создать значок в трее KDE")?;
    if !has_window {
        monitor(handle, rx, instance, |_| {});
        return Ok(());
    }

    if show_window { ui::show(&shared, tx.clone())?; }
    std::thread::spawn(move || {
        monitor(handle, rx, instance, |event| {
            let show = matches!(event, WindowEvent::Show);
            if let WindowEvent::Update(snapshot) = event
                && let Ok(mut current) = shared.lock()
            {
                *current = snapshot.clone();
            }
            let shared_update = shared.clone();
            let tx = tx.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if show {
                    if let Err(error) = ui::show(&shared_update, tx) {
                        eprintln!("Battery Life: не удалось открыть окно: {error}");
                    }
                } else {
                    ui::update_open_window(&shared_update);
                }
            });
        });
        let _ = slint::invoke_from_event_loop(|| { let _ = slint::quit_event_loop(); });
    });
    slint::run_event_loop_until_quit()?;
    Ok(())
}

fn monitor(
    handle: Handle,
    rx: mpsc::Receiver<Command>,
    session: Connection,
    window_event: impl Fn(WindowEvent<'_>),
) {
    let Some((sender, mut appearance)) = handle.update(|tray| (tray.tx.clone(), tray.appearance)) else {
        return;
    };
    let panel = panel::Panel::start(sender);
    let history_path = history::state_path();
    let mut history = history_path.as_deref().map(DeviceHistory::load)
        .transpose().unwrap_or_else(|error| {
            eprintln!("Battery Life: не удалось прочитать историю заряда: {error:#}");
            None
        }).unwrap_or_default();
    let mut history_dirty = false;
    let mut alerts = LowBatteryAlerts::default();
    loop {
        let mut snapshot = scan();
        history_dirty |= history.update(&mut snapshot.devices);
        if let Some(path) = history_path.as_deref().filter(|_| history_dirty) {
            match history.save(path) {
                Ok(()) => history_dirty = false,
                Err(error) => snapshot.warnings.push(format!("Не удалось сохранить историю заряда: {error:#}")),
            }
        }
        alerts.update(&snapshot.devices, &session);
        window_event(WindowEvent::Update(&snapshot));
        panel.send(panel::Message::Update(snapshot.clone(), appearance));
        if handle.update(|tray| tray.snapshot = snapshot.clone()).is_none() {
            return;
        }
        let next_scan = Instant::now() + Duration::from_secs(30);
        loop {
            match rx.recv_timeout(next_scan.saturating_duration_since(Instant::now())) {
                Ok(Command::Refresh) | Err(mpsc::RecvTimeoutError::Timeout) => break,
                Ok(Command::OpenWindow) => window_event(WindowEvent::Show),
                Ok(Command::TogglePanel(x, y)) => {
                    panel.send(panel::Message::Toggle(x, y));
                    break;
                }
                Ok(Command::ContextMenu(x, y)) => {
                    panel.send(panel::Message::ContextMenu(x, y));
                }
                Ok(Command::PanelUnavailable) => {
                    let _ = handle.update(|tray| tray.has_panel = false);
                }
                Ok(command @ (Command::SetOrientation(_) | Command::SetStyle(_) | Command::SetLanguage(_))) => {
                    match command {
                        Command::SetOrientation(value) => appearance.orientation = value,
                        Command::SetStyle(value) => appearance.style = value,
                        Command::SetLanguage(value) => appearance.language = value,
                        _ => unreachable!(),
                    }
                    let _ = handle.update(|tray| tray.appearance = appearance);
                    panel.send(panel::Message::Update(snapshot.clone(), appearance));
                    if let Err(error) = appearance.save() {
                        eprintln!("Battery Life: не удалось сохранить настройки значка: {error:#}");
                    }
                    window_event(WindowEvent::Update(&snapshot));
                }
                Ok(Command::Quit) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}
