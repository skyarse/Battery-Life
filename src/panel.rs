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
        scale: 1, position: (0, 0),
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
                Message::Toggle(x, y) => {
                    if state.layer.is_some() {
                        state.close();
                    } else {
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
}

impl State {
    fn open(&mut self, qh: &QueueHandle<Self>, x: i32, y: i32) -> Result<()> {
        let mut choices = self.outputs.outputs().filter_map(|output| {
            let info = self.outputs.info(&output)?;
            Some((output, info))
        }).collect::<Vec<_>>();
        let selected = choices.iter().position(|(_, info)| {
            let (left, top) = info.logical_position.unwrap_or(info.location);
            let (width, height) = info.logical_size.unwrap_or((1920, 1080));
            x >= left && x < left + width && y >= top && y < top + height
        }).unwrap_or_default();
        anyhow::ensure!(!choices.is_empty(), "No Wayland output is available");
        let (output, info) = choices.swap_remove(selected);
        let (left, top) = info.logical_position.unwrap_or(info.location);
        let (width, height) = info.logical_size.unwrap_or((1920, 1080));
        self.scale = info.scale_factor.max(1);
        self.width = width.max(1) as u32;
        self.height = height.max(1) as u32;
        let (local_x, local_y) = ((x-left).clamp(0, width), (y-top).clamp(0, height));
        self.position = (local_x, local_y);
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
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer.commit();
        self.layer = Some(layer);
        self.configured = false;
        Ok(())
    }

    fn update_geometry(&self) {
        let (width, height) = (self.width as i32, self.height as i32);
        let panel_width = 390.min(width.saturating_sub(16)).max(1);
        let panel_height = (self.ui.get_desired_height().ceil() as i32).min(height.saturating_sub(16)).max(1);
        let (local_x, local_y) = self.position;
        let distances = [local_y, height-local_y, local_x, width-local_x];
        let edge = distances.iter().enumerate().min_by_key(|(_, distance)| **distance).map(|(i, _)| i).unwrap_or(1);
        let (px, py) = match edge {
            0 => (local_x-panel_width/2, local_y+8),
            1 => (local_x-panel_width/2, local_y-panel_height-8),
            2 => (local_x+8, local_y-panel_height/2),
            _ => (local_x-panel_width-8, local_y-panel_height/2),
        };
        let max_x = (width-panel_width).max(0);
        let max_y = (height-panel_height).max(0);
        let inset_x = 8.min(max_x/2);
        let inset_y = 8.min(max_y/2);
        self.ui.set_panel_x(px.clamp(inset_x, max_x-inset_x) as f32);
        self.ui.set_panel_y(py.clamp(inset_y, max_y-inset_y) as f32);
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
