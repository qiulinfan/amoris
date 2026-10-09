//! GPU regression: an unoccluded constant probe field must match the renderer's constant sky.
//! cargo run -p pocket-render --example sky_gi_units -- [--output-dir DIR]
//! A black-field negative control proves that the probe asset was actually used.

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    if let Err(error) = native::run() {
        eprintln!("sky_gi_units: {error}");
        std::process::exit(1);
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {
    eprintln!("sky_gi_units requires a native GPU");
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use glam::Vec3;
    use pocket_assets::frame::{EnvironmentView, InstanceUpdate, Look, Pose, RenderFrame};
    use pocket_assets::gi::{BAKED_GI_FORMAT, BakedGi, BakedProbe};
    use pocket_assets::mesh::ModelAsset;
    use pocket_render::{AssetSource, BackendChoice, CameraState, Gpu, Renderer};

    const SIZE: u32 = 256;
    const SKY: [f32; 3] = [0.2, 0.3, 0.4];
    const AMBIENT: f32 = 1.1;
    const CONSTANT_PATH: &str = "constant-probes.json";
    const DARK_PATH: &str = "dark-probes.json";

    struct ConstantGi {
        pending: Vec<(String, Result<BakedGi, String>)>,
        deliveries: Arc<AtomicUsize>,
    }

    impl AssetSource for ConstantGi {
        fn request(&mut self, _: &str) {}
        fn poll(&mut self) -> Vec<(String, Result<ModelAsset, String>)> {
            Vec::new()
        }
        fn request_baked_gi(&mut self, path: &str) {
            let radiance = match path {
                CONSTANT_PATH => Ok(SKY.map(|v| v * AMBIENT)),
                DARK_PATH => Ok([0.0; 3]),
                _ => Err(format!("unexpected GI request: {path}")),
            };
            self.pending
                .push((path.into(), radiance.and_then(probe_field)));
        }
        fn poll_baked_gi(&mut self) -> Vec<(String, Result<BakedGi, String>)> {
            self.deliveries
                .fetch_add(self.pending.len(), Ordering::Relaxed);
            std::mem::take(&mut self.pending)
        }
    }

    fn probe_field(radiance: [f32; 3]) -> Result<BakedGi, String> {
        let mut sh = [[0.0; 3]; 9];
        sh[0] = radiance.map(|v| v * (4.0 * std::f32::consts::PI).sqrt());
        let data = BakedGi {
            format: BAKED_GI_FORMAT.into(),
            origin: [-4.0; 3],
            spacing: [4.0; 3],
            dimensions: [3; 3],
            probes: vec![
                BakedProbe {
                    radiance_sh: sh,
                    distance_moments: vec![[100.0, 10_000.0]; 16]
                };
                27
            ],
            distance_resolution: 4,
            max_distance: 100.0,
            rays_per_probe: 1,
            bounces: 1,
            scene_signature: "sky-gi-units-constant-field".into(),
        };
        data.validate()?;
        Ok(data)
    }

    fn frame(path: &str, tick: u64) -> RenderFrame {
        RenderFrame {
            tick,
            t_s: 0.0,
            dt_s: 1.0 / 60.0,
            reset: true,
            instances: vec![InstanceUpdate {
                id: 1,
                pose: Some(Pose {
                    scale: [2.0; 3],
                    ..Pose::default()
                }),
                look: Some(Look {
                    mesh: "cube".into(),
                    material: String::new(),
                    color: [1.0; 4],
                    metallic: 0.0,
                    roughness: 1.0,
                    transmission: None,
                    ior: None,
                    emissive: [0.0; 3],
                    cast_shadows: false,
                    visible: true,
                }),
                anim: None,
            }],
            lights: Some(Vec::new()),
            cameras: Some(Vec::new()),
            environment: Some(EnvironmentView {
                sky: 1,
                sky_color: SKY,
                ambient: AMBIENT,
                baked_gi: path.into(),
                neural_gi: String::new(),
                gi_intensity: 1.0,
                fog_density: 0.0,
                fog_color: [0.0; 3],
                exposure_ev: 0.0,
                bloom: 0.0,
            }),
            ..RenderFrame::default()
        }
    }

    fn capture(renderer: &mut Renderer, path: &str, tick: u64) -> Result<Vec<u8>, String> {
        renderer.apply(frame(path, tick), 0.0);
        // poll_gi requests and receives through the same resource path used by real projects.
        let _ = renderer.capture_rgba(0.0);
        let (w, h, rgba) = renderer.capture_still(0.0);
        if let Some(error) = renderer.gi_error() {
            return Err(format!("GI load: {error}"));
        }
        if (w, h, rgba.len()) != (SIZE, SIZE, (SIZE * SIZE * 4) as usize) {
            return Err("invalid GPU capture size".into());
        }
        Ok(rgba)
    }

    fn save(dir: &Path, name: &str, rgba: &[u8]) -> Result<(), String> {
        image::save_buffer(dir.join(name), rgba, SIZE, SIZE, image::ColorType::Rgba8)
            .map_err(|e| e.to_string())
    }

    pub fn run() -> Result<(), String> {
        let mut args = std::env::args().skip(1);
        let output = match args.next().as_deref() {
            None => None,
            Some("--output-dir") => Some(PathBuf::from(
                args.next().ok_or("--output-dir requires DIR")?,
            )),
            Some("--help" | "-h") => {
                println!("usage: sky_gi_units [--output-dir DIR]");
                return Ok(());
            }
            _ => return Err("usage: sky_gi_units [--output-dir DIR]".into()),
        };
        if args.next().is_some() {
            return Err("unexpected argument".into());
        }
        let gpu = Gpu::headless(BackendChoice::from_env()).map_err(|e| e.to_string())?;
        let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let deliveries = Arc::new(AtomicUsize::new(0));
        let mut renderer = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, SIZE, SIZE);
        renderer.set_asset_source(Box::new(ConstantGi {
            pending: Vec::new(),
            deliveries: deliveries.clone(),
        }));
        let mut camera = CameraState::look_at(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO);
        camera.ortho_height = Some(3.0);
        renderer.set_camera_override(Some(camera));
        let sky = capture(&mut renderer, "", 1)?;
        let baked = capture(&mut renderer, CONSTANT_PATH, 2)?;
        let dark = capture(&mut renderer, DARK_PATH, 3)?;
        if let Some(error) = pollster::block_on(scope.pop()) {
            return Err(format!("GPU validation: {error}"));
        }
        // A 96x96 region lies well inside the cube face, excluding MSAA edge coverage.
        let (mut max_difference, mut sum_difference, mut negative_difference) = (0u32, 0u64, 0u64);
        let mut sky_sum = [0u64; 3];
        let mut baked_sum = [0u64; 3];
        let mut dark_sum = [0u64; 3];
        for y in 80..176 {
            for x in 80..176 {
                let at = ((y * SIZE + x) * 4) as usize;
                for c in 0..3 {
                    let difference = sky[at + c].abs_diff(baked[at + c]);
                    max_difference = max_difference.max(u32::from(difference));
                    sum_difference += u64::from(difference);
                    negative_difference += u64::from(baked[at + c].abs_diff(dark[at + c]));
                    sky_sum[c] += u64::from(sky[at + c]);
                    baked_sum[c] += u64::from(baked[at + c]);
                    dark_sum[c] += u64::from(dark[at + c]);
                }
            }
        }
        let pixels = 96u64 * 96;
        let mean_difference = sum_difference as f64 / (pixels * 3) as f64;
        let negative_mean_difference = negative_difference as f64 / (pixels * 3) as f64;
        let mean = |sum: [u64; 3]| sum.map(|v| v as f64 / pixels as f64);
        let delivery_count = deliveries.load(Ordering::Relaxed);
        let passed = max_difference <= 2
            && mean_difference <= 1.0
            && negative_mean_difference > 20.0
            && delivery_count == 2;
        let report = serde_json::json!({
            "passed": passed, "adapter": gpu.info.name, "backend": gpu.backend_name(),
            "gpu_validation": "passed", "comparison": "sRGB8 center object region", "region": [80, 80, 96, 96],
            "sky_radiance": SKY, "ambient": AMBIENT, "constant_field_dc": SKY.map(|v| v * AMBIENT * (4.0 * std::f32::consts::PI).sqrt()),
            "max_difference_bytes": max_difference, "mean_difference_bytes": mean_difference,
            "negative_control_mean_difference_bytes": negative_mean_difference,
            "gi_asset_deliveries": delivery_count,
            "sky_mean_rgb": mean(sky_sum), "baked_mean_rgb": mean(baked_sum), "dark_mean_rgb": mean(dark_sum),
        });
        if let Some(dir) = output {
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            save(&dir, "sky.png", &sky)?;
            save(&dir, "baked.png", &baked)?;
            save(&dir, "dark.png", &dark)?;
            std::fs::write(
                dir.join("report.json"),
                serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        }
        println!("{report}");
        if passed {
            Ok(())
        } else {
            Err("constant sky/probe units mismatch or probe negative control failed".into())
        }
    }
}
