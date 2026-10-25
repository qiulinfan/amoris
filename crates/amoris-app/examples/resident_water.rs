//! Same prefilled dam and optional source on all three actual game backends. Includes the host
//! bridge, GPU digest and periodic checkpoints. Raw results belong in ignored out/ storage.
//! cargo run --release -p amoris-app --example resident_water -- --ticks 256 --probes 64
#[path = "../src/water_compute.rs"]
mod water_compute;
use amoris_runtime::{Extractor, GameSetup, Project};
use amoris_sim::{StepHooks, SystemKey, TickPhase};
use serde_json::json;
use std::{sync::Arc, time::Instant};
#[derive(Default)]
struct Timer {
    start: Option<Instant>,
    water_ms: f64,
    buoyancy_ms: f64,
    rigid_ms: f64,
}
impl StepHooks for Timer {
    fn system(&mut self, key: &SystemKey, begin: bool) {
        if !matches!(
            key.as_str(),
            "physics.water" | "physics.buoyancy" | "physics.step"
        ) {
            return;
        }
        if begin {
            self.start = Some(Instant::now());
        } else {
            let ms = self.start.take().unwrap().elapsed().as_secs_f64() * 1e3;
            match key.as_str() {
                "physics.water" => self.water_ms = ms,
                "physics.buoyancy" => self.buoyancy_ms = ms,
                _ => self.rigid_ms = ms,
            }
        }
    }
    fn phase(&mut self, _: TickPhase, _: bool) {}
}
fn setup(side: u32, mode: &str, probes: u32, source: bool, bodies: u32) -> Arc<GameSetup> {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/water-resident");
    let mut setup = Project::load(&root).unwrap().setup(false).unwrap();
    setup.scene.entities.truncate(2);
    let r = &mut setup.scene.entities[1].components["WaterRegion"];
    r["solver"] = json!(mode);
    r["size"] = json!([side, side]);
    r["tiles"] = json!(
        (0..side / 16)
            .flat_map(|z| {
                (0..side/16).map(move |x| {
        let depth: Vec<_> = (0..256).map(|n| if x*16+n%16<side/2 {2.0} else {0.0}).collect();
        json!({"key":[x,z],"bed":vec![0.0;256],"depth":depth,"u":vec![0.0;256],"w":vec![0.0;256]})
    })
            })
            .collect::<Vec<_>>()
    );
    if mode == "gpu_resident" {
        for n in 0..probes {
            setup.scene.entities.push(
                serde_json::from_value(json!({"name":format!("Probe{n}"),"components":{
                    "WaterProbe":{"region":2,"at":[f64::from(n%8)*f64::from(side)/8.0+0.5,
                        f64::from(n/8)*f64::from(side)/8.0+0.5]}
                }}))
                .unwrap(),
            );
        }
    }
    if source {
        setup.scene.entities.push(serde_json::from_value(json!({"name":"Spring","components":{
        "WaterSource":{"region":2,"at":[f64::from(side)*0.25+0.5,f64::from(side)*0.5+0.5],"rate_m3s":1.0}
    }})).unwrap());
    }
    if bodies > 0 {
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/water-floaters");
        let authored = Project::load(&root).unwrap().setup(false).unwrap();
        let template = &authored.scene.entities[2];
        for n in 0..bodies {
            let mut body = template.clone();
            body.name = Some(format!("Float{n}"));
            body.components["Transform"]["position"] = json!([
                f64::from(side) * 0.25 + f64::from(n % 16),
                2.7,
                f64::from(side) * 0.5 + f64::from(n / 16)
            ]);
            setup.scene.entities.push(body);
        }
    }
    Arc::new(setup)
}
fn percentile(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p) as usize]
}

struct DrawBench {
    gpu: amoris_render::Gpu,
    renderer: amoris_render::Renderer,
    target: wgpu::TextureView,
    phases: [f64; 3],
}
impl DrawBench {
    fn new(root: &std::path::Path, lod: amoris_render::LodMode) -> Self {
        let gpu = amoris_render::Gpu::headless(amoris_render::host_backend()).unwrap();
        gpu.device.on_uncaptured_error(std::sync::Arc::new(|e| {
            panic!("resident draw benchmark GPU validation: {e}")
        }));
        let mut renderer =
            amoris_render::Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, 1280, 720);
        renderer.set_asset_source(Box::new(amoris_render::loader::FileAssets::new(
            root.to_path_buf(),
        )));
        renderer.set_antialiasing(amoris_render::Antialiasing::Msaa);
        renderer.set_gtao(amoris_render::Gtao::Off);
        renderer.set_prepass(amoris_render::PrepassMode::Off);
        renderer.set_occlusion(amoris_render::OcclusionMode::Off);
        renderer.set_water_culling(true);
        renderer.set_lod(lod);
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("resident combined benchmark"),
            size: wgpu::Extent3d {
                width: 1280,
                height: 720,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        Self {
            gpu,
            renderer,
            target: texture.create_view(&Default::default()),
            phases: [0.0; 3],
        }
    }
    fn draw(&mut self, frame: amoris_assets::frame::RenderFrame, t: f64) -> (f64, f64, f64) {
        let start = Instant::now();
        self.renderer.apply(frame, t);
        let applied = start.elapsed().as_secs_f64() * 1000.0;
        let stats = self.renderer.render(&self.target, t);
        let encoded = start.elapsed().as_secs_f64() * 1000.0;
        self.gpu
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(30)),
            })
            .unwrap();
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        self.phases = [applied, encoded - applied, elapsed - encoded];
        (elapsed, f64::from(stats.cpu_ms), f64::from(stats.gpu_ms))
    }
}
#[derive(Default)]
struct ProjectBench {
    warmup: u32,
    rows: Option<std::path::PathBuf>,
    wake: Option<u32>,
    snapshots: bool,
    decode_frame: bool,
}

fn bench_project(
    root: &std::path::Path,
    ticks: u32,
    feedback_off: bool,
    draw_lod: Option<amoris_render::LodMode>,
    options: ProjectBench,
) {
    let setup_start = Instant::now();
    let mut setup = Project::load(root).unwrap().setup(false).unwrap();
    let keep_history = setup.scene.entities.iter().any(|e| {
        e.components
            .get("WaterRegion")
            .and_then(|r| r.get("edges"))
            .and_then(|v| v.as_array())
            .is_some_and(|v| v.iter().any(|e| e != "Wall"))
    });
    let mut water_history = Vec::new();
    if feedback_off {
        for e in &mut setup.scene.entities {
            e.components.remove("WaterFeedback");
        }
    }
    let service = water_compute::NativeWater::default();
    let backend = service.clone();
    let mut game = amoris_runtime::GameBuilder::new(Arc::new(setup))
        .seed(1)
        .system(move |sim| {
            amoris_runtime::water_compute::install(sim, backend.clone());
            Ok(())
        })
        .build()
        .unwrap();
    let mut draw = draw_lod.map(|lod| DrawBench::new(root, lod));
    let setup_ms = setup_start.elapsed().as_secs_f64() * 1e3;
    let mut warmup_ticks = Vec::new();
    let feed = amoris_assets::Feed::new();
    let mailbox = feed.subscribe();
    let mut extractor = Extractor::default();
    let mut timer = Timer::default();
    let (mut water, mut buoyancy, mut rigid, mut step, mut publish) =
        (vec![], vec![], vec![], vec![], vec![]);
    let (mut bytes, mut maximum, mut checkpoint_max) = (0usize, 0usize, 0.0f64);
    let (mut complete, mut draw_wait, mut draw_cpu, mut draw_gpu) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut rows = Vec::new();
    let mut snapshot_cache = amoris_runtime::SnapshotCache::new();
    let mut snapshot_checkpoint_bytes = None;
    let mut _last_snapshot = None;
    for tick in 1..=ticks + options.warmup {
        if options.wake == Some(tick) {
            game.apply(&amoris_runtime::Command::new(amoris_link::Source::Developer(0),u64::from(tick),"world.edit",json!({"label":"wake pipeline fixture", "ops":[{"set":{"entity":"Lake0","component":"WaterRegion","value":{"edges":[{"Inflow":{"m3s_per_m":0.1}},"Wall","Wall","Wall"]}}}]}))).unwrap();
        }
        let iteration = Instant::now();
        let start = Instant::now();
        let report = game.step_with(&mut timer).unwrap();
        assert!(
            report.errors.is_empty(),
            "project benchmark tick failed: {:?}",
            report.errors
        );
        let elapsed = start.elapsed().as_secs_f64() * 1e3;
        let start = Instant::now();
        game.present(&mut extractor, &feed);
        let frame = mailbox.take();
        let extracted = start.elapsed().as_secs_f64() * 1000.0;
        let checkpoint_count = frame
            .resident_water
            .iter()
            .filter(|r| r.checkpoint.is_some())
            .count();
        let snapshot_start = Instant::now();
        let (snapshot_ms, snapshot_bytes, snapshot_changed) = if options.snapshots {
            let snapshot = snapshot_cache
                .snapshot(game.world(), game.registry())
                .unwrap();
            let section = snapshot
                .sections()
                .iter()
                .find(|s| s.key.name == "WaterCheckpoints");
            let bytes = section.map_or(0, |s| s.bytes.len());
            let changed = section.is_some_and(|s| {
                snapshot_checkpoint_bytes
                    .as_ref()
                    .is_none_or(|old| !Arc::ptr_eq(old, &s.bytes))
            });
            snapshot_checkpoint_bytes = section.map(|s| s.bytes.clone());
            _last_snapshot = Some(snapshot);
            (
                snapshot_start.elapsed().as_secs_f64() * 1000.0,
                bytes,
                changed,
            )
        } else {
            (0.0, 0, false)
        };
        let encode_start = Instant::now();
        let encoded_frame = frame.encode();
        let len = encoded_frame.len();
        let encoded = encode_start.elapsed().as_secs_f64() * 1000.0;
        let decode_start = Instant::now();
        let (frame, decoded) = if options.decode_frame {
            let received = amoris_assets::frame::RenderFrame::decode(&encoded_frame).unwrap();
            assert_eq!(received.tick, frame.tick);
            (received, decode_start.elapsed().as_secs_f64() * 1000.0)
        } else {
            (frame, 0.0)
        };
        let sent = extracted + encoded;
        let drawn = draw.as_mut().map(|d| d.draw(frame, f64::from(tick) / 60.0));
        let total = iteration.elapsed().as_secs_f64() * 1000.0;
        if options.rows.is_some() {
            let counters = service.resident_diagnostics().unwrap();
            rows.push(json!({"tick":tick,"water_ms":timer.water_ms,"step_ms":elapsed,"extract_ms":extracted,"encode_ms":encoded,"decode_ms":decoded,"snapshot_ms":snapshot_ms,"snapshot_checkpoint_bytes":snapshot_bytes,"snapshot_checkpoint_changed":snapshot_changed,"publish_ms":sent,"draw":drawn,"draw_phases":draw.as_ref().map(|d|d.phases),"complete_ms":total,"feed_bytes":len,"checkpoints":checkpoint_count,"resident":{"slots":counters.slots,"evictions":counters.evictions,"reserved_columns":counters.columns,"resizes":counters.resizes,"uploads":counters.transfers.uploads,"field_readbacks":counters.transfers.field_readbacks,"small_readbacks":counters.transfers.small_readbacks,"readback_bytes":counters.transfers.readback_bytes}}));
        }
        if tick <= options.warmup {
            warmup_ticks.push(
                json!({"tick":tick,"water_ms":timer.water_ms,"step_ms":elapsed,
                "publish_ms":sent,"feed_bytes":len,"draw":drawn,"complete_ms":total}),
            );
        }
        if tick > options.warmup {
            water.push(timer.water_ms);
            buoyancy.push(timer.buoyancy_ms);
            rigid.push(timer.rigid_ms);
            step.push(elapsed);
            publish.push(sent);
            complete.push(total);
            if let Some((wait, cpu, gpu)) = drawn {
                draw_wait.push(wait);
                draw_cpu.push(cpu);
                draw_gpu.push(gpu);
            }
            bytes += len;
            maximum = maximum.max(len);
            if checkpoint_count > 0 {
                checkpoint_max = checkpoint_max.max(timer.water_ms);
            }
        }
        if keep_history && tick % 60 == 0 {
            let state=game.apply(&amoris_runtime::Command::new(amoris_link::Source::Developer(0),u64::from(tick),"world.query",json!({"with":["WaterResident"],"fields":["WaterResident.boundary","WaterResident.stats"]}))).unwrap();
            water_history.push(json!({"tick":tick,"state":state}));
        }
    }
    if let Some(path) = &options.rows {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, serde_json::to_vec(&rows).unwrap()).unwrap();
    }
    let execution = game
        .apply(&amoris_runtime::Command::new(
            amoris_link::Source::Developer(0),
            4,
            "world.query",
            json!({"with":["WaterResident"],"fields":["WaterResident.recipe.environment"]}),
        ))
        .unwrap();
    let diagnostics = service.resident_diagnostics().unwrap();
    let resident_state=game.apply(&amoris_runtime::Command::new(amoris_link::Source::Developer(0),3,"world.query",json!({"with":["WaterResident"],"fields":["WaterResident.still","WaterResident.completed_tick","WaterResident.recipe.digest"]}))).unwrap();
    let metric = |mut samples: Vec<f64>| json!({"median_ms":percentile(&mut samples,0.5),"p95_ms":percentile(&mut samples,0.95)});
    let reaction=game.apply(&amoris_runtime::Command::new(amoris_link::Source::Developer(0),1,"world.query",json!({
        "with":["WaterFeedback"],"fields":["WaterFeedback.emitted_total","WaterFeedback.applied_total","WaterFeedback.rejected_total","WaterFeedback.residual_total"]
    }))).unwrap();
    let water_state=game.apply(&amoris_runtime::Command::new(amoris_link::Source::Developer(0),2,"world.query",json!({
        "with":["WaterResident"],"fields":["WaterResident.stats","WaterResident.boundary","WaterResident.completed_tick","WaterResident.controls"]
    }))).unwrap();
    let rendering=draw.as_ref().map(|d|json!({"lod":format!("{:?}",draw_lod.unwrap()),"render_and_wait":metric(draw_wait),"cpu":metric(draw_cpu),"timestamped_gpu_sum":if d.gpu.caps.timestamps{Some(metric(draw_gpu))}else{None},"mesh":d.renderer.water_mesh_stats().unwrap(),"ledges":d.renderer.water_ledge_stats().unwrap()}));
    println!(
        "{}",
        json!({"project":root,"feedback_off":feedback_off,"ticks":ticks,"warmup":options.warmup,"snapshots":options.snapshots,"decode_frame":options.decode_frame,"batch_groups":{"sync":diagnostics.batches.sync_groups,"step":diagnostics.batches.step_groups,"checkpoint":diagnostics.batches.checkpoint_groups,"max_regions":diagnostics.batches.max_group_regions},"execution":execution,"setup_ms":setup_ms,"resident_state":resident_state,"resident_service":{"slots":diagnostics.slots,"evictions":diagnostics.evictions,"reserved_columns":diagnostics.columns,"resizes":diagnostics.resizes,"uploads":diagnostics.transfers.uploads,"field_readbacks":diagnostics.transfers.field_readbacks,"small_readbacks":diagnostics.transfers.small_readbacks,"readback_bytes":diagnostics.transfers.readback_bytes},"warmup_ticks":warmup_ticks,
        "water":metric(water),"buoyancy":metric(buoyancy),"rigid":metric(rigid),"step":metric(step),"publish":metric(publish),
        "checkpoint_max_ms":checkpoint_max,"feed_mean_bytes":bytes as f64/f64::from(ticks),"feed_max_bytes":maximum,"reaction":reaction,"water_state":water_state,"water_history":water_history,"complete_tick_publish_draw_wait":metric(complete),"rendering":rendering,"world_hash":game.world_hash().unwrap().to_string()})
    );
}
struct CaptureOptions {
    motion: bool,
    ticks: u32,
    splashes: bool,
    aa: amoris_render::Antialiasing,
    camera: Option<[f32; 6]>,
    decode_frame: bool,
}
fn capture(out: &std::path::Path, project: Option<&std::path::Path>, options: CaptureOptions) {
    let CaptureOptions {
        motion,
        ticks,
        splashes,
        aa,
        camera,
        decode_frame,
    } = options;
    use amoris_render::{Gpu, Gtao, Renderer};
    std::fs::create_dir_all(out).unwrap();
    if motion {
        std::fs::create_dir_all(out.join("motion")).unwrap();
    }
    let root = project
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/water-resident")
        });
    let setup = Arc::new(Project::load(&root).unwrap().setup(false).unwrap());
    let mut game = water_compute::builder(setup, 1).build().unwrap();
    let feed = amoris_assets::Feed::new();
    let mailbox = feed.subscribe();
    let mut extractor = Extractor::default();
    let gpu = Gpu::headless(amoris_render::host_backend()).unwrap();
    let mut renderer = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, 1280, 720);
    renderer.set_asset_source(Box::new(amoris_render::loader::FileAssets::new(
        root.clone(),
    )));
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("resident game capture"),
        size: wgpu::Extent3d {
            width: 1280,
            height: 720,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let target = target.create_view(&Default::default());
    renderer.set_antialiasing(aa);
    renderer.set_gtao(Gtao::Off);
    renderer.set_splashes(splashes);
    if let Some(c) = camera {
        renderer.set_camera_override(Some(amoris_render::CameraState::look_at(
            glam::Vec3::from_array([c[0], c[1], c[2]]),
            glam::Vec3::from_array([c[3], c[4], c[5]]),
        )));
    }
    let mut spray_motion = Vec::new();
    let mut merged = amoris_assets::frame::RenderFrame::default();
    let mut body_motion = Vec::new();
    for tick in 1..=ticks {
        let result = game.step().unwrap();
        assert!(
            result.errors.is_empty(),
            "capture tick failures: {:?}",
            result.errors
        );
        game.present(&mut extractor, &feed);
        let frame = mailbox.take();
        let frame = if decode_frame {
            amoris_assets::frame::RenderFrame::decode(&frame.encode()).unwrap()
        } else {
            frame
        };
        merged.merge(frame.clone()).unwrap();
        let t = f64::from(tick) / 60.0;
        renderer.apply(frame, t);
        let key_frame = [1, 27, 63, 64, 120].contains(&tick) || tick == ticks;
        if !key_frame && !motion {
            renderer.render(&target, t);
            continue;
        }
        // Exactly one render per simulation tick; captures include the current temporal state.
        let (w, h, pixels) = renderer.capture_rgba(t);
        let rows=game.apply(&amoris_runtime::Command::new(amoris_link::Source::Developer(0),u64::from(tick),
            "world.query",json!({"with":["Floater","Transform"],"limit":u32::MAX,
                "fields":["Transform.position","Transform.rotation","Velocity.linear","Velocity.angular","Floater.submerged"]}))).unwrap();
        let bodies: Vec<_>=rows.as_array().unwrap().iter().map(|r|json!({"id":r["id"],
            "position":r["Transform.position"],"rotation":r["Transform.rotation"],
            "velocity":r["Velocity.linear"],"angular":r["Velocity.angular"],"submerged":r["Floater.submerged"]})).collect();
        if !bodies.is_empty() {
            body_motion.push(json!({"tick":tick,"bodies":bodies}));
        }
        spray_motion.push(json!({"tick":tick,"spray":renderer.water_spray_stats().unwrap(),"ledges":renderer.water_ledge_stats().unwrap()}));
        if key_frame {
            image::save_buffer(
                out.join(format!("tick-{tick}.png")),
                &pixels,
                w,
                h,
                image::ColorType::Rgba8,
            )
            .unwrap();
            std::fs::write(out.join(format!("tick-{tick}.bin")), merged.encode()).unwrap();
        }
        if motion {
            image::save_buffer(
                out.join("motion").join(format!("{tick:05}.png")),
                &pixels,
                w,
                h,
                image::ColorType::Rgba8,
            )
            .unwrap();
        }
    }
    if !body_motion.is_empty() {
        std::fs::write(
            out.join("body-motion.json"),
            serde_json::to_vec_pretty(&body_motion).unwrap(),
        )
        .unwrap();
    }
    let water=game.apply(&amoris_runtime::Command::new(amoris_link::Source::Developer(0),1,"world.query",json!({"with":["WaterResident"],"fields":["WaterResident.stats","WaterResident.boundary"]}))).unwrap();
    std::fs::write(out.join("spray.json"),serde_json::to_vec_pretty(&json!({"scope":"actual Game and current displayed-field spray, diagnostics/captures excluded from timing","decode_frame":decode_frame,"splashes":splashes,"antialiasing":aa.name(),"world_hash":game.world_hash().unwrap().to_string(),"water":water,"rows":spray_motion})).unwrap()).unwrap();
    println!("native resident game and wire frames: {}", out.display());
}

fn run() {
    let args: Vec<_> = std::env::args().collect();
    if let Some(pair) = args.windows(2).find(|a| a[0] == "--capture") {
        let project = args
            .windows(2)
            .find(|a| a[0] == "--project")
            .map(|a| std::path::Path::new(&a[1]));
        capture(
            std::path::Path::new(&pair[1]),
            project,
            CaptureOptions {
                motion: args.iter().any(|a| a == "--motion"),
                ticks: args
                    .windows(2)
                    .find(|a| a[0] == "--ticks")
                    .map(|a| a[1].parse().expect("capture ticks"))
                    .unwrap_or(120),
                splashes: !args.iter().any(|a| a == "--no-splashes"),
                aa: args
                    .windows(2)
                    .find(|a| a[0] == "--aa")
                    .map(|a| amoris_render::Antialiasing::parse(&a[1]).expect("valid --aa"))
                    .unwrap_or(amoris_render::Antialiasing::Msaa),
                camera: args.windows(2).find(|a| a[0] == "--camera-look").map(|a| {
                    a[1].split(',')
                        .map(|v| v.parse::<f32>().expect("numeric camera coordinate"))
                        .collect::<Vec<_>>()
                        .try_into()
                        .expect("six --camera-look coordinates")
                }),
                decode_frame: args.iter().any(|a| a == "--decode-frame"),
            },
        );
        return;
    }
    let number = |name, default| {
        args.windows(2)
            .find(|w| w[0] == name)
            .map(|w| w[1].parse::<u32>().unwrap())
            .unwrap_or(default)
    };
    let ticks = number("--ticks", 256);
    if let Some(pair) = args.windows(2).find(|a| a[0] == "--bench-project") {
        assert!(ticks >= 64);
        bench_project(
            std::path::Path::new(&pair[1]),
            ticks,
            args.iter().any(|a| a == "--feedback-off"),
            args.windows(2)
                .find(|a| a[0] == "--draw-lod")
                .map(|a| amoris_render::LodMode::parse(&a[1]).expect("--draw-lod is off or on")),
            ProjectBench {
                warmup: number("--warmup", 10),
                rows: args
                    .windows(2)
                    .find(|a| a[0] == "--rows")
                    .map(|a| a[1].clone().into()),
                snapshots: args.iter().any(|a| a == "--snapshots"),
                decode_frame: args.iter().any(|a| a == "--decode-frame"),
                wake: args
                    .windows(2)
                    .find(|a| a[0] == "--wake")
                    .map(|a| a[1].parse().expect("numeric wake tick")),
            },
        );
        return;
    }
    let probes = number("--probes", 0);
    let bodies = number("--bodies", 0);
    assert!(ticks >= 64 && probes <= 64 && bodies <= 128 && bodies * 27 + probes <= 4096);
    let source = args.iter().any(|a| a == "--source");
    println!(
        "side,mode,probes,source,water_p50_ms,water_p95_ms,step_p50_ms,publish_p50_ms,checkpoint_max_ms,feed_mean_bytes,bodies,buoyancy_p50_ms,rigid_p50_ms"
    );
    for side in [128, 256, 512] {
        for mode in ["cpu", "gpu", "gpu_resident"] {
            let mut game = water_compute::builder(setup(side, mode, probes, source, bodies), 1)
                .build()
                .unwrap();
            let feed = amoris_assets::Feed::new();
            let mailbox = feed.subscribe();
            let mut extractor = Extractor::default();
            let mut timer = Timer::default();
            let (mut water, mut step, mut publish) = (vec![], vec![], vec![]);
            let (mut buoyancy, mut rigid) = (vec![], vec![]);
            let (mut checkpoint_max, mut bytes) = (0.0f64, 0usize);
            for tick in 1..=ticks + 10 {
                let start = Instant::now();
                let report = game.step_with(&mut timer).unwrap();
                assert!(
                    report.errors.is_empty(),
                    "benchmark tick failed: {:?}",
                    report.errors
                );
                let elapsed = start.elapsed().as_secs_f64() * 1e3;
                let start = Instant::now();
                game.present(&mut extractor, &feed);
                let frame = mailbox.take();
                let len = frame.encode().len();
                let sent = start.elapsed().as_secs_f64() * 1e3;
                if tick > 10 {
                    water.push(timer.water_ms);
                    step.push(elapsed);
                    publish.push(sent);
                    buoyancy.push(timer.buoyancy_ms);
                    rigid.push(timer.rigid_ms);
                    bytes += len;
                    if tick % 64 == 0 {
                        checkpoint_max = checkpoint_max.max(timer.water_ms);
                    }
                }
            }
            let p50 = percentile(&mut water, 0.5);
            let p95 = percentile(&mut water, 0.95);
            println!(
                "{side},{mode},{},{source},{p50:.4},{p95:.4},{:.4},{:.4},{checkpoint_max:.4},{:.0},{bodies},{:.4},{:.4}",
                if mode == "gpu_resident" { probes } else { 0 },
                percentile(&mut step, 0.5),
                percentile(&mut publish, 0.5),
                bytes as f64 / f64::from(ticks),
                percentile(&mut buoyancy, 0.5),
                percentile(&mut rigid, 0.5)
            );
        }
    }
}
fn main() {
    std::thread::Builder::new()
        .stack_size(amoris_runtime::GAME_STACK_BYTES)
        .spawn(run)
        .unwrap()
        .join()
        .unwrap();
}
