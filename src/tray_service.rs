use crate::{BatteryTray, Command};
use anyhow::Result;
use ksni::Tray;
use std::sync::{Arc, Mutex};
use zbus::{blocking::{Connection, Proxy}, zvariant::ObjectPath};

const PATH: &str = "/StatusNotifierItem";
const INTERFACE: &str = "org.kde.StatusNotifierItem";
const WATCHER: &str = "org.kde.StatusNotifierWatcher";
type Pixmaps = Vec<(i32, i32, Vec<u8>)>;
type Tooltip = (String, Pixmaps, String, String);

pub struct Handle {
    state: Arc<Mutex<BatteryTray>>,
    connection: Connection,
}

impl Handle {
    pub fn update<R>(&self, update: impl FnOnce(&mut BatteryTray) -> R) -> Option<R> {
        let result = {
            let mut tray = self.state.lock().ok()?;
            update(&mut tray)
        };
        for signal in ["NewIcon", "NewToolTip"] {
            let _ = self.connection.emit_signal(None::<&str>, PATH, INTERFACE, signal, &());
        }
        Some(result)
    }
}

struct StatusNotifierItem(Arc<Mutex<BatteryTray>>);

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl StatusNotifierItem {
    fn activate(&self, x: i32, y: i32) {
        if let Ok(mut tray) = self.0.lock() { tray.activate(x, y); }
    }

    fn context_menu(&self, x: i32, y: i32) {
        if let Ok(tray) = self.0.lock() { let _ = tray.tx.send(Command::ContextMenu(x, y)); }
    }

    fn secondary_activate(&self, _x: i32, _y: i32) {}
    fn scroll(&self, _delta: i32, _orientation: &str) {}
    fn provide_xdg_activation_token(&self, _token: &str) {}

    #[zbus(signal)]
    async fn new_icon(emitter: &zbus::object_server::SignalEmitter<'_>) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn new_tool_tip(emitter: &zbus::object_server::SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(property)]
    fn category(&self) -> &str { "ApplicationStatus" }
    #[zbus(property)]
    fn id(&self) -> &str { "battery-life" }
    #[zbus(property)]
    fn title(&self) -> &str { "Battery Life" }
    #[zbus(property)]
    fn status(&self) -> &str { "Active" }
    #[zbus(property)]
    fn window_id(&self) -> i32 { 0 }
    #[zbus(property)]
    fn item_is_menu(&self) -> bool { false }
    #[zbus(property)]
    fn menu(&self) -> ObjectPath<'static> { ObjectPath::from_static_str_unchecked("/NO_DBUSMENU") }
    #[zbus(property)]
    fn icon_theme_path(&self) -> String {
        self.0.lock().map(|tray| tray.icon_theme_path()).unwrap_or_default()
    }
    #[zbus(property)]
    fn icon_name(&self) -> String {
        self.0.lock().map(|tray| tray.icon_name()).unwrap_or_default()
    }
    #[zbus(property)]
    fn icon_pixmap(&self) -> Pixmaps { Vec::new() }
    #[zbus(property)]
    fn overlay_icon_name(&self) -> &str { "" }
    #[zbus(property)]
    fn overlay_icon_pixmap(&self) -> Pixmaps { Vec::new() }
    #[zbus(property)]
    fn attention_icon_name(&self) -> &str { "" }
    #[zbus(property)]
    fn attention_icon_pixmap(&self) -> Pixmaps { Vec::new() }
    #[zbus(property)]
    fn attention_movie_name(&self) -> &str { "" }
    #[zbus(property)]
    fn tool_tip(&self) -> Tooltip {
        let tip = self.0.lock().map(|tray| tray.tool_tip()).unwrap_or_default();
        (tip.icon_name, tip.icon_pixmap.into_iter().map(|icon| (icon.width, icon.height, icon.data)).collect(), tip.title, tip.description)
    }
}

fn register(connection: &Connection) -> Result<()> {
    let watcher = Proxy::new(connection, WATCHER, "/StatusNotifierWatcher", WATCHER)?;
    watcher.call::<_, _, ()>("RegisterStatusNotifierItem", &(PATH,))?;
    Ok(())
}

pub fn spawn(tray: BatteryTray, builder: zbus::blocking::connection::Builder<'_>) -> Result<Handle> {
    let state = Arc::new(Mutex::new(tray));
    let connection = builder.serve_at(PATH, StatusNotifierItem(state.clone()))?.build()?;
    let dbus = zbus::blocking::fdo::DBusProxy::new(&connection)?;
    let changes = dbus.receive_name_owner_changed()?;
    register(&connection)?;
    let watcher_connection = connection.clone();
    std::thread::spawn(move || {
        for change in changes {
            if let Ok(args) = change.args() {
                if args.name().as_str() == WATCHER && args.new_owner().as_ref().is_some() {
                    if let Err(error) = register(&watcher_connection) {
                        eprintln!("Battery Life tray: {error:#}");
                    }
                }
            }
        }
    });
    Ok(Handle { state, connection })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Snapshot, tray_icon::Appearance};
    use std::sync::mpsc;

    #[test]
    fn right_click_requests_exit_menu_without_advertising_native_menu() {
        let (tx, rx) = mpsc::channel();
        let item = StatusNotifierItem(Arc::new(Mutex::new(BatteryTray {
            snapshot: Snapshot { version: "test", checked_at: 0, devices: vec![], warnings: vec![] },
            tx, appearance: Appearance::default(), icon_theme_path: String::new(), has_panel: true,
        })));
        assert_eq!(item.menu().as_str(), "/NO_DBUSMENU");
        assert!(!item.item_is_menu());
        item.context_menu(120, 800);
        assert!(matches!(rx.try_recv(), Ok(Command::ContextMenu(120, 800))));
        item.activate(120, 800);
        assert!(matches!(rx.try_recv(), Ok(Command::TogglePanel(120, 800))));
        assert_eq!(item.tool_tip().2, "Battery Life");
    }

    struct Watcher(mpsc::Sender<String>);
    #[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
    impl Watcher {
        fn register_status_notifier_item(&self, service: &str) {
            let _ = self.0.send(service.to_owned());
        }
    }

    #[test]
    fn dbus_registers_custom_context_menu_and_keeps_left_activation_separate() -> Result<()> {
        use std::{io::{BufRead, BufReader}, process::{Child, Command as ProcessCommand, Stdio}};
        struct Bus(Child);
        impl Drop for Bus {
            fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); }
        }
        let mut bus = Bus(ProcessCommand::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped()).spawn()?);
        let mut address = String::new();
        BufReader::new(bus.0.stdout.as_mut().unwrap()).read_line(&mut address)?;
        let (tx, rx) = mpsc::channel();
        let (registered, registrations) = mpsc::channel();
        let client = zbus::blocking::connection::Builder::address(address.trim())?
            .method_timeout(std::time::Duration::from_secs(3))
            .name(WATCHER)?.serve_at("/StatusNotifierWatcher", Watcher(registered))?.build()?;
        let handle = spawn(BatteryTray {
            snapshot: Snapshot { version: "test", checked_at: 0, devices: vec![], warnings: vec![] },
            tx, appearance: Appearance::default(), icon_theme_path: String::new(), has_panel: true,
        }, zbus::blocking::connection::Builder::address(address.trim())?)?;
        assert_eq!(registrations.recv_timeout(std::time::Duration::from_secs(1))?, PATH);
        let proxy = Proxy::new(&client, handle.connection.unique_name().unwrap().as_str(), PATH, INTERFACE)?;
        handle.update(|tray| tray.icon_theme_path = "/test/icons".into()).unwrap();
        let icon_path: String = proxy.get_property("IconThemePath")?;
        assert_eq!(icon_path, "/test/icons");
        let menu: zbus::zvariant::OwnedObjectPath = proxy.get_property("Menu")?;
        assert_eq!(menu.as_str(), "/NO_DBUSMENU");
        let tooltip: Tooltip = proxy.get_property("ToolTip")?;
        assert_eq!(tooltip.2, "Battery Life");
        proxy.call::<_, _, ()>("ContextMenu", &(120_i32, 800_i32))?;
        assert!(matches!(rx.recv_timeout(std::time::Duration::from_secs(1)), Ok(Command::ContextMenu(120, 800))));
        proxy.call::<_, _, ()>("Activate", &(140_i32, 820_i32))?;
        assert!(matches!(rx.recv_timeout(std::time::Duration::from_secs(1)), Ok(Command::TogglePanel(140, 820))));
        Ok(())
    }

}
