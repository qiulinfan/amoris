//! The wasm-bindgen surface of the viewport.

use glam::{Quat, Vec3};
use pocket_assets::RenderFrame;
use pocket_render::{CameraState, Gpu, Renderer};
use wasm_bindgen::prelude::*;

use crate::PageAssets;

#[wasm_bindgen(start)]
pub fn start() {
    struct ConsoleLog;
    impl log::Log for ConsoleLog {
        fn enabled(&self, m: &log::Metadata<'_>) -> bool {
            m.level() <= log::Level::Info && !m.target().starts_with("wgpu") && !m.target().starts_with("naga")
        }
        fn log(&self, r: &log::Record<'_>) {
            if self.enabled(r.metadata()) {
                web_sys::console::log_1(&format!("[pocket {}] {}", r.level(), r.args()).into());
            }
        }
        fn flush(&self) {}
    }
    static LOGGER: ConsoleLog = ConsoleLog;
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Info);
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&format!("pocket panicked: {info}").into());
    }));
}

/// The renderer drawing into a canvas.
#[wasm_bindgen]
pub struct Viewport {
    renderer: Renderer,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    assets: PageAssets,
}

#[wasm_bindgen]
impl Viewport {
    /// A viewport on `canvas` (its current pixel size). Fails without WebGPU.
    pub async fn create(canvas: web_sys::HtmlCanvasElement) -> Result<Viewport, JsValue> {
        let (w, h) = (canvas.width().max(1), canvas.height().max(1));
        let instance = pocket_render::gpu::instance(pocket_render::BackendChoice::Auto);
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
            .map_err(|e| JsValue::from_str(&format!("no WebGPU surface: {e}")))?;
        let gpu = Gpu::new(instance, Some(&surface))
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let caps = surface.get_capabilities(&gpu.adapter);
        // Browsers offer bgra8unorm or rgba8unorm; the display transform encodes sRGB itself.
        let format = caps.formats[0];
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: w,
            height: h,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&gpu.device, &config);
        let mut renderer = Renderer::new(&gpu, format, w, h);
        let assets = PageAssets::default();
        renderer.set_asset_source(Box::new(assets.clone()));
        Ok(Viewport {
            renderer,
            surface,
            config,
            assets,
        })
    }

    /// Applies one frame of the render feed (the bytes of a `/render` message).
    pub fn push_frame(&mut self, bytes: &[u8], now_ms: f64) -> bool {
        match RenderFrame::decode(bytes) {
            Some(f) => {
                self.renderer.apply(f, now_ms / 1000.0);
                true
            }
            None => false,
        }
    }

    /// Asset paths the renderer asked for since the last call; the page fetches each and calls
    /// `deliver_asset` or `asset_failed`.
    pub fn take_asset_requests(&mut self) -> Vec<String> {
        std::mem::take(&mut *self.assets.requests.borrow_mut())
    }

    pub fn deliver_asset(&mut self, path: &str, bytes: &[u8]) {
        self.assets.deliver(path, Ok(bytes));
    }

    pub fn asset_failed(&mut self, path: &str, why: &str) {
        self.assets.deliver(path, Err(why.to_owned()));
    }

    /// Draws from this camera: position, rotation quaternion (x, y, z, w), vertical fov degrees.
    #[allow(clippy::too_many_arguments)]
    pub fn set_camera(&mut self, px: f32, py: f32, pz: f32, qx: f32, qy: f32, qz: f32, qw: f32, fov_deg: f32) {
        self.renderer.set_camera_override(Some(CameraState {
            position: Vec3::new(px, py, pz),
            rotation: Quat::from_xyzw(qx, qy, qz, qw).normalize(),
            fov_y: fov_deg.to_radians(),
            near: 0.1,
            exposure_ev: 0.0,
        }));
    }

    /// Draws from the scene's active camera again.
    pub fn use_scene_camera(&mut self) {
        self.renderer.set_camera_override(None);
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.renderer.gpu().device, &self.config);
        self.renderer.resize(width, height);
    }

    /// Draws a frame; returns its stats as JSON.
    pub fn render(&mut self, now_ms: f64) -> String {
        let tex = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            _ => {
                self.surface.configure(&self.renderer.gpu().device, &self.config);
                return "{}".into();
            }
        };
        let view = tex.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let s = self.renderer.render(&view, now_ms / 1000.0);
        self.renderer.gpu().queue.present(tex);
        let passes: Vec<String> = s
            .passes
            .iter()
            .map(|(l, ms)| format!("{{\"pass\":\"{l}\",\"ms\":{ms:.3}}}"))
            .collect();
        format!(
            "{{\"gpu_ms\":{:.3},\"instances\":{},\"entities\":{},\"meshes\":{},\"pending_assets\":{},\"tick\":{},\"backend\":\"{}\",\"passes\":[{}]}}",
            s.gpu_ms,
            s.instances,
            s.entities,
            s.meshes,
            s.pending_assets,
            s.tick,
            s.backend,
            passes.join(",")
        )
    }

    /// Loads the many_cubes benchmark scene (Bevy's layouts) for in-browser measurement.
    pub fn demo_cubes(&mut self, count: u32, dense: bool, now_ms: f64) {
        let f = pocket_render::demo::many_cubes(count as usize, dense, false);
        self.renderer.apply(f, now_ms / 1000.0);
    }

    /// Points the camera as many_cubes does at frame `frame`.
    pub fn demo_cubes_camera(&mut self, frame: u32, dense: bool) {
        self.renderer
            .set_camera_override(Some(pocket_render::demo::cubes_camera(frame, dense)));
    }

    /// The camera the next frame draws from: [px, py, pz, qx, qy, qz, qw, fov_deg].
    pub fn camera(&self) -> Vec<f32> {
        let c = self.renderer.camera();
        vec![
            c.position.x,
            c.position.y,
            c.position.z,
            c.rotation.x,
            c.rotation.y,
            c.rotation.z,
            c.rotation.w,
            c.fov_y.to_degrees(),
        ]
    }
}
