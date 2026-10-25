//! Sub-grid ripples (water.md 57) in the frontier samples, played through the player's own actions:
//! the pool party's swimmer runs off the deck, jumps into the deep end and swims for the ducks; the
//! surf beach's rider lies on the board and paddles; the river falls' rider paddles its tube
//! downstream, ripples in a fast current. Renderers with and without the ripples draw the same
//! frames of the actual game, from the player's eye and from a camera beside them; the captures
//! (`<out>/<sample>/{eye,side}-{on,off}-<tick>.png`) and the ripples' GPU time
//! (`<out>/<sample>/timing.json`) go under the directory given, which belongs in ignored `out/`.
//! cargo run --release -p amoris-app --example ripple_shots -- --sample pool-party --out out/ripples
//! `--only on|off` draws one renderer, the player's eye with the ripples on or off, and captures
//! nothing: with `AMORIS_TRACE=frames=300,skip=120,out=PATH`, a frame trace for
//! `tools/frame_trace.py` (the passes overlap, so their sum is no cost; the frame span and the
//! passes' union are).
#[path = "../src/water_compute.rs"]
mod water_compute;
use amoris_link::Source;
use amoris_render::{Antialiasing, BackendChoice, CameraState, Gpu, Gtao, Renderer};
use amoris_runtime::{Command, Extractor, Game, Project};
use glam::Vec3;
use serde_json::{Value, json};
use std::sync::Arc;

const W: u32 = 1280;
const H: u32 = 720;

/// The named entity's components, or null once it is gone (a collected duck).
fn fields(g: &mut Game, entity: &str, components: &[&str]) -> Value {
    g.apply(&Command::new(
        Source::Developer(9),
        1,
        "world.get",
        json!({"entity": entity, "fields": components}),
    ))
    .map_or(Value::Null, |v| v["fields"].clone())
}
fn vec3(v: &Value) -> Vec3 {
    Vec3::new(
        v[0].as_f64().unwrap() as f32,
        v[1].as_f64().unwrap() as f32,
        v[2].as_f64().unwrap() as f32,
    )
}
/// The yaw a heading has, Walker style: 0 along -z, positive to the left.
fn yaw_of(x: f64, z: f64) -> f64 {
    (-x).atan2(-z)
}
fn wrap(a: f64) -> f64 {
    (a + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI
}
/// The direction a body's nose (-z in its frame) points along the water.
fn nose(q: &Value) -> [f64; 2] {
    let [qx, qy, qz, qw] = [0, 1, 2, 3].map(|i| q[i].as_f64().unwrap());
    [
        -2.0 * (qx * qz + qw * qy),
        -(1.0 - 2.0 * (qx * qx + qy * qy)),
    ]
}

struct Player {
    sequence: u64,
}
impl Player {
    fn act(&mut self, g: &mut Game, actions: Value) {
        self.sequence += 1;
        g.apply(&Command::new(
            Source::Player(0),
            self.sequence,
            "player.act",
            json!({ "actions": actions }),
        ))
        .unwrap();
    }
}

/// What the player does this tick; returns the point the side camera watches, if any.
type Script = Box<dyn FnMut(&mut Game, &mut Player, u32) -> Option<(Vec3, Vec3)> + Send>;

/// The pool party: run west off the east deck, jump from the coping, then swim for the nearest
/// duck holding `rise`, so the eye stays over the water.
fn pool_party() -> Script {
    let mut jumped = false;
    Box::new(move |g, player, tick| {
        let walker = fields(g, "Swimmer", &["Walker", "Transform"]);
        let eye = vec3(&walker["Transform"]["position"]);
        if tick == 3 {
            player.act(
                g,
                json!([{"do":"set","controls":{"forward":1.0,"yaw":std::f64::consts::FRAC_PI_2,"run":true}}]),
            );
        }
        if !jumped && tick > 3 && eye.x < 16.5 {
            player.act(g, json!([{"do":"pulse","control":"jump"}]));
            jumped = true;
        }
        let swimming = jumped && walker["Walker"]["in_liquid"] == json!(true);
        if swimming && tick % 6 == 0 {
            let nearest = (0..8)
                .filter_map(|n| {
                    let d = fields(g, &format!("Duck{n}"), &["Transform"]);
                    d["Transform"]["position"]
                        .is_array()
                        .then(|| vec3(&d["Transform"]["position"]))
                })
                .min_by(|a, b| a.distance(eye).total_cmp(&b.distance(eye)));
            if let Some(duck) = nearest {
                let yaw = yaw_of(f64::from(duck.x - eye.x), f64::from(duck.z - eye.z));
                player.act(
                    g,
                    json!([{"do":"set","controls":{"forward":1.0,"yaw":yaw,"rise":true,"run":false}}]),
                );
            }
        }
        // Behind the swimmer and above, looking at the water just ahead of them.
        let yaw = walker["Walker"]["yaw"].as_f64().unwrap() as f32;
        let ahead = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
        let water = Vec3::new(eye.x, 0.0, eye.z);
        Some((water - ahead * 2.6 + Vec3::Y * 2.2, water + ahead * 1.2))
    })
}

/// The surf beach: lie on the board facing the beach and paddle.
fn surf_beach() -> Script {
    Box::new(move |g, player, _tick| {
        let board = fields(g, "Board", &["Transform"]);
        let q = &board["Transform"]["rotation"];
        let n = nose(q);
        let error = wrap(yaw_of(1.0, 0.0) - yaw_of(n[0], n[1]));
        player.act(
            g,
            json!([{"do":"set","controls":{"forward":1.0,"strafe":(-2.0 * error).clamp(-1.0, 1.0)}}]),
        );
        let at = vec3(&board["Transform"]["position"]);
        let ahead = Vec3::new(n[0] as f32, 0.0, n[1] as f32).normalize();
        let side = Vec3::new(-ahead.z, 0.0, ahead.x);
        let water = Vec3::new(at.x, 0.0, at.z);
        Some((
            water - ahead * 2.0 + side * 2.4 + Vec3::Y * 2.0,
            water + ahead * 0.6,
        ))
    })
}

/// The river: lie on the tube and paddle downstream, steering for the river's middle.
fn river_falls() -> Script {
    Box::new(move |g, player, _tick| {
        let tube = fields(g, "Tube", &["Transform"]);
        let q = &tube["Transform"]["rotation"];
        let n = nose(q);
        let at = vec3(&tube["Transform"]["position"]);
        let error = wrap(yaw_of(1.0, f64::from(16.0 - at.z) * 0.1) - yaw_of(n[0], n[1]));
        player.act(
            g,
            json!([{"do":"set","controls":{"forward":1.0,"strafe":(-2.0 * error).clamp(-1.0, 1.0)}}]),
        );
        let ahead = Vec3::new(n[0] as f32, 0.0, n[1] as f32).normalize();
        let side = Vec3::new(-ahead.z, 0.0, ahead.x);
        let water = Vec3::new(at.x, at.y, at.z);
        Some((
            water - ahead * 2.5 + side * 2.0 + Vec3::Y * 2.0,
            water + ahead * 0.8,
        ))
    })
}

struct View {
    renderer: Renderer,
    ripples: bool,
    side: bool,
}

fn median(mut v: Vec<f32>) -> Option<f32> {
    v.sort_by(f32::total_cmp);
    (!v.is_empty()).then(|| v[v.len() / 2])
}
fn percentile(mut v: Vec<f32>, p: f64) -> Option<f32> {
    v.sort_by(f32::total_cmp);
    (!v.is_empty()).then(|| v[((v.len() - 1) as f64 * p).round() as usize])
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| args.windows(2).find(|a| a[0] == name).map(|a| a[1].clone());
    let sample = arg("--sample").unwrap_or_else(|| "pool-party".into());
    let out = std::path::PathBuf::from(arg("--out").unwrap_or_else(|| "out/ripples".into()))
        .join(&sample);
    let ticks: u32 = arg("--ticks").map_or(720, |t| t.parse().unwrap());
    let every: u32 = arg("--every").map_or(120, |t| t.parse().unwrap());
    let only = arg("--only").map(|o| o == "on");
    std::fs::create_dir_all(&out).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../samples")
        .join(&sample);
    let mut script = match sample.as_str() {
        "pool-party" => pool_party(),
        "surf-beach" => surf_beach(),
        "river-falls" => river_falls(),
        other => panic!("no ripple script for {other}"),
    };
    std::thread::Builder::new()
        .stack_size(amoris_runtime::GAME_STACK_BYTES)
        .spawn(move || {
            let setup = Arc::new(Project::load(&root).unwrap().setup(false).unwrap());
            let mut game = water_compute::builder(setup, 1).build().unwrap();
            let feed = amoris_assets::Feed::new();
            let mailbox = feed.subscribe();
            let mut extractor = Extractor::default();
            let gpu = Gpu::headless(BackendChoice::from_env()).unwrap();
            let target = gpu
                .device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("ripple shots"),
                    size: wgpu::Extent3d {
                        width: W,
                        height: H,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&Default::default());
            let all = [(true, false), (false, false), (true, true), (false, true)];
            let mut views: Vec<View> = all
                .into_iter()
                .filter(|(ripples, side)| only.is_none_or(|on| on == *ripples && !side))
                .map(|(ripples, side)| {
                    let mut renderer =
                        Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
                    renderer.set_asset_source(Box::new(amoris_render::loader::FileAssets::new(
                        root.clone(),
                    )));
                    renderer.set_antialiasing(Antialiasing::Msaa);
                    renderer.set_gtao(Gtao::Off);
                    renderer.set_water_ripples(ripples);
                    View {
                        renderer,
                        ripples,
                        side,
                    }
                })
                .collect();
            let mut player = Player { sequence: 0 };
            let (mut ripple_ms, mut frame_on, mut frame_off) = (Vec::new(), Vec::new(), Vec::new());
            // Each GPU pass's time with the ripples and without, for where the difference goes.
            let mut passes: std::collections::BTreeMap<(&'static str, bool), Vec<f32>> =
                Default::default();
            let (mut stir_bytes, mut stir_points) = (Vec::new(), Vec::new());
            for tick in 1..=ticks {
                let watch = script(&mut game, &mut player, tick);
                let result = game.step().unwrap();
                assert!(result.errors.is_empty(), "{:?}", result.errors);
                game.present(&mut extractor, &feed);
                let frame = mailbox.take();
                if let Some(stirs) = &frame.stirs {
                    // What the list adds to the frame on the wire.
                    let bare = amoris_assets::frame::RenderFrame::default();
                    let with = amoris_assets::frame::RenderFrame {
                        stirs: Some(stirs.clone()),
                        ..amoris_assets::frame::RenderFrame::default()
                    };
                    stir_points.push(stirs.len() as f32);
                    stir_bytes.push((with.encode().len() - bare.encode().len()) as f32);
                }
                let t = f64::from(tick) / 60.0;
                let shoot = only.is_none() && (tick % every == 0 || tick == ticks);
                for view in &mut views {
                    view.renderer.apply(frame.clone(), t);
                    if view.side {
                        view.renderer.set_camera_override(
                            watch.map(|(eye, at)| CameraState::look_at(eye, at)),
                        );
                    }
                    if shoot {
                        let (w, h, pixels) = view.renderer.capture_rgba(t);
                        let name = format!(
                            "{}-{}-{tick:04}.png",
                            if view.side { "side" } else { "eye" },
                            if view.ripples { "on" } else { "off" }
                        );
                        image::save_buffer(out.join(name), &pixels, w, h, image::ColorType::Rgba8)
                            .unwrap();
                    } else {
                        let stats = view.renderer.render(&target, t);
                        if !view.side && tick > 60 {
                            for (name, ms) in &stats.passes {
                                passes.entry((name, view.ripples)).or_default().push(*ms);
                            }
                            if view.ripples {
                                frame_on.push(stats.gpu_ms);
                                if let Some((_, ms)) =
                                    stats.passes.iter().find(|(n, _)| *n == "water ripples")
                                {
                                    ripple_ms.push(*ms);
                                }
                            } else {
                                frame_off.push(stats.gpu_ms);
                            }
                        }
                    }
                }
            }
            let mut by_pass = serde_json::Map::new();
            for ((name, on), times) in passes {
                let entry = by_pass.entry(name).or_insert_with(|| json!({}));
                entry[if on { "on" } else { "off" }] = json!(median(times));
            }
            let summary = json!({
                "passes_median_ms": by_pass,
                "sample": sample,
                "ticks": ticks,
                "size": [W, H],
                "backend": format!("{:?}", gpu.info.backend),
                "adapter": gpu.info.name,
                "ripples_ms": {"median": median(ripple_ms.clone()), "p95": percentile(ripple_ms.clone(), 0.95), "frames": ripple_ms.len()},
                "frame_gpu_ms": {"on": median(frame_on), "off": median(frame_off)},
                "stirs": {"points_median": median(stir_points), "bytes_median": median(stir_bytes)},
            });
            std::fs::write(
                out.join("timing.json"),
                serde_json::to_vec_pretty(&summary).unwrap(),
            )
            .unwrap();
            println!("{summary}");
        })
        .unwrap()
        .join()
        .unwrap();
}
