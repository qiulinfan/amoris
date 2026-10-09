//! The wasm-bindgen surface of the viewport.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

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
    /// Frames submitted and not yet done on the GPU (counted only under a limit), and the most
    /// allowed (0: no limit, the default; `set_max_frames_in_flight`).
    in_flight: Arc<AtomicU32>,
    max_in_flight: u32,
    /// The last frame's stats, which a skipped call answers again (marked `skipped`).
    last_stats: String,
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
            in_flight: Arc::new(AtomicU32::new(0)),
            max_in_flight: 0,
            last_stats: "{}".into(),
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
            .as_chunks::<11>()
            .0
            .iter()
            .map(|&[ax, ay, az, bx, by, bz, r, g, b, a, width]| {
                pocket_render::overlay::OverlayLine {
                    a: [ax, ay, az],
                    b: [bx, by, bz],
                    color: [r, g, b, a],
                    width,
                }
            })
            .collect();
        o.polys = triangles
            .as_chunks::<7>()
            .0
            .iter()
            .map(
                |&[x, y, z, r, g, b, a]| pocket_render::overlay::OverlayVertex {
                    p: [x, y, z],
                    color: [r, g, b, a],
                },
            )
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

    /// How many frames the page may have on the GPU at once; 0 (the default) for no limit
    /// (docs/bench/polish.md 3). A page whose frame loop is not held to the display (Chrome
    /// without vsync or a frame-rate limit, as the benchmarks run it) otherwise submits 300 to 370
    /// frames ahead of Chrome's GPU process, and every readback (the entity-id pass, the GPU
    /// timestamps, occlusion culling's counters) waits behind them, up to seconds; held to the
    /// display it answers in two or three frames, as natively, where the swap chain bounds the same
    /// queue. With a limit of 2 to 4 an uncapped page's readbacks answered in 3 to 7 frames, but it
    /// drew 3 to 10 times fewer frames, and in some runs nearly none for seconds (Chrome's
    /// `onSubmittedWorkDone`, which counts the frames done, then answered after hundreds of
    /// milliseconds), so no limit is the default.
    pub fn set_max_frames_in_flight(&mut self, n: u32) {
        self.max_in_flight = n;
    }

    /// Draws a frame; returns its stats as JSON. Under a limit (`set_max_frames_in_flight`), while
    /// that many frames are on the GPU it draws nothing and answers the last frame's stats with
    /// `"skipped": true`, so the page never runs further ahead of the GPU than that.
    pub fn render(&mut self, now_ms: f64) -> String {
        if self.max_in_flight > 0 && self.in_flight.load(Ordering::Acquire) >= self.max_in_flight {
            return skipped(&self.last_stats);
        }
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
        let gpu = self.renderer.gpu();
        gpu.queue.present(tex);
        if self.max_in_flight > 0 {
            self.in_flight.fetch_add(1, Ordering::AcqRel);
            let in_flight = self.in_flight.clone();
            gpu.queue.on_submitted_work_done(move || {
                in_flight.fetch_sub(1, Ordering::AcqRel);
            });
        }
        let passes: Vec<String> = s
            .passes
            .iter()
            .map(|(l, ms)| format!("{{\"pass\":\"{l}\",\"ms\":{ms:.3}}}"))
            .collect();
        self.last_stats = format!(
            "{{\"gpu_ms\":{:.3},\"instances\":{},\"entities\":{},\"meshes\":{},\"pending_assets\":{},\"tick\":{},\"backend\":\"{}\",\"draw_path\":\"{}\",\"draw_calls\":{},\"occlusion\":\"{}\",\"occluded\":{},\"lod\":\"{}\",\"passes\":[{}]}}",
            s.gpu_ms,
            s.instances,
            s.entities,
            s.meshes,
            s.pending_assets,
            s.tick,
            s.backend,
            s.draw_path,
            s.draw_calls,
            s.occlusion,
            s.occlusion_stats.map_or(0, |o| o.occluded),
            s.lod,
            passes.join(",")
        );
        self.last_stats.clone()
    }

    /// Levels of detail: `off` draws every instance's full mesh, `on` (the default) picks levels
    /// per instance and view (docs/spec/lod.md); anything else leaves it as it is.
    pub fn set_lod(&mut self, mode: &str) {
        if let Some(m) = pocket_render::LodMode::parse(mode) {
            self.renderer.set_lod(m);
        }
    }

    /// Loads the levels-of-detail benchmark's field (`n` x `n` dense rocks and knots `spacing`
    /// metres apart, meshes of subdivision `detail`), its levels made here as the importer makes
    /// them, with its camera at `t`.
    pub fn demo_lod(&mut self, n: u32, spacing: f32, detail: u32, t: f32, now_ms: f64) {
        self.renderer.add_model(
            pocket_render::demo::LOD_MODEL,
            &pocket_render::demo::lod_model(detail),
        );
        self.renderer
            .apply(pocket_render::demo::lod_field(n, spacing), now_ms / 1000.0);
        self.demo_lod_camera(n, spacing, t);
    }

    /// The levels-of-detail field's camera at `t` (0: in the first cell, 1: near the middle).
    pub fn demo_lod_camera(&mut self, n: u32, spacing: f32, t: f32) {
        self.renderer
            .set_camera_override(Some(pocket_render::demo::lod_field_camera(n, spacing, t)));
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

    /// Occlusion culling of the camera view: `off`, `on` or `auto` (the default; anything else
    /// leaves it as it is).
    pub fn set_occlusion(&mut self, mode: &str) {
        if let Some(m) = pocket_render::OcclusionMode::parse(mode) {
            self.renderer.set_occlusion(m);
        }
    }

    /// Loads the occlusion culling check's scene (a wall with a slit, an alpha-masked occluder, a
    /// grid of primitives behind them) with its front camera.
    pub fn demo_occluders(&mut self, now_ms: f64) {
        self.renderer.add_model(
            pocket_render::demo::MIXED_MODEL,
            &pocket_render::demo::mixed_model(),
        );
        self.renderer
            .apply(pocket_render::demo::occluders(3.0), now_ms / 1000.0);
        let [front, _] = pocket_render::demo::occluders_cameras();
        self.renderer.set_camera_override(Some(front));
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

/// `stats` (a JSON object) with `"skipped": true`: a call that drew nothing.
fn skipped(stats: &str) -> String {
    match stats.strip_suffix('}') {
        Some(head) if head.len() > 1 => format!("{head},\"skipped\":true}}"),
        _ => "{\"skipped\":true}".into(),
    }
}

/// Gaussian splat clouds go to the splat renderer, everything else to the model importer.
fn is_splat(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.ends_with(".ply") || p.ends_with(".splat")
}
