//! A browser harness (Cargo.toml says how to run it): the engine's renderer on a canvas drawing a
//! random splat cloud with either rasterizer, so the tile rasterizer can be checked end to end in
//! Chrome's WebGPU.

use glam::{Quat, Vec3};
use pocket_assets::frame::{EnvironmentView, Pose, RenderFrame, SplatView};
use pocket_render::splat::{RawSplat, SplatCloud, SplatRaster};
use pocket_render::{CameraState, Gpu, Renderer};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn start() {
    struct ConsoleLog;
    impl log::Log for ConsoleLog {
        fn enabled(&self, m: &log::Metadata<'_>) -> bool {
            m.level() <= log::Level::Info && !m.target().starts_with("naga")
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

/// A dome of overlapping flat splats over a disc of ground ones, random colors.
fn cloud(n: usize) -> SplatCloud {
    let mut x = 0x2545_f491u32;
    let mut f = move || {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        (x >> 8) as f32 / (1u32 << 24) as f32
    };
    let raw: Vec<RawSplat> = (0..n)
        .map(|i| {
            let a = f() * std::f32::consts::TAU;
            let (pos, normal) = if i % 2 == 0 {
                let r = 4.0 * f().sqrt();
                (Vec3::new(r * a.cos(), 0.0, r * a.sin()), Vec3::Y)
            } else {
                let z = f();
                let r = (1.0 - z * z).sqrt();
                let n = Vec3::new(r * a.cos(), z, r * a.sin());
                (n * 1.5 + Vec3::Y * 0.2, n)
            };
            let s = 0.02 + 0.06 * f();
            let q = Quat::from_rotation_arc(Vec3::Z, normal) * Quat::from_rotation_z(f() * 6.28);
            RawSplat {
                position: pos.to_array(),
                scale: [s, s * (0.3 + 0.7 * f()), s * 0.05],
                rotation: q.to_array(),
                color: [f(), 0.3 + 0.5 * f(), f()],
                opacity: 0.3 + 0.7 * f(),
            }
        })
        .collect();
    SplatCloud::from_raw(&raw, 0, &[])
}

#[wasm_bindgen]
pub struct App {
    renderer: Renderer,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
}

#[wasm_bindgen]
impl App {
    pub async fn create(canvas: web_sys::HtmlCanvasElement, count: u32) -> Result<App, JsValue> {
        let (w, h) = (canvas.width().max(1), canvas.height().max(1));
        let instance = pocket_render::gpu::instance(pocket_render::BackendChoice::Auto);
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
            .map_err(|e| JsValue::from_str(&format!("no WebGPU surface: {e}")))?;
        let gpu = Gpu::new(instance, Some(&surface))
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let caps = surface.get_capabilities(&gpu.adapter);
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
        renderer.splats.insert("c", &cloud(count as usize));
        renderer.apply(
            RenderFrame {
                tick: 1,
                reset: true,
                splats: Some(vec![SplatView {
                    id: 9,
                    asset: "c".into(),
                    pose: Pose::default(),
                    visible: true,
                }]),
                environment: Some(EnvironmentView {
                    sky: 0,
                    sky_color: [0.3, 0.5, 0.8],
                    ambient: 1.0,
                    baked_gi: String::new(),
                    neural_gi: String::new(),
                    gi_intensity: 1.0,
                    fog_density: 0.0,
                    fog_color: [0.6, 0.7, 0.8],
                    exposure_ev: 0.0,
                    bloom: 0.0,
                }),
                ..RenderFrame::default()
            },
            0.0,
        );
        renderer.set_camera_override(Some(CameraState::look_at(
            Vec3::new(3.0, 2.2, 4.5),
            Vec3::new(0.0, 0.5, 0.0),
        )));
        Ok(App {
            renderer,
            surface,
            config,
        })
    }

    pub fn set_tiles(&mut self, tiles: bool) {
        self.renderer.splats.raster = if tiles {
            SplatRaster::Tiles
        } else {
            SplatRaster::Quads
        };
    }

    /// Draws a frame; returns the pass timings and the splat stats.
    pub fn render(&mut self, now_ms: f64) -> String {
        let tex = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            _ => {
                self.surface
                    .configure(&self.renderer.gpu().device, &self.config);
                return "no surface texture".into();
            }
        };
        let view = tex.texture.create_view(&Default::default());
        let s = self.renderer.render(&view, now_ms / 1000.0);
        self.renderer.gpu().queue.present(tex);
        format!(
            "{} {:?} {:?}",
            s.backend, s.passes, self.renderer.splats.stats
        )
    }
}
