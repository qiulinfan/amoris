//! The water's sound (water.md 59) in the frontier samples, played through the player's own
//! actions: the pool party's swimmer runs off the deck, jumps into the deep end and swims for the
//! ducks; the surf beach's rider lies on the board and paddles out among the breaking swell; the
//! river falls' rider paddles the tube down the gorge and over the falls. Each tick the renderer
//! draws the player's eye and tallies what the water's spray sources sound like there; the audio
//! presenter's synthesizer then turns the tallies into a stereo recording.
//! cargo run --release -p amoris-app --example water_sound -- --sample river-falls --out out/sound
//! writes `<out>/<sample>.wav` (48 kHz, 16 bit) and `<out>/<sample>.json` (each kind's events a
//! second over time, every tick's tally, and the synthesizer's time); the directory belongs in
//! ignored `out/`. `--from <json>` sounds a recorded run's tallies again without playing it;
//! `--deaf` plays without listening, to time the tally against (with `AMORIS_TRACE`).
#[path = "../src/water_compute.rs"]
mod water_compute;
use amoris_audio::water::{KINDS, WaterSource, WaterSynth};
use amoris_link::Source;
use amoris_render::water_acoustics::{AcousticSource, NAMES, WaterAcoustics};
use amoris_render::{Antialiasing, BackendChoice, Gpu, Gtao, Renderer};
use amoris_runtime::{Command, Extractor, Game, Project};
use glam::Vec3;
use serde_json::{Value, json};
use std::sync::Arc;

const W: u32 = 640;
const H: u32 = 360;
const RATE: u32 = 48_000;

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

type Script = Box<dyn FnMut(&mut Game, &mut Player, u32) + Send>;

/// The pool party: run west off the east deck, jump from the coping, then swim for the nearest
/// duck.
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
        if jumped && walker["Walker"]["in_liquid"] == json!(true) && tick % 6 == 0 {
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
    })
}

/// The surf beach: lie on the board facing the beach and paddle into the swell's path.
fn surf_beach() -> Script {
    Box::new(move |g, player, _tick| {
        let board = fields(g, "Board", &["Transform"]);
        let n = nose(&board["Transform"]["rotation"]);
        let error = wrap(yaw_of(1.0, 0.0) - yaw_of(n[0], n[1]));
        player.act(
            g,
            json!([{"do":"set","controls":{"forward":1.0,"strafe":(-2.0 * error).clamp(-1.0, 1.0)}}]),
        );
    })
}

/// The river: lie on the tube and paddle downstream, steering for the river's middle.
fn river_falls() -> Script {
    Box::new(move |g, player, _tick| {
        let tube = fields(g, "Tube", &["Transform"]);
        let n = nose(&tube["Transform"]["rotation"]);
        let at = vec3(&tube["Transform"]["position"]);
        let error = wrap(yaw_of(1.0, f64::from(16.0 - at.z) * 0.1) - yaw_of(n[0], n[1]));
        player.act(
            g,
            json!([{"do":"set","controls":{"forward":1.0,"strafe":(-2.0 * error).clamp(-1.0, 1.0)}}]),
        );
    })
}

/// A 16-bit stereo WAV file.
fn wav(frames: &[[f32; 2]], rate: u32) -> Vec<u8> {
    let data = (frames.len() * 4) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for f in frames {
        for v in f {
            out.extend_from_slice(&((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
        }
    }
    out
}

/// Plays the sample for `ticks` ticks through the script, drawing the player's eye, and returns
/// each tick's tally and the GPU it ran on.
fn play(
    root: &std::path::Path,
    script: &mut Script,
    ticks: u32,
    listen: bool,
) -> (Vec<WaterAcoustics>, Value) {
    let setup = Arc::new(Project::load(root).unwrap().setup(false).unwrap());
    let mut game = water_compute::builder(setup, 1).build().unwrap();
    let feed = amoris_assets::Feed::new();
    let mailbox = feed.subscribe();
    let mut extractor = Extractor::default();
    let gpu = Gpu::headless(BackendChoice::from_env()).unwrap();
    let target = gpu
        .device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("water sound"),
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
    let mut renderer = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
    renderer.set_asset_source(Box::new(amoris_render::loader::FileAssets::new(
        root.to_path_buf(),
    )));
    renderer.set_antialiasing(Antialiasing::Msaa);
    renderer.set_gtao(Gtao::Off);
    renderer.set_water_acoustics(listen);
    let mut player = Player { sequence: 0 };
    let mut heard = Vec::with_capacity(ticks as usize);
    let mut latest = WaterAcoustics::default();
    for tick in 1..=ticks {
        script(&mut game, &mut player, tick);
        let result = game.step().unwrap();
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        game.present(&mut extractor, &feed);
        let t = f64::from(tick) / 60.0;
        renderer.apply(mailbox.take(), t);
        renderer.render(&target, t);
        // Offline, wait for this frame's tally instead of hearing it a frame late.
        let _ = gpu.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(10)),
        });
        if let Some(a) = renderer.water_acoustics() {
            latest = a;
        }
        heard.push(latest);
    }
    let gpu = json!({"backend": format!("{:?}", gpu.info.backend), "adapter": gpu.info.name});
    (heard, gpu)
}

/// A recorded run's tallies (`--from`).
fn recorded(path: &str) -> Vec<WaterAcoustics> {
    let v: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    v["tallies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| WaterAcoustics {
            dt: 1.0 / 60.0,
            kinds: std::array::from_fn(|k| AcousticSource {
                rate: t[k][0].as_f64().unwrap() as f32,
                left: t[k][1].as_f64().unwrap() as f32,
                right: t[k][2].as_f64().unwrap() as f32,
            }),
        })
        .collect()
}

/// Sounds the tallies, each for its tick's sixtieth of a second; returns the frames, the
/// synthesizer's seconds and the most bubbles it rang at once.
fn sound(heard: &[WaterAcoustics]) -> (Vec<[f32; 2]>, f64, usize) {
    let mut synth = WaterSynth::new(RATE, 1);
    let per_tick = (RATE / 60) as usize;
    let mut frames = vec![[0.0f32; 2]; per_tick * heard.len()];
    let started = std::time::Instant::now();
    let mut most = 0;
    for (a, chunk) in heard.iter().zip(frames.chunks_mut(per_tick)) {
        synth.set(std::array::from_fn(|k| WaterSource {
            rate: a.kinds[k].rate,
            left: a.kinds[k].left,
            right: a.kinds[k].right,
        }));
        synth.render(chunk);
        most = most.max(synth.ringing());
    }
    (frames, started.elapsed().as_secs_f64(), most)
}

fn main() {
    amoris_render::stderr_log::init();
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| args.windows(2).find(|a| a[0] == name).map(|a| a[1].clone());
    let sample = arg("--sample").unwrap_or_else(|| "river-falls".into());
    let out = std::path::PathBuf::from(arg("--out").unwrap_or_else(|| "out/sound".into()));
    let ticks: u32 = arg("--ticks").map_or(1200, |t| t.parse().unwrap());
    std::fs::create_dir_all(&out).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../samples")
        .join(&sample);
    let mut script = match sample.as_str() {
        "pool-party" => pool_party(),
        "surf-beach" => surf_beach(),
        "river-falls" => river_falls(),
        other => panic!("no script for {other}"),
    };
    let from = arg("--from");
    let listen = !args.iter().any(|a| a == "--deaf");
    std::thread::Builder::new()
        .stack_size(amoris_runtime::GAME_STACK_BYTES)
        .spawn(move || {
            let (heard, gpu) = match &from {
                Some(path) => (recorded(path), Value::Null),
                None => play(&root, &mut script, ticks, listen),
            };
            let (frames, synth_s, most) = sound(&heard);
            std::fs::write(out.join(format!("{sample}.wav")), wav(&frames, RATE)).unwrap();
            let seconds = heard.len() as f64 / 60.0;
            let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len().max(1) as f32;
            let max = |v: &[f32]| v.iter().fold(0.0f32, |m, x| m.max(*x));
            let kinds: serde_json::Map<String, Value> = (0..KINDS)
                .map(|k| {
                    let rates: Vec<f32> = heard.iter().map(|a| a.kinds[k].rate).collect();
                    let energy: Vec<f32> = heard
                        .iter()
                        .map(|a| a.kinds[k].left + a.kinds[k].right)
                        .collect();
                    (
                        NAMES[k].to_string(),
                        json!({"events_per_s": {"mean": mean(&rates), "max": max(&rates)},
                               "energy_per_s": {"mean": mean(&energy), "max": max(&energy)}}),
                    )
                })
                .collect();
            let peak = frames
                .iter()
                .fold(0.0f32, |m, f| m.max(f[0].abs()).max(f[1].abs()));
            let tallies: Vec<Value> = heard
                .iter()
                .map(|a| json!(a.kinds.iter().map(|s| [s.rate, s.left, s.right]).collect::<Vec<_>>()))
                .collect();
            let summary = json!({
                "sample": sample,
                "seconds": seconds,
                "rate_hz": RATE,
                "gpu": gpu,
                "kinds": kinds,
                "synth": {"seconds": synth_s, "share_of_real_time": synth_s / seconds,
                          "most_bubbles_ringing": most, "peak": peak},
                "tallies": tallies,
            });
            let name = if from.is_some() { format!("{sample}-again.json") } else { format!("{sample}.json") };
            std::fs::write(out.join(name), serde_json::to_vec(&summary).unwrap()).unwrap();
            println!(
                "{}",
                json!({"sample": summary["sample"], "kinds": summary["kinds"], "synth": summary["synth"]})
            );
        })
        .unwrap()
        .join()
        .unwrap();
}
