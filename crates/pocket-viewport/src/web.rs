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
            m.level() <= log::Level::Info
                && !m.target().starts_with("wgpu")
                && !m.target().starts_with("naga")
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
    /// A frame failed to decode (logged once).
    frame_error: bool,
}

#[wasm_bindgen]
impl Viewport {
    /// A viewport on `canvas` (its current pixel size). Fails without WebGPU. `gpu_minimal` leaves
    /// optional device features out as `POCKET_GPU_MINIMAL` does natively (e.g. `first-instance`
    /// forces the WebGPU baseline draw path).
    pub async fn create(
        canvas: web_sys::HtmlCanvasElement,
        gpu_minimal: Option<String>,
    ) -> Result<Viewport, JsValue> {
        let (w, h) = (canvas.width().max(1), canvas.height().max(1));
        let minimal = pocket_render::gpu::Minimal::parse(gpu_minimal.as_deref().unwrap_or(""));
        let instance =
            pocket_render::gpu::instance_with(pocket_render::BackendChoice::Auto, minimal);
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
            .map_err(|e| JsValue::from_str(&format!("no WebGPU surface: {e}")))?;
        let gpu = Gpu::new_with(instance, Some(&surface), minimal)
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
            frame_error: false,
        })
    }

    /// Applies one frame of the render feed (the bytes of a `/render` message).
    pub fn push_frame(&mut self, bytes: &[u8], now_ms: f64) -> bool {
        match RenderFrame::decode(bytes) {
            Ok(f) => {
                self.renderer.apply(f, now_ms / 1000.0);
                true
            }
            Err(e) => {
                if !self.frame_error {
                    self.frame_error = true;
                    log::error!("{e}");
                }
                false
            }
        }
    }

    /// Asset paths the renderer asked for since the last call; the page fetches each and calls
    /// `deliver_asset` or `asset_failed`.
    pub fn take_asset_requests(&mut self) -> Vec<String> {
        let mut paths = std::mem::take(&mut *self.assets.requests.borrow_mut());
        paths.extend(self.renderer.splats.take_requests());
        paths
    }

    pub fn deliver_asset(&mut self, path: &str, bytes: &[u8]) {
        if is_splat(path) {
            if let Err(e) = self.renderer.splats.deliver(path, bytes) {
                log::warn!("splats: {e}");
            }
        } else {
            self.assets.deliver(path, Ok(bytes));
        }
    }

    pub fn asset_failed(&mut self, path: &str, why: &str) {
        if is_splat(path) {
            log::warn!("splats: {path}: {why}");
        } else {
            self.assets.deliver(path, Err(why.to_owned()));
        }
    }

    /// Draws from this camera: position, rotation quaternion (x, y, z, w), vertical fov degrees.
    #[allow(clippy::too_many_arguments)]
    pub fn set_camera(
        &mut self,
        px: f32,
        py: f32,
        pz: f32,
        qx: f32,
        qy: f32,
        qz: f32,
        qw: f32,
        fov_deg: f32,
    ) {
        self.renderer.set_camera_override(Some(CameraState {
            position: Vec3::new(px, py, pz),
            rotation: Quat::from_xyzw(qx, qy, qz, qw).normalize(),
            fov_y: fov_deg.to_radians(),
            near: 0.1,
            exposure_ev: 0.0,
            ortho_height: None,
        }));
    }

    /// The editor's orbit camera: eye, target, up, vertical fov, near plane, and the view height
    /// in world units when orthographic (`ortho_height` <= 0 for perspective).
    #[allow(clippy::too_many_arguments)]
    pub fn set_camera_look(
        &mut self,
        ex: f32,
        ey: f32,
        ez: f32,
        tx: f32,
        ty: f32,
        tz: f32,
        ux: f32,
        uy: f32,
        uz: f32,
        fov_deg: f32,
        near: f32,
        ortho_height: f32,
    ) {
        let mut c = CameraState::look_at_up(
            Vec3::new(ex, ey, ez),
            Vec3::new(tx, ty, tz),
            Vec3::new(ux, uy, uz),
        );
        c.fov_y = fov_deg.to_radians();
        c.near = near.max(0.001);
        c.ortho_height = (ortho_height > 0.0).then_some(ortho_height);
        self.renderer.set_camera_override(Some(c));
    }

    /// The entity under a pixel of the backbuffer (device pixels), at once: [entity, x, y, z] of
    /// the first hit, or empty.
    pub fn pick_ray(&self, x: f32, y: f32) -> Vec<f64> {
        match self.renderer.pick_ray(x, y) {
            Some((e, _, p)) => vec![e as f64, f64::from(p[0]), f64::from(p[1]), f64::from(p[2])],
            None => Vec::new(),
        }
    }

    /// Asks which entity is at a pixel of the backbuffer (device pixels).
    pub fn request_pick(&mut self, x: u32, y: u32) {
        self.renderer.request_pick(x, y);
    }

    /// The last pick: -1 while not ready, 0 for no entity, else the entity id.
    pub fn take_pick(&mut self) -> f64 {
        match self.renderer.take_pick() {
            None => -1.0,
            Some(None) => 0.0,
            Some(Some(e)) => e as f64,
        }
    }

    pub fn pick_debug(&self) -> String {
        self.renderer.pick_debug()
    }

    /// Asks which entities the view shows; `take_visible` answers [id, share, id, share, ...].
    pub fn request_visible(&mut self) {
        self.renderer.request_visible();
    }

    pub fn take_visible(&mut self) -> Option<Vec<f64>> {
        self.renderer.take_visible().map(|v| {
            v.into_iter()
                .flat_map(|(e, s)| [e as f64, f64::from(s)])
                .collect()
        })
    }

    pub fn set_overlays(&mut self, grid: bool, axes: bool) {
        self.renderer.overlays.grid = grid;
        self.renderer.overlays.axes = axes;
    }

    /// Selected entities (outlined) and the hovered one (0 for none).
    pub fn set_selection(&mut self, ids: Vec<f64>, hovered: f64) {
        self.renderer.overlays.selection = ids
            .into_iter()
            .filter(|x| *x > 0.0)
            .map(|x| x as u64)
            .collect();
        self.renderer.overlays.hovered = (hovered > 0.0).then_some(hovered as u64);
    }

    /// Gizmo geometry: `lines` packs segments as [ax, ay, az, bx, by, bz, r, g, b, a, width_px]*,
    /// `triangles` packs vertices as [x, y, z, r, g, b, a]* (colours sRGB 0..1).
    pub fn set_gizmo(&mut self, lines: Vec<f32>, triangles: Vec<f32>) {
        let o = &mut self.renderer.overlays;
        o.lines = lines
            .chunks_exact(11)
            .map(|c| pocket_render::overlay::OverlayLine {
                a: [c[0], c[1], c[2]],
                b: [c[3], c[4], c[5]],
                color: [c[6], c[7], c[8], c[9]],
                width: c[10],
            })
            .collect();
        o.polys = triangles
            .chunks_exact(7)
            .map(|c| pocket_render::overlay::OverlayVertex {
                p: [c[0], c[1], c[2]],
                color: [c[3], c[4], c[5], c[6]],
            })
            .collect();
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
        self.surface
            .configure(&self.renderer.gpu().device, &self.config);
        self.renderer.resize(width, height);
    }

    /// Draws a frame; returns its stats as JSON.
    pub fn render(&mut self, now_ms: f64) -> String {
        let tex = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            _ => {
                self.surface
                    .configure(&self.renderer.gpu().device, &self.config);
                return "{}".into();
            }
        };
        let view = tex
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let s = self.renderer.render(&view, now_ms / 1000.0);
        self.renderer.gpu().queue.present(tex);
        let passes: Vec<String> = s
            .passes
            .iter()
            .map(|(l, ms)| format!("{{\"pass\":\"{l}\",\"ms\":{ms:.3}}}"))
            .collect();
        format!(
            "{{\"gpu_ms\":{:.3},\"instances\":{},\"entities\":{},\"meshes\":{},\"pending_assets\":{},\"tick\":{},\"backend\":\"{}\",\"draw_path\":\"{}\",\"draw_calls\":{},\"passes\":[{}]}}",
            s.gpu_ms,
            s.instances,
            s.entities,
            s.meshes,
            s.pending_assets,
            s.tick,
            s.backend,
            s.draw_path,
            s.draw_calls,
            passes.join(",")
        )
    }

    /// Loads the mixed scene (every pipeline variant in every view; the draw paths' check).
    pub fn demo_mixed(&mut self, n: u32, now_ms: f64) {
        self.renderer.add_model(
            pocket_render::demo::MIXED_MODEL,
            &pocket_render::demo::mixed_model(),
        );
        self.renderer
            .apply(pocket_render::demo::mixed(n), now_ms / 1000.0);
        self.renderer
            .set_camera_override(Some(pocket_render::demo::mixed_camera(n)));
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

/// Gaussian splat clouds go to the splat renderer, everything else to the model importer.
fn is_splat(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.ends_with(".ply") || p.ends_with(".splat")
}
