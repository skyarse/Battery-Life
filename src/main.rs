mod hardware;
mod model;
mod protocols;
mod system;
mod ui;

use anyhow::{Context, Result};
use ksni::{MenuItem, Tray, menu::StandardItem, blocking::TrayMethods};
use model::{Device, State};
use serde::Serialize;
use std::{sync::{Arc, Mutex, mpsc}, time::Duration};

const RULES: &str = include_str!("../packaging/70-battery-life.rules");

#[derive(Clone, Serialize)]
pub struct Snapshot {
    version: &'static str,
    checked_at: u64,
    devices: Vec<Device>,
    warnings: Vec<String>,
}

fn scan() -> Snapshot {
    let mut devices = model::expected();
    let mut warnings = Vec::new();
    system::scan(&mut devices, &mut warnings);
    hardware::scan(&mut devices, &mut warnings);
    Snapshot { version: env!("CARGO_PKG_VERSION"), checked_at: model::now(), devices, warnings }
}

#[derive(Clone, Copy)]
enum Command { Refresh, Details, Quit }

struct BatteryTray {
    snapshot: Snapshot,
    tx: mpsc::Sender<Command>,
}

impl Tray for BatteryTray {
    const MENU_ON_ACTIVATE: bool = true;
    fn id(&self) -> String { "battery-life".into() }
    fn title(&self) -> String { "Battery Life".into() }
    fn icon_name(&self) -> String {
        if self.snapshot.devices.iter().any(|d| d.state == State::Ready && (d.percent.is_some_and(|p| p <= 15) || d.level.as_deref() == Some("Критический") || d.level.as_deref() == Some("Низкий"))) {
            "battery-caution-symbolic"
        } else { "battery-good-symbolic" }.into()
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Battery Life".into(),
            description: self.snapshot.devices.iter().map(|d| format!("{}: {}", d.name, d.label())).collect::<Vec<_>>().join("\n"),
            ..Default::default()
        }
    }
    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut menu = vec![StandardItem { label: "Беспроводные устройства".into(), enabled: false, ..Default::default() }.into()];
        menu.push(MenuItem::Separator);
        for device in &self.snapshot.devices {
            let tx = self.tx.clone();
            menu.push(StandardItem {
                label: format!("{} — {}", device.name, device.label()),
                activate: Box::new(move |_| { let _ = tx.send(Command::Details); }),
                ..Default::default()
            }.into());
        }
        menu.push(MenuItem::Separator);
        for (label, command) in [("Обновить", Command::Refresh), ("Подробности и доступ к устройствам…", Command::Details), ("Выход", Command::Quit)] {
            let tx = self.tx.clone();
            menu.push(StandardItem { label: label.into(), activate: Box::new(move |_| { let _ = tx.send(command); }), ..Default::default() }.into());
        }
        menu
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
    let instance = zbus::blocking::Connection::session().context("Не удалось подключиться к D-Bus сеанса")?;
    instance.request_name("io.github.skyarse.BatteryLife").context("Приложение уже запущено или имя D-Bus недоступно")?;

    let (tx, rx) = mpsc::channel();
    let initial = Snapshot { version: env!("CARGO_PKG_VERSION"), checked_at: model::now(), devices: model::expected(), warnings: vec![] };
    let shared = Arc::new(Mutex::new(initial.clone()));
    let window = ui::create(shared.clone(), tx.clone())?;
    if args.iter().any(|a| a == "--window") { window.show()?; }
    let handle = BatteryTray { snapshot: initial, tx }.spawn().context("Не удалось создать значок в трее KDE")?;
    let weak = window.as_weak();
    std::thread::spawn(move || {
        let mut previous = Vec::new();
        loop {
            let mut snapshot = scan();
            model::retain_recent(&mut snapshot.devices, &previous, snapshot.checked_at);
            previous = snapshot.devices.clone();
            if let Ok(mut current) = shared.lock() { *current = snapshot.clone(); }
            let _ = handle.update(|tray| tray.snapshot = snapshot);
            let weak_update = weak.clone();
            let shared_update = shared.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(window) = weak_update.upgrade() { ui::update(&window, &shared_update); }
            });
            let next_scan = std::time::Instant::now() + Duration::from_secs(30);
            loop {
                match rx.recv_timeout(next_scan.saturating_duration_since(std::time::Instant::now())) {
                    Ok(Command::Refresh) | Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Ok(Command::Quit) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                        let _ = slint::invoke_from_event_loop(|| { let _ = slint::quit_event_loop(); });
                        return;
                    }
                    Ok(Command::Details) => {
                        let weak = weak.clone();
                        let _ = slint::invoke_from_event_loop(move || { if let Some(w) = weak.upgrade() { let _ = w.show(); } });
                    }
                }
            }
        }
    });
    slint::run_event_loop_until_quit()?;
    Ok(())
}

use slint::ComponentHandle;
