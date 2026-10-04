//! `pocket play <project>`: the game in a native window (Metal on macOS, Vulkan elsewhere or with
//! `POCKET_BACKEND=vulkan`). The game runs on its game thread in real time; the window's renderer
//! subscribes to the render feed and draws at the display's rate, interpolating between ticks.
//! Runs on the process's main thread (winit requires it on macOS).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use glam::Vec3;
use pocket_assets::{Feed, Mailbox};
use pocket_contract::{Problem, detail};
use pocket_render::app::{Host, RunOptions, run};
use pocket_render::loader::FileAssets;
use pocket_render::{BackendChoice, CameraState, FrameStats, Renderer};
use pocket_runtime::thread::{GameHandle, GameThread, Pacing, ThreadOptions};
use pocket_runtime::{Game, Project};
use serde_json::json;

use crate::check::Outcome;
use crate::cli::{Args, Flag};

const FLAGS: &[Flag] = &[
    ("seed", true),
    ("speed", true),
    ("fly", false),
    ("vsync", false),
    ("bench", true),
    ("width", true),
    ("height", true),
    ("capture", true),
    ("mute", false),
    ("capture-after", true),
];

struct Play {
    mailbox: Arc<Mailbox>,
    game: GameHandle,
    audio: Option<pocket_audio::Audio>,
    events: pocket_link::EventCursor,
    /// The project's `[sounds]`: event name (or `prefix.*`) to clip.
    sounds: std::collections::BTreeMap<String, String>,
    assets_set: Option<PathBuf>,
    /// `--capture PATH`: save the frame after `capture_after` frames, then close.
    capture: Option<(PathBuf, u32)>,
    frames: u32,
}

impl Host for Play {
    fn update(&mut self, r: &mut Renderer, now_s: f64) {
        if let Some(root) = self.assets_set.take() {
            r.set_asset_source(Box::new(FileAssets::new(root)));
        }
        r.apply(self.mailbox.take(), now_s);
        if let Some(audio) = self.audio.as_mut() {
            let cam = r.camera();
            audio.set_listener(cam.position, cam.rotation);
            audio.sync(&r.scene.audio);
            let batch = self.game.reader().events(&mut self.events, 256);
            for rec in batch.records {
                let Ok(ev) = rec.to_json() else { continue };
                let name = ev["name"].as_str().unwrap_or("");
                let data = &ev["data"];
                let clip = if name == "sound" {
                    data["clip"].as_str().map(str::to_owned)
                } else {
                    self.sounds.get(name).cloned().or_else(|| {
                        self.sounds.iter().find_map(|(k, v)| {
                            k.strip_suffix(".*").filter(|p| name.starts_with(&format!("{p}."))).map(|_| v.clone())
                        })
                    })
                };
                let Some(clip) = clip else { continue };
                let at = ev["subject"]
                    .as_u64()
                    .and_then(|e| r.scene.position_of(e))
                    .map(glam::Vec3::from);
                let volume = data["volume"].as_f64().unwrap_or(1.0) as f32;
                let pitch = data["pitch"].as_f64().unwrap_or(1.0) as f32;
                audio.play_once(&clip, volume, pitch, at);
            }
        }
    }

    fn after_frame(&mut self, _stats: &FrameStats) {
        self.frames += 1;
    }

    fn wants_capture(&mut self) -> bool {
        self.capture
            .as_ref()
            .is_some_and(|(_, after)| self.frames >= *after)
    }

    fn captured(&mut self, w: u32, h: u32, rgba: Vec<u8>) -> bool {
        if let Some((path, _)) = self.capture.take() {
            match image::save_buffer(&path, &rgba, w, h, image::ColorType::Rgba8) {
                Ok(()) => eprintln!("saved {} ({w}x{h})", path.display()),
                Err(e) => eprintln!("could not save {}: {e}", path.display()),
            }
        }
        true
    }
}

fn fail(p: &Problem, code: i32) -> Outcome {
    Outcome {
        stdout: serde_json::to_string_pretty(&json!({"error": p})).unwrap_or_default(),
        code,
    }
}

/// `pocket play`.
pub fn play(raw: &[String]) -> Outcome {
    match play_inner(raw) {
        Ok(o) => o,
        Err(p) => fail(&p, 1),
    }
}

fn play_inner(raw: &[String]) -> Result<Outcome, Problem> {
    let args = Args::parse(raw, FLAGS)?;
    let dir = args.positional.first().map(PathBuf::from).ok_or_else(|| {
        Problem::new(
            "check.usage",
            "pocket play needs a project directory",
            detail([]),
        )
    })?;
    let project = Project::load(&dir)?;
    let seed = args.number::<u64>("seed")?.unwrap_or(project.manifest.seed);
    let speed = args.number::<f64>("speed")?.unwrap_or(1.0);
    let setup = Arc::new(project.setup(false)?);
    let start = Instant::now();
    let clock: pocket_runtime::thread::Clock =
        Arc::new(move || start.elapsed().as_secs_f64() * 1000.0);
    let feed = Feed::new();
    let mailbox = feed.subscribe();
    let mut options = ThreadOptions::new(clock);
    options.pacing = Pacing::RealTime { speed };
    options.feed = Some(feed);
    let game = GameThread::spawn(move || Game::new(setup, seed), options)?;
    let fly = args
        .has("fly")
        .then(|| CameraState::look_at(Vec3::new(-14.0, 7.0, 16.0), Vec3::new(8.0, 0.0, 0.0)));
    let report = run(
        Play {
            mailbox,
            game,
            audio: if args.has("mute") { None } else { pocket_audio::Audio::new(Some(dir.clone())) },
            events: pocket_link::EventCursor::start(),
            sounds: project.manifest.sounds.clone(),
            assets_set: Some(dir.clone()),
            capture: args.value("capture").map(|p| {
                (
                    PathBuf::from(p),
                    args.number::<u32>("capture-after")
                        .ok()
                        .flatten()
                        .unwrap_or(120),
                )
            }),
            frames: 0,
        },
        RunOptions {
            title: format!("Pocket3D — {}", project.manifest.name),
            width: args.number::<u32>("width")?.unwrap_or(1280),
            height: args.number::<u32>("height")?.unwrap_or(720),
            backend: BackendChoice::from_env(),
            vsync: args.has("vsync"),
            fly_camera: fly,
            bench: args.number::<u32>("bench")?.map(|n| (60, n)),
        },
    )
    .map_err(|e| Problem::new("render.failed", e, detail([])))?;
    Ok(Outcome {
        stdout: report.map(|r| format!("{r:#?}")).unwrap_or_default(),
        code: 0,
    })
}
