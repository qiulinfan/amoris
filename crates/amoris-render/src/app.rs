//! A native window that draws a [`Renderer`] (winit): the `amoris` binary's game window, the
//! examples and the benchmarks. A [`Host`] feeds the renderer each frame. Optional fly camera
//! (right mouse to look, WASD/QE to move, wheel for speed), first-person devices for a host that
//! walks (keys, the mouse and gamepads, docs/spec/input.md 6) and a benchmark mode that measures a
//! fixed number of frames and reports.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use amoris_assets::input::{DeviceFrame, Pad};

use glam::{Quat, Vec3};
use winit::application::ApplicationHandler;
use winit::event::{
    DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::camera::CameraState;
use crate::gpu::{BackendChoice, Gpu, instance};
use crate::pad::{PadEvent, Pads};
use crate::renderer::{FrameStats, Renderer, web_time};

/// What drives the window's renderer.
pub trait Host {
    /// Before each frame: apply the render feed, move cameras.
    fn update(&mut self, renderer: &mut Renderer, now_s: f64);
    /// A key went down or up (the key's winit name, e.g. `KeyW`).
    fn key(&mut self, _key: &str, _pressed: bool) {}
    /// Presenter-only first-person input; the host owns collision and the camera pose.
    fn walk_input(&mut self, _input: WalkInput) {}
    /// Optional loading or interaction status appended to the native window title.
    fn status(&self) -> Option<&str> {
        None
    }
    /// The pointer pressed a game UI button (docs/spec/ui.md 6): the host sends `ui.press`.
    fn ui_press(&mut self, _entity: u64) {}
    /// After each frame, with its stats.
    fn after_frame(&mut self, _stats: &FrameStats) {}
    /// Whether to read the next frame back (a screenshot); `captured` receives it.
    fn wants_capture(&mut self) -> bool {
        false
    }
    /// The captured frame, RGBA8 sRGB, tightly packed. Return `true` to close the window.
    fn captured(&mut self, _width: u32, _height: u32, _rgba: Vec<u8>) -> bool {
        false
    }
}

/// One frame of native first-person devices (docs/spec/input.md 2), which the host resolves
/// through its input map (`amoris_assets::input::Seat`), with the window's own keys.
#[derive(Clone, Debug, Default)]
pub struct WalkInput {
    /// Every key and mouse button held while captured, by name (`KeyQ`, `Space`, `MouseRight`),
    /// their presses and the wheel's notches since the last input, the mouse's raw movement, the
    /// active gamepad while the window is focused (captured or not, so the press that captured is
    /// not also an action) and the frame's seconds (at most 0.1).
    pub devices: DeviceFrame,
    pub captured: bool,
    /// `R` pressed while captured: the collision camera's respawn.
    pub reset: bool,
    pub gi_toggle: bool,
    /// The active gamepad's name.
    pub pad: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RunOptions {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub backend: BackendChoice,
    pub vsync: bool,
    /// Start with this free camera (the host may still override it).
    pub fly_camera: Option<CameraState>,
    /// Capture the mouse on click and forward first-person controls to the host.
    pub walk: bool,
    /// Benchmark: frames to skip, then frames to measure; the window closes after.
    pub bench: Option<(u32, u32)>,
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions {
            title: "Amoris".into(),
            width: 1280,
            height: 720,
            backend: crate::host_backend(),
            vsync: true,
            fly_camera: None,
            walk: false,
            bench: None,
        }
    }
}

struct WalkControls {
    keys: HashSet<KeyCode>,
    look: [f32; 2],
    reset: bool,
    gi_toggle: bool,
    captured: bool,
    wheel: f32,
    /// Mouse buttons held while captured, and presses since the last input, by name.
    mouse: HashSet<&'static str>,
    pressed: Vec<String>,
    /// The gamepads (none where gilrs has no backend), read only while the window has focus.
    pads: Option<Pads>,
    focused: bool,
}

impl Default for WalkControls {
    fn default() -> WalkControls {
        WalkControls {
            keys: HashSet::new(),
            look: [0.0; 2],
            reset: false,
            gi_toggle: false,
            captured: false,
            wheel: 0.0,
            mouse: HashSet::new(),
            pressed: Vec::new(),
            pads: None,
            focused: true,
        }
    }
}

/// A mouse button's name for a game's buttons.
fn mouse_name(button: MouseButton) -> Option<&'static str> {
    match button {
        MouseButton::Left => Some("MouseLeft"),
        MouseButton::Right => Some("MouseRight"),
        MouseButton::Middle => Some("MouseMiddle"),
        _ => None,
    }
}

impl WalkControls {
    fn release(&mut self, window: &Window) {
        let _ = window.set_cursor_grab(CursorGrabMode::None);
        window.set_cursor_visible(true);
        self.keys.clear();
        self.mouse.clear();
        self.pressed.clear();
        self.look = [0.0; 2];
        self.reset = false;
        self.gi_toggle = false;
        self.captured = false;
        self.wheel = 0.0;
    }

    fn capture(&mut self, window: &Window) {
        match window
            .set_cursor_grab(CursorGrabMode::Locked)
            .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined))
        {
            Ok(()) => {
                self.captured = true;
                window.set_cursor_visible(false);
            }
            Err(e) => eprintln!("could not capture mouse: {e}"),
        }
    }

    /// Drains the gamepads' events: while the window has focus, `PadStart` releases a captured
    /// mouse as Escape does, and any button captures a released one as a click does. Returns the
    /// active pad's state while focused.
    fn poll_pads(&mut self, window: &Window) -> Option<Pad> {
        let events = self.pads.as_mut()?.poll();
        for e in events {
            match e {
                PadEvent::Connected(name) => eprintln!("gamepad: {name} connected"),
                PadEvent::Disconnected(name) => eprintln!("gamepad: {name} disconnected"),
                PadEvent::Pressed(button) if self.focused => {
                    if button == "PadStart" && self.captured {
                        self.release(window);
                    } else if !self.captured {
                        self.capture(window);
                    }
                }
                PadEvent::Pressed(_) => {}
            }
        }
        if self.focused {
            self.pads.as_ref()?.state()
        } else {
            None
        }
    }

    fn input(&mut self, dt_s: f32, pad: Option<Pad>) -> WalkInput {
        let mut held = BTreeMap::new();
        if self.captured {
            for k in &self.keys {
                held.insert(format!("{k:?}"), 1.0);
            }
            for m in &self.mouse {
                held.insert((*m).to_owned(), 1.0);
            }
        }
        WalkInput {
            devices: DeviceFrame {
                held,
                pressed: std::mem::take(&mut self.pressed),
                look: std::mem::take(&mut self.look),
                pad,
                dt_s: dt_s.clamp(0.0, 0.1),
            },
            captured: self.captured,
            reset: std::mem::take(&mut self.reset),
            gi_toggle: std::mem::take(&mut self.gi_toggle),
            pad: self.pads.as_ref().and_then(Pads::name),
        }
    }
}

/// A benchmark's measurements.
#[derive(Clone, Debug, Default)]
pub struct BenchReport {
    pub backend: String,
    pub adapter: String,
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    /// Wall time between presented frames, milliseconds.
    pub frame_ms_mean: f64,
    pub frame_ms_p50: f64,
    pub frame_ms_p95: f64,
    pub frame_ms_p99: f64,
    pub frame_ms_max: f64,
    pub gpu_ms_mean: f64,
    pub cpu_ms_mean: f64,
    pub passes: Vec<(String, f64)>,
    pub instances: usize,
    /// The last frame's occlusion culling and depth prepass (`FrameStats::occlusion`, `prepass`).
    pub occlusion: String,
    pub prepass: String,
    /// Measured frames that drew the depth prepass.
    pub prepass_frames: u32,
}

struct Fly {
    cam: CameraState,
    yaw: f32,
    pitch: f32,
    keys: HashSet<KeyCode>,
    looking: bool,
    last_cursor: Option<(f64, f64)>,
    speed: f32,
}

impl Fly {
    fn new(cam: CameraState) -> Fly {
        let f = cam.forward();
        Fly {
            yaw: f.x.atan2(-f.z),
            pitch: f.y.clamp(-1.0, 1.0).asin(),
            cam,
            keys: HashSet::new(),
            looking: false,
            last_cursor: None,
            speed: 10.0,
        }
    }

    fn step(&mut self, dt: f32) {
        self.cam.rotation = Quat::from_rotation_y(-self.yaw) * Quat::from_rotation_x(self.pitch);
        let f = self.cam.rotation * -Vec3::Z;
        let r = self.cam.rotation * Vec3::X;
        let mut m = Vec3::ZERO;
        let k = |c| self.keys.contains(&c);
        if k(KeyCode::KeyW) {
            m += f;
        }
        if k(KeyCode::KeyS) {
            m -= f;
        }
        if k(KeyCode::KeyD) {
            m += r;
        }
        if k(KeyCode::KeyA) {
            m -= r;
        }
        if k(KeyCode::KeyE) {
            m += Vec3::Y;
        }
        if k(KeyCode::KeyQ) {
            m -= Vec3::Y;
        }
        let boost = if k(KeyCode::ShiftLeft) { 4.0 } else { 1.0 };
        self.cam.position += m.normalize_or_zero() * self.speed * boost * dt;
    }
}

struct State {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    fly: Option<Fly>,
    walk: Option<WalkControls>,
    last_frame: f64,
    frame_times: Vec<f64>,
    gpu_times: Vec<f64>,
    cpu_times: Vec<f64>,
    prepass_frames: u32,
    frames: u32,
    title_at: f64,
    /// The game UI's pointer (docs/spec/ui.md 6): where the cursor is in physical pixels, whether
    /// the left button is down, and the button it went down on.
    cursor: Option<[f32; 2]>,
    ui_down: bool,
    ui_target: Option<u64>,
}

impl State {
    /// Whether the mouse is captured for first-person play (then it never touches the UI).
    fn captured(&self) -> bool {
        self.walk.as_ref().is_some_and(|w| w.captured)
    }

    /// The UI button under the cursor while the mouse is free.
    fn ui_under_cursor(&self) -> Option<u64> {
        let [x, y] = self.cursor?;
        if self.captured() {
            return None;
        }
        self.renderer.ui_hit(x, y)
    }
}

struct App<H: Host> {
    host: H,
    options: RunOptions,
    state: Option<State>,
    report: Option<BenchReport>,
    error: Option<String>,
}

impl<H: Host> App<H> {
    fn init(&mut self, el: &ActiveEventLoop) -> Result<(), String> {
        let attrs = Window::default_attributes()
            .with_title(&self.options.title)
            .with_inner_size(winit::dpi::LogicalSize::new(
                self.options.width,
                self.options.height,
            ));
        let window = Arc::new(el.create_window(attrs).map_err(|e| e.to_string())?);
        let inst = instance(self.options.backend);
        let surface = inst
            .create_surface(window.clone())
            .map_err(|e| e.to_string())?;
        let gpu = pollster::block_on(Gpu::new(inst, Some(&surface))).map_err(|e| e.to_string())?;
        let caps = surface.get_capabilities(&gpu.adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let size = window.inner_size();
        let present_mode = if self.options.vsync {
            wgpu::PresentMode::AutoVsync
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        log::info!(
            "surface {:?}, present modes {:?}, chosen {:?}",
            format,
            caps.present_modes,
            present_mode
        );
        surface.configure(&gpu.device, &config);
        let mut renderer = Renderer::new(&gpu, format, config.width, config.height);
        renderer.set_ui_scale(window.scale_factor() as f32);
        self.state = Some(State {
            window,
            surface,
            config,
            renderer,
            fly: self.options.fly_camera.map(Fly::new),
            walk: self.options.walk.then(|| WalkControls {
                pads: Pads::new(),
                ..WalkControls::default()
            }),
            last_frame: web_time(),
            frame_times: Vec::new(),
            gpu_times: Vec::new(),
            cpu_times: Vec::new(),
            prepass_frames: 0,
            frames: 0,
            title_at: 0.0,
            cursor: None,
            ui_down: false,
            ui_target: None,
        });
        Ok(())
    }

    fn frame(&mut self, el: &ActiveEventLoop) {
        let Some(st) = self.state.as_mut() else {
            return;
        };
        let now = web_time();
        let dt = (now - st.last_frame) as f32;
        st.last_frame = now;
        // A frame trace's frame (trace.rs): the update, the acquire, the render and the present.
        st.renderer.trace_begin_frame();
        let ts = st.renderer.trace_mark();
        if let Some(fly) = st.fly.as_mut() {
            fly.step(dt.min(0.1));
            st.renderer.set_camera_override(Some(fly.cam));
        }
        if let Some(walk) = st.walk.as_mut() {
            let pad = walk.poll_pads(&st.window);
            self.host.walk_input(walk.input(dt, pad));
        }
        self.host.update(&mut st.renderer, now);
        let pointer = if st.captured() { None } else { st.cursor };
        st.renderer.set_ui_pointer(pointer, st.ui_down);
        st.renderer.trace_span("update", ts);
        let ts = st.renderer.trace_mark();
        let tex = match st.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                st.surface.configure(&st.renderer.gpu().device, &st.config);
                st.renderer.trace_end_frame();
                return;
            }
            _ => {
                st.renderer.trace_end_frame();
                return;
            }
        };
        st.renderer.trace_span("acquire", ts);
        let view = tex
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let stats = st.renderer.render(&view, now);
        let ts = st.renderer.trace_mark();
        st.window.pre_present_notify();
        st.renderer.gpu().queue.present(tex);
        st.renderer.trace_span("present", ts);
        // A complete trace is written now, one long frame, or during a benchmark once the window
        // closes (`run`), after the measured frames.
        if st.renderer.trace_end_frame() && self.options.bench.is_none() {
            st.renderer.finish_trace();
        }
        self.host.after_frame(&stats);
        if self.host.wants_capture() {
            let (w, h, px) = st.renderer.capture_rgba(now);
            if self.host.captured(w, h, px) {
                el.exit();
            }
        }
        st.frames += 1;
        if let Some((skip, measure)) = self.options.bench {
            if st.frames > skip {
                st.frame_times.push(f64::from(dt) * 1000.0);
                st.gpu_times.push(f64::from(stats.gpu_ms));
                st.cpu_times.push(f64::from(stats.cpu_ms));
                st.prepass_frames += u32::from(stats.prepass.ends_with("on"));
            }
            if st.frames >= skip + measure {
                let mut ft = st.frame_times.clone();
                ft.sort_by(f64::total_cmp);
                let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
                let gpu = st.renderer.gpu();
                self.report = Some(BenchReport {
                    backend: gpu.backend_name().into(),
                    adapter: gpu.info.name.clone(),
                    width: st.config.width,
                    height: st.config.height,
                    frames: measure,
                    frame_ms_mean: mean(&st.frame_times),
                    frame_ms_p50: ft[ft.len() / 2],
                    frame_ms_p95: ft[(ft.len() * 95 / 100).min(ft.len() - 1)],
                    frame_ms_p99: ft[(ft.len() * 99 / 100).min(ft.len() - 1)],
                    frame_ms_max: ft[ft.len() - 1],
                    gpu_ms_mean: mean(&st.gpu_times),
                    cpu_ms_mean: mean(&st.cpu_times),
                    passes: stats
                        .passes
                        .iter()
                        .map(|(l, m)| ((*l).to_owned(), f64::from(*m)))
                        .collect(),
                    instances: stats.instances,
                    occlusion: stats.occlusion.into(),
                    prepass: stats.prepass.into(),
                    prepass_frames: st.prepass_frames,
                });
                el.exit();
            }
        }
        if now - st.title_at > 0.5 {
            st.title_at = now;
            if self.options.walk {
                let pad = st
                    .walk
                    .as_ref()
                    .and_then(|w| w.pads.as_ref())
                    .and_then(Pads::name)
                    .map(|n| format!(" — gamepad: {n}"))
                    .unwrap_or_default();
                st.window.set_title(&format!(
                    "{} — {}{pad}",
                    self.options.title,
                    self.host.status().unwrap_or("Walk")
                ));
                return;
            }
            st.window.set_title(&format!(
                "{}{} — {} {}x{} — {:.2} ms frame, {:.2} ms GPU, {} instances",
                self.options.title,
                self.host
                    .status()
                    .map(|s| format!(" — {s}"))
                    .unwrap_or_default(),
                stats.backend,
                st.config.width,
                st.config.height,
                dt * 1000.0,
                stats.gpu_ms,
                stats.instances
            ));
        }
    }
}

impl<H: Host> ApplicationHandler for App<H> {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.state.is_none()
            && let Err(e) = self.init(el)
        {
            self.error = Some(e);
            el.exit();
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(size) => {
                if let Some(st) = self.state.as_mut() {
                    st.config.width = size.width.max(1);
                    st.config.height = size.height.max(1);
                    st.surface.configure(&st.renderer.gpu().device, &st.config);
                    st.renderer.resize(st.config.width, st.config.height);
                }
            }
            WindowEvent::RedrawRequested => self.frame(el),
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    let down = event.state == ElementState::Pressed;
                    if code == KeyCode::Escape && down && !event.repeat {
                        if let Some(st) = self.state.as_mut() {
                            if let Some(walk) = st.walk.as_mut().filter(|w| w.captured) {
                                walk.release(&st.window);
                            } else {
                                el.exit();
                            }
                        } else {
                            el.exit();
                        }
                    }
                    if let Some(walk) = self.state.as_mut().and_then(|s| s.walk.as_mut()) {
                        // Lighting comparison also works while the mouse is released.
                        if code == KeyCode::KeyG && down && !event.repeat {
                            walk.gi_toggle = true;
                        }
                        if down && walk.captured {
                            if !event.repeat {
                                walk.pressed.push(format!("{code:?}"));
                            }
                            walk.keys.insert(code);
                            if code == KeyCode::KeyR && !event.repeat {
                                walk.reset = true;
                            }
                        } else {
                            walk.keys.remove(&code);
                        }
                    }
                    if let Some(fly) = self.state.as_mut().and_then(|s| s.fly.as_mut()) {
                        if down {
                            fly.keys.insert(code);
                        } else {
                            fly.keys.remove(&code);
                        }
                    }
                    self.host.key(&format!("{code:?}"), down);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                // The game UI takes a left press on one of its buttons while the mouse is free:
                // it does not capture, and a release over the same button presses it.
                if button == MouseButton::Left
                    && let Some(st) = self.state.as_mut()
                {
                    if state == ElementState::Pressed {
                        st.ui_target = st.ui_under_cursor();
                        st.ui_down = st.ui_target.is_some();
                    } else {
                        st.ui_down = false;
                        if let Some(id) = st.ui_target.take() {
                            if st.ui_under_cursor() == Some(id) {
                                self.host.ui_press(id);
                            }
                            return;
                        }
                    }
                    if st.ui_target.is_some() {
                        return;
                    }
                }
                if state == ElementState::Pressed
                    && let Some(st) = self.state.as_mut()
                    && let Some(walk) = st.walk.as_mut()
                {
                    let was = walk.captured;
                    // The first click captures the mouse; later ones act.
                    if button == MouseButton::Left && !walk.captured {
                        walk.capture(&st.window);
                    }
                    if was && let Some(name) = mouse_name(button) {
                        walk.mouse.insert(name);
                        walk.pressed.push(name.to_owned());
                    }
                }
                if state == ElementState::Released
                    && let Some(walk) = self.state.as_mut().and_then(|s| s.walk.as_mut())
                    && let Some(name) = mouse_name(button)
                {
                    walk.mouse.remove(name);
                }
                if button == MouseButton::Right
                    && let Some(fly) = self.state.as_mut().and_then(|s| s.fly.as_mut())
                {
                    fly.looking = state == ElementState::Pressed;
                    fly.last_cursor = None;
                }
            }
            WindowEvent::Focused(true) => {
                if let Some(walk) = self.state.as_mut().and_then(|s| s.walk.as_mut()) {
                    walk.focused = true;
                }
            }
            WindowEvent::Focused(false) => {
                if let Some(st) = self.state.as_mut() {
                    if let Some(walk) = st.walk.as_mut() {
                        walk.release(&st.window);
                        walk.focused = false;
                    }
                    if let Some(fly) = st.fly.as_mut() {
                        fly.keys.clear();
                        fly.looking = false;
                        fly.last_cursor = None;
                    }
                }
            }
            WindowEvent::CursorLeft { .. } => {
                if let Some(st) = self.state.as_mut() {
                    st.cursor = None;
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(st) = self.state.as_mut() {
                    st.cursor = Some([position.x as f32, position.y as f32]);
                }
                if let Some(fly) = self.state.as_mut().and_then(|s| s.fly.as_mut())
                    && fly.looking
                {
                    if let Some((x, y)) = fly.last_cursor {
                        fly.yaw += ((position.x - x) * 0.003) as f32;
                        fly.pitch =
                            (fly.pitch - ((position.y - y) * 0.003) as f32).clamp(-1.55, 1.55);
                    }
                    fly.last_cursor = Some((position.x, position.y));
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if let Some(walk) = self.state.as_mut().and_then(|s| s.walk.as_mut())
                    && walk.captured
                {
                    walk.wheel += match delta {
                        MouseScrollDelta::LineDelta(_, y) => -y,
                        MouseScrollDelta::PixelDelta(p) => -(p.y as f32) / 40.0,
                    };
                    while walk.wheel >= 1.0 {
                        walk.wheel -= 1.0;
                        walk.pressed.push("WheelDown".into());
                    }
                    while walk.wheel <= -1.0 {
                        walk.wheel += 1.0;
                        walk.pressed.push("WheelUp".into());
                    }
                }
                if let Some(fly) = self.state.as_mut().and_then(|s| s.fly.as_mut()) {
                    let d = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y,
                        MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                    };
                    fly.speed = (fly.speed * 1.15f32.powf(d)).clamp(0.5, 500.0);
                }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event
            && let Some(walk) = self.state.as_mut().and_then(|s| s.walk.as_mut())
            && walk.captured
        {
            walk.look[0] += delta.0 as f32;
            walk.look[1] += delta.1 as f32;
        }
    }

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
        if let Some(st) = &self.state {
            st.window.request_redraw();
        }
    }
}

/// Opens the window and runs until it closes; a benchmark run returns its report.
pub fn run<H: Host>(host: H, options: RunOptions) -> Result<Option<BenchReport>, String> {
    if options.walk && options.fly_camera.is_some() {
        return Err("walk and fly camera controls cannot be enabled together".into());
    }
    let el = EventLoop::new().map_err(|e| e.to_string())?;
    el.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        host,
        options,
        state: None,
        report: None,
        error: None,
    };
    el.run_app(&mut app).map_err(|e| e.to_string())?;
    // A trace not yet written when the window closed (a benchmark's, or one still recording) is
    // written with the frames it has.
    if let Some(st) = app.state.as_mut() {
        st.renderer.finish_trace();
    }
    if let Some(e) = app.error {
        return Err(e);
    }
    Ok(app.report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_input_names_held_keys_and_buttons_and_consumes_presses() {
        let mut controls = WalkControls {
            keys: [KeyCode::KeyW, KeyCode::ShiftRight].into(),
            look: [10.0, -3.0],
            reset: true,
            gi_toggle: true,
            captured: true,
            mouse: ["MouseRight"].into(),
            pressed: vec!["KeyE".to_owned(), "WheelDown".to_owned()],
            ..WalkControls::default()
        };
        let pad = Pad {
            buttons: vec![1.0],
            axes: vec![0.0; 4],
        };
        let input = controls.input(1.0, Some(pad.clone()));
        let d = &input.devices;
        assert_eq!(
            d.held.keys().collect::<Vec<_>>(),
            ["KeyW", "MouseRight", "ShiftRight"]
        );
        assert_eq!(d.pressed, ["KeyE", "WheelDown"]);
        assert_eq!((d.look, d.dt_s), ([10.0, -3.0], 0.1));
        assert_eq!(d.pad.as_ref(), Some(&pad));
        assert!(input.captured && input.reset && input.gi_toggle);
        let next = controls.input(1.0 / 60.0, None);
        assert!(next.devices.pressed.is_empty() && next.devices.look == [0.0; 2]);
        assert!(!next.reset && !next.gi_toggle);
        controls.captured = false;
        assert!(
            controls.input(0.0, None).devices.held.is_empty(),
            "nothing held while released"
        );
    }
}
