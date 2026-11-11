use crate::{Command, Snapshot, panel_ui, tray_icon::Appearance};
use anyhow::{Context, Result};
use slint::{
    ComponentHandle, LogicalPosition, PhysicalSize,
    platform::{Platform, PlatformError, WindowAdapter, WindowEvent, PointerEventButton,
        software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType}},
};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState}, registry_handlers,
    reexports::{calloop::EventLoop, calloop_wayland_source::WaylandSource},
    seat::{Capability, SeatHandler, SeatState, pointer::{PointerEvent, PointerEventKind, PointerHandler}},
    shell::{WaylandSurface, wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure}},
    shm::{Shm, ShmHandler, slot::SlotPool},
};
use std::{rc::Rc, sync::mpsc, time::Duration};
use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum, globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
};

pub enum Message {
    Toggle(i32, i32),
    ContextMenu(i32, i32),
    Update(Snapshot, Appearance),
    Quit,
}

pub struct Panel {
    sender: mpsc::Sender<Message>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Panel {
    pub fn start(tx: mpsc::Sender<Command>) -> Self {
        let (sender, receiver) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            if let Err(error) = run(tx.clone(), receiver) {
                eprintln!("Battery Life panel: {error:#}");
                let _ = tx.send(Command::PanelUnavailable);
            }
        });
        Self { sender, thread: Some(thread) }
    }

    pub fn send(&self, message: Message) {
        let _ = self.sender.send(message);
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        let _ = self.sender.send(Message::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct SoftwarePlatform(Rc<MinimalSoftwareWindow>);
impl Platform for SoftwarePlatform {
    fn create_window_adapter(&self) -> std::result::Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.0.clone())
    }
}

fn run(tx: mpsc::Sender<Command>, receiver: mpsc::Receiver<Message>) -> Result<()> {
    let connection = Connection::connect_to_env().context("Wayland connection failed")?;
    let (globals, queue) = registry_queue_init(&connection)?;
    let qh = queue.handle();
    let compositor = CompositorState::bind(&globals, &qh)?;
    let shell = LayerShell::bind(&globals, &qh).context("The compositor does not support layer-shell")?;
    let shm = Shm::bind(&globals, &qh)?;
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    slint::platform::set_platform(Box::new(SoftwarePlatform(window.clone())))?;
    let (dismiss, dismiss_rx) = mpsc::channel();
    let ui = panel_ui::create(tx, dismiss)?;
    let mut state = State {
        registry: RegistryState::new(&globals), outputs: OutputState::new(&globals, &qh),
        seats: SeatState::new(&globals, &qh), shm,
        compositor, shell, layer: None, window, ui, pool: None, pixels: Vec::new(),
        pointer: None, keyboard: None, configured: false, width: 1, height: 1,
        scale: 1, position: (0, 0), output_size: (1, 1),
    };
    let mut event_loop = EventLoop::<State>::try_new()?;
    WaylandSource::new(connection.clone(), queue).insert(event_loop.handle()).map_err(|e| anyhow::anyhow!("{e}"))?;

    loop {
        let wait = if state.layer.is_some() { 16 } else { 160 };
        event_loop.dispatch(Duration::from_millis(wait), &mut state)?;
        loop {
            let message = match receiver.try_recv() {
                Ok(message) => message,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            };
            match message {
                Message::Quit => return Ok(()),
                Message::Update(snapshot, appearance) => panel_ui::update(&state.ui, &snapshot, appearance),
                Message::ContextMenu(x, y) => {
                    state.close();
                    state.ui.set_exit_menu(true);
                    state.open(&qh, x, y)?;
                }
                Message::Toggle(x, y) => {
                    if state.layer.is_some() && !state.ui.get_exit_menu() {
                        state.close();
                    } else {
                        state.close();
                        state.ui.set_exit_menu(false);
                        state.open(&qh, x, y)?;
                    }
                }
            }
        }
        if dismiss_rx.try_recv().is_ok() {
            state.close();
        }
        slint::platform::update_timers_and_animations();
        state.draw()?;
        connection.flush()?;
    }
}

struct State {
    registry: RegistryState,
    outputs: OutputState,
    seats: SeatState,
    shm: Shm,
    compositor: CompositorState,
    shell: LayerShell,
    layer: Option<LayerSurface>,
    window: Rc<MinimalSoftwareWindow>,
    ui: panel_ui::TrayPanel,
    pool: Option<SlotPool>,
    pixels: Vec<PremultipliedRgbaColor>,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    configured: bool,
    width: u32,
    height: u32,
    scale: i32,
    position: (i32, i32),
    output_size: (i32, i32),
}

impl State {
    fn open(&mut self, qh: &QueueHandle<Self>, x: i32, y: i32) -> Result<()> {
        let mut choices = self.outputs.outputs().filter_map(|output| {
            let info = self.outputs.info(&output)?;
            Some((output, info))
        }).collect::<Vec<_>>();
        anyhow::ensure!(!choices.is_empty(), "No Wayland output is available");
        let native_coordinates = plasma_uses_native_coordinates();
        let single_output = choices.len() == 1;
        let selected = choices.iter().position(|(_, info)| {
            activation_geometry(info, native_coordinates, single_output).contains((x, y))
        }).unwrap_or_default();
        let (output, info) = choices.swap_remove(selected);
        let geometry = activation_geometry(&info, native_coordinates, single_output);
        let (width, height) = geometry.logical_size;
        self.scale = info.scale_factor.max(1);
        self.width = width.max(1) as u32;
        self.height = height.max(1) as u32;
        let (local_x, local_y) = geometry.local_position((x, y));
        self.position = (local_x, local_y);
        self.output_size = (width, height);
        self.ui.set_settings(false);
        self.ui.set_expanded_id("".into());
        self.update_geometry();
        self.window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: self.scale as f32 });
        self.window.set_size(PhysicalSize::new(self.width*self.scale as u32, self.height*self.scale as u32));
        self.ui.show()?;
        let surface = self.compositor.create_surface(qh);
        surface.set_buffer_scale(self.scale);
        let layer = self.shell.create_layer_surface(qh, surface, Layer::Overlay, Some("battery-life-panel"), Some(&output));
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_size(0, 0);
        layer.set_exclusive_zone(0);
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer.commit();
        self.layer = Some(layer);
        self.configured = false;
        Ok(())
    }

    fn update_geometry(&self) {
        let desired_height = self.ui.get_desired_height().ceil() as i32;
        let available = (self.width as i32, self.height as i32);
        let (px, py, panel_width, panel_height) = if self.ui.get_exit_menu() {
            popup_geometry_with_width(self.output_size, available, self.position, desired_height, 144)
        } else {
            popup_geometry(self.output_size, available, self.position, desired_height)
        };
        self.ui.set_panel_x(px as f32);
        self.ui.set_panel_y(py as f32);
        self.ui.set_panel_width(panel_width as f32);
        self.ui.set_panel_height(panel_height as f32);
    }

    fn close(&mut self) {
        self.layer.take();
        self.configured = false;
        let _ = self.ui.hide();
    }

    fn draw(&mut self) -> Result<()> {
        self.update_geometry();
        let Some(layer) = self.layer.as_ref().filter(|_| self.configured) else { return Ok(()); };
        let (width, height) = (self.width*self.scale as u32, self.height*self.scale as u32);
        self.pixels.resize((width as usize)*(height as usize), PremultipliedRgbaColor::default());
        let rendered = self.window.draw_if_needed(|renderer| {
            renderer.render(&mut self.pixels, width as usize);
        });
        if !rendered { return Ok(()); }
        if self.pool.is_none() {
            self.pool = Some(SlotPool::new(self.pixels.len()*4, &self.shm)?);
        }
        let (buffer, canvas) = self.pool.as_mut().unwrap()
            .create_buffer(width as i32, height as i32, width as i32*4, wl_shm::Format::Argb8888)?;
        for (target, pixel) in canvas.chunks_exact_mut(4).zip(&self.pixels) {
            target.copy_from_slice(&[pixel.blue, pixel.green, pixel.red, pixel.alpha]);
        }
        layer.wl_surface().damage_buffer(0, 0, width as i32, height as i32);
        buffer.attach_to(layer.wl_surface())?;
        layer.commit();
        Ok(())
    }
}

struct ActivationGeometry {
    origin: (i32, i32),
    extent: (i32, i32),
    logical_size: (i32, i32),
}

impl ActivationGeometry {
    fn contains(&self, point: (i32, i32)) -> bool {
        point.0 >= self.origin.0 && point.0 < self.origin.0 + self.extent.0
            && point.1 >= self.origin.1 && point.1 < self.origin.1 + self.extent.1
    }

    fn local_position(&self, point: (i32, i32)) -> (i32, i32) {
        let convert = |value: i32, origin: i32, extent: i32, logical: i32| {
            (((value - origin) as f64 * logical as f64 / extent.max(1) as f64).round() as i32).clamp(0, logical)
        };
        (convert(point.0, self.origin.0, self.extent.0, self.logical_size.0),
         convert(point.1, self.origin.1, self.extent.1, self.logical_size.1))
    }
}

fn activation_geometry(info: &smithay_client_toolkit::output::OutputInfo, native: bool, single: bool) -> ActivationGeometry {
    let origin = info.logical_position.unwrap_or(info.location);
    let logical_size = info.logical_size.unwrap_or((1920, 1080));
    let extent = if native {
        info.modes.iter().find(|mode| mode.current).map(|mode| {
            match info.transform {
                wl_output::Transform::_90 | wl_output::Transform::_270
                | wl_output::Transform::Flipped90 | wl_output::Transform::Flipped270 => (mode.dimensions.1, mode.dimensions.0),
                _ => mode.dimensions,
            }
        }).unwrap_or((logical_size.0 * info.scale_factor.max(1), logical_size.1 * info.scale_factor.max(1)))
    } else {
        logical_size
    };
    ActivationGeometry { origin: if native && single { (0, 0) } else { origin }, extent, logical_size }
}

fn xwayland_clients_scale(config: &str) -> Option<bool> {
    let mut kscreen = false;
    let mut result = None;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            kscreen = line == "[KScreen]" || line == "[KScreen][$i]";
        } else if kscreen {
            if let Some((key, value)) = line.split_once('=') {
                if key.trim().trim_end_matches("[$i]") == "XwaylandClientsScale" {
                    result = match value.trim().to_ascii_lowercase().as_str() {
                        "false" | "0" => Some(false),
                        "true" | "1" => Some(true),
                        _ => result,
                    };
                }
            }
        }
    }
    result
}

fn plasma_uses_native_coordinates() -> bool {
    let kde = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default()
        .split(':').any(|desktop| desktop.eq_ignore_ascii_case("KDE"))
        || std::env::var("KDE_FULL_SESSION").is_ok_and(|value| value == "true");
    if !kde { return false; }
    let mut directories = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config")))
        .into_iter().collect::<Vec<_>>();
    directories.extend(std::env::split_paths(&std::env::var_os("XDG_CONFIG_DIRS").unwrap_or_else(|| "/etc/xdg".into())));
    directories.into_iter().find_map(|directory| {
        std::fs::read_to_string(directory.join("kdeglobals")).ok().and_then(|text| xwayland_clients_scale(&text))
    }).unwrap_or(true)
}

fn popup_geometry(output: (i32, i32), available: (i32, i32), position: (i32, i32), desired_height: i32) -> (i32, i32, i32, i32) {
    popup_geometry_with_width(output, available, position, desired_height, 288)
}

fn popup_geometry_with_width(output: (i32, i32), available: (i32, i32), position: (i32, i32), desired_height: i32, preferred_width: i32) -> (i32, i32, i32, i32) {
    let (width, height) = (available.0.max(1), available.1.max(1));
    let reserved_x = (output.0 - width).max(0);
    let reserved_y = (output.1 - height).max(0);
    let at_top = position.1 < output.1 / 2;
    let at_left = position.0 < output.0 / 2;
    let local_x = (position.0 - if at_left { reserved_x } else { 0 }).clamp(0, width);
    let local_y = (position.1 - if at_top { reserved_y } else { 0 }).clamp(0, height);
    let panel_width = preferred_width.min(width.saturating_sub(16)).max(1);
    let panel_height = desired_height.min(height.saturating_sub(16)).max(1);
    let horizontal_panel = if reserved_y > 0 {
        true
    } else if reserved_x > 0 {
        false
    } else {
        let vertical_distance = position.1.min(output.1 - position.1);
        let horizontal_distance = position.0.min(output.0 - position.0);
        vertical_distance <= 96 || vertical_distance <= horizontal_distance
    };
    let (x, y) = if horizontal_panel {
        (local_x - panel_width / 2, if at_top { local_y + 8 } else { local_y - panel_height - 8 })
    } else {
        (if at_left { local_x + 8 } else { local_x - panel_width - 8 }, local_y - panel_height / 2)
    };
    let max_x = (width - panel_width).max(0);
    let max_y = (height - panel_height).max(0);
    let inset_x = 8.min(max_x / 2);
    let inset_y = 8.min(max_y / 2);
    (x.clamp(inset_x, max_x - inset_x), y.clamp(inset_y, max_y - inset_y), panel_width, panel_height)
}

#[cfg(test)]
mod geometry_tests {
    use super::{popup_geometry, ActivationGeometry, xwayland_clients_scale};

    #[test]
    fn fractional_plasma_coordinates_center_popup_on_the_icon() {
        for (extent, point) in [((2560, 1440), (2000, 1410)), ((3072, 1728), (2400, 1692)), ((4096, 2304), (3200, 2256))] {
            let geometry = ActivationGeometry { origin: (0, 0), extent, logical_size: (2048, 1152) };
            assert!(geometry.contains(point));
            let position = geometry.local_position(point);
            assert_eq!(position, (1600, 1128));
            let (x, y, w, h) = popup_geometry((2048, 1152), (2048, 1104), position, 190);
            assert_eq!(x + w / 2, 1600);
            assert_eq!(y + h, 1096);
        }
    }

    #[test]
    fn logical_coordinates_and_nonzero_screen_origins_are_preserved() {
        let logical = ActivationGeometry { origin: (-1920, 0), extent: (1920, 1080), logical_size: (1920, 1080) };
        assert!(logical.contains((-320, 1050)));
        assert!(!logical.contains((100, 1050)));
        assert_eq!(logical.local_position((-320, 1050)), (1600, 1050));
        let native = ActivationGeometry { origin: (1920, 0), extent: (2560, 1440), logical_size: (2048, 1152) };
        assert!(native.contains((3920, 1410)));
        assert_eq!(native.local_position((3920, 1410)), (1600, 1128));
    }

    #[test]
    fn plasma_coordinate_setting_respects_explicit_false_and_config_groups() {
        assert_eq!(xwayland_clients_scale("[KScreen]\nXwaylandClientsScale=false\n"), Some(false));
        assert_eq!(xwayland_clients_scale("[KScreen]\nXwaylandClientsScale=true\n"), Some(true));
        assert_eq!(xwayland_clients_scale("[Other]\nXwaylandClientsScale=false\n"), None);
        assert_eq!(xwayland_clients_scale("[KScreen][$i]\nXwaylandClientsScale[$i]=0\n"), Some(false));
    }


    #[test]
    fn bottom_tray_opens_above_reserved_panel_even_near_a_corner() {
        let (x, y, w, h) = popup_geometry((1920, 1080), (1920, 1020), (120, 1050), 190);
        assert_eq!(x, 8);
        assert_eq!(y + h, 1012);
        assert_eq!(w, 288);
        let (x, y, w, h) = popup_geometry((1920, 1080), (1920, 1020), (1200, 1050), 370);
        assert_eq!(x + w / 2, 1200);
        assert_eq!(y + h, 1012);
    }

    #[test]
    fn top_and_side_panels_use_available_surface_coordinates() {
        let (x, y, w, _) = popup_geometry((1920, 1080), (1920, 1020), (1200, 30), 190);
        assert_eq!(x + w / 2, 1200);
        assert_eq!(y, 8);
        let (x, y, _, h) = popup_geometry((1920, 1080), (1860, 1080), (30, 700), 370);
        assert_eq!(x, 8);
        assert_eq!(y + h / 2, 700);
        let (x, _, w, _) = popup_geometry((1920, 1080), (1860, 1080), (1890, 700), 370);
        assert_eq!(x + w, 1852);
    }

    #[test]
    fn unreserved_bottom_corner_still_opens_above_the_icon() {
        let (x, y, _, h) = popup_geometry((1920, 1080), (1920, 1080), (20, 1050), 190);
        assert_eq!(x, 8);
        assert_eq!(y + h, 1042);
    }

    #[test]
    fn expanded_and_small_screen_popups_stay_inside_work_area() {
        for size in [(1920, 1020), (320, 240), (200, 150), (1, 1)] {
            for position in [(0, 0), (1910, 1070), (960, 1050)] {
                let (x, y, w, h) = popup_geometry((1920, 1080), size, position, 560);
                assert!(x >= 0 && y >= 0 && w > 0 && h > 0);
                assert!(x + w <= size.0 && y + h <= size.1);
            }
        }
    }
}

impl LayerShellHandler for State {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) { self.close(); }
    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface, configure: LayerSurfaceConfigure, _: u32) {
        if configure.new_size.0 > 0 && configure.new_size.1 > 0 {
            self.width = configure.new_size.0;
            self.height = configure.new_size.1;
            self.window.set_size(PhysicalSize::new(self.width*self.scale as u32, self.height*self.scale as u32));
        }
        self.configured = true;
    }
}

impl CompositorHandler for State {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &wl_surface::WlSurface, scale: i32) {
        if self.layer.as_ref().is_some_and(|layer| layer.wl_surface() == surface) {
            self.scale = scale.max(1);
            surface.set_buffer_scale(self.scale);
            self.window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: self.scale as f32 });
            self.window.set_size(PhysicalSize::new(self.width*self.scale as u32, self.height*self.scale as u32));
        }
    }
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
}
impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState { &mut self.outputs }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) { self.close(); }
}
impl SeatHandler for State {
    fn seat_state(&mut self) -> &mut SeatState { &mut self.seats }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat, capability: Capability) {
        match capability {
            Capability::Pointer if self.pointer.is_none() => self.pointer = Some(self.seats.get_pointer(qh, &seat).expect("pointer capability")),
            Capability::Keyboard if self.keyboard.is_none() => self.keyboard = Some(seat.get_keyboard(qh, ())),
            _ => {}
        }
    }
    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat, capability: Capability) {
        match capability {
            Capability::Pointer => { if let Some(pointer) = self.pointer.take() { pointer.release(); } }
            Capability::Keyboard => { if let Some(keyboard) = self.keyboard.take() { keyboard.release(); } }
            _ => {}
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}
impl PointerHandler for State {
    fn pointer_frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_pointer::WlPointer, events: &[PointerEvent]) {
        for event in events {
            if !self.layer.as_ref().is_some_and(|layer| layer.wl_surface() == &event.surface) { continue; }
            let position = LogicalPosition::new(event.position.0 as f32, event.position.1 as f32);
            let input = match event.kind {
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => WindowEvent::PointerMoved { position },
                PointerEventKind::Leave { .. } => WindowEvent::PointerExited,
                PointerEventKind::Press { button, .. } | PointerEventKind::Release { button, .. } => {
                    let button = match button { 0x110 => PointerEventButton::Left, 0x111 => PointerEventButton::Right, 0x112 => PointerEventButton::Middle, _ => PointerEventButton::Other };
                    if matches!(event.kind, PointerEventKind::Press { .. }) { WindowEvent::PointerPressed { position, button } }
                    else { WindowEvent::PointerReleased { position, button } }
                }
                PointerEventKind::Axis { horizontal, vertical, .. } => WindowEvent::PointerScrolled { position, delta_x: -(horizontal.absolute as f32), delta_y: -(vertical.absolute as f32) },
            };
            self.window.dispatch_event(input);
        }
    }
}
impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(state: &mut Self, _: &wl_keyboard::WlKeyboard, event: wl_keyboard::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_keyboard::Event::Key { key: 1, state: WEnum::Value(wl_keyboard::KeyState::Pressed), .. } => state.close(),
            wl_keyboard::Event::Enter { surface, .. } if state.layer.as_ref().is_some_and(|layer| layer.wl_surface() == &surface) => {
                state.window.dispatch_event(WindowEvent::WindowActiveChanged(true));
                if let Some(layer) = &state.layer {
                    layer.set_keyboard_interactivity(KeyboardInteractivity::OnDemand);
                    layer.commit();
                }
            }
            wl_keyboard::Event::Leave { surface, .. } if state.layer.as_ref().is_some_and(|layer| layer.wl_surface() == &surface) => {
                state.window.dispatch_event(WindowEvent::WindowActiveChanged(false));
                state.close();
            }
            wl_keyboard::Event::Key { key, state: WEnum::Value(key_state), .. } => {
                use slint::platform::Key;
                let text = match key {
                    15 => Key::Tab.into(), 28 => Key::Return.into(), 57 => " ".into(),
                    103 => Key::UpArrow.into(), 108 => Key::DownArrow.into(),
                    105 => Key::LeftArrow.into(), 106 => Key::RightArrow.into(),
                    _ => return,
                };
                let event = if key_state == wl_keyboard::KeyState::Pressed { WindowEvent::KeyPressed { text } } else { WindowEvent::KeyReleased { text } };
                state.window.dispatch_event(event);
            }
            _ => {}
        }
    }
}
impl ShmHandler for State { fn shm_state(&mut self) -> &mut Shm { &mut self.shm } }
impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState { &mut self.registry }
    registry_handlers![OutputState, SeatState];
}
delegate_compositor!(State);
delegate_output!(State);
delegate_shm!(State);
delegate_seat!(State);
delegate_pointer!(State);
delegate_layer!(State);
delegate_registry!(State);
