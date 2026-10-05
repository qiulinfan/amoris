//! A native window that draws a [`Renderer`] (winit): the `pocket` binary's game window, the
//! examples and the benchmarks. A [`Host`] feeds the renderer each frame. Optional fly camera
//! (right mouse to look, WASD/QE to move, wheel for speed) and a benchmark mode that measures a
//! fixed number of frames and reports.

use std::collections::HashSet;
use std::sync::Arc;

use glam::{Quat, Vec3};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::camera::CameraState;
use crate::gpu::{BackendChoice, Gpu, instance};
use crate::renderer::{FrameStats, Renderer, web_time};

/// What drives the window's renderer.
pub trait Host {
    /// Before each frame: apply the render feed, move cameras.
    fn update(&mut self, renderer: &mut Renderer, now_s: f64);
    /// A key went down or up (the key's winit name, e.g. `KeyW`).
    fn key(&mut self, _key: &str, _pressed: bool) {}
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

#[derive(Clone, Debug)]
pub struct RunOptions {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub backend: BackendChoice,
    pub vsync: bool,
    /// Start with this free camera (the host may still override it).
    pub fly_camera: Option<CameraState>,
    /// Benchmark: frames to skip, then frames to measure; the window closes after.
    pub bench: Option<(u32, u32)>,
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions {
            title: "Amoris".into(),
            width: 1280,
            height: 720,
            backend: BackendChoice::from_env(),
            vsync: true,
            fly_camera: None,
            bench: None,
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
    pub gpu_ms_mean: f64,
    pub cpu_ms_mean: f64,
    pub passes: Vec<(String, f64)>,
    pub instances: usize,
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
    last_frame: f64,
    frame_times: Vec<f64>,
    gpu_times: Vec<f64>,
    cpu_times: Vec<f64>,
    frames: u32,
    title_at: f64,
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
            last_frame: web_time(),
            frame_times: Vec::new(),
            gpu_times: Vec::new(),
            cpu_times: Vec::new(),
            frames: 0,
            title_at: 0.0,
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
        if let Some(fly) = st.fly.as_mut() {
            fly.step(dt.min(0.1));
            st.renderer.set_camera_override(Some(fly.cam));
        }
        self.host.update(&mut st.renderer, now);
        let tex = match st.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                st.surface.configure(&st.renderer.gpu().device, &st.config);
                return;
            }
            _ => return,
        };
        let view = tex
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let stats = st.renderer.render(&view, now);
        st.window.pre_present_notify();
        st.renderer.gpu().queue.present(tex);
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
                    gpu_ms_mean: mean(&st.gpu_times),
                    cpu_ms_mean: mean(&st.cpu_times),
                    passes: stats
                        .passes
                        .iter()
                        .map(|(l, m)| ((*l).to_owned(), f64::from(*m)))
                        .collect(),
                    instances: stats.instances,
                });
                el.exit();
            }
        }
        if now - st.title_at > 0.5 {
            st.title_at = now;
            st.window.set_title(&format!(
                "{} — {} {}x{} — {:.2} ms frame, {:.2} ms GPU, {} instances",
                self.options.title,
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
        if self.state.is_none() {
            if let Err(e) = self.init(el) {
                self.error = Some(e);
                el.exit();
            }
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
                    if code == KeyCode::Escape && down {
                        el.exit();
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
                if button == MouseButton::Right {
                    if let Some(fly) = self.state.as_mut().and_then(|s| s.fly.as_mut()) {
                        fly.looking = state == ElementState::Pressed;
                        fly.last_cursor = None;
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(fly) = self.state.as_mut().and_then(|s| s.fly.as_mut()) {
                    if fly.looking {
                        if let Some((x, y)) = fly.last_cursor {
                            fly.yaw += ((position.x - x) * 0.003) as f32;
                            fly.pitch =
                                (fly.pitch - ((position.y - y) * 0.003) as f32).clamp(-1.55, 1.55);
                        }
                        fly.last_cursor = Some((position.x, position.y));
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
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

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
        if let Some(st) = &self.state {
            st.window.request_redraw();
        }
    }
}

/// Opens the window and runs until it closes; a benchmark run returns its report.
pub fn run<H: Host>(host: H, options: RunOptions) -> Result<Option<BenchReport>, String> {
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
    if let Some(e) = app.error {
        return Err(e);
    }
    Ok(app.report)
}
