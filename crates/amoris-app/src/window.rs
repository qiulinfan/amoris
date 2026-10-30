//! `amoris play <project>`: the game in a native window (Metal on macOS, Direct3D 12 on Windows,
//! Vulkan elsewhere: `amoris_render::host_backend`; `AMORIS_BACKEND` names another). The game runs
//! on its game thread in real time; the window's renderer subscribes to the render feed and draws
//! at the display's rate, interpolating between ticks. Runs on the process's main thread (winit
//! requires it on macOS).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Instant;

use amoris_assets::collision::{RawCollisionMesh, collision_primitive, load_collision_gltf};
use amoris_assets::frame::UiView;
use amoris_assets::{Feed, Mailbox};
use amoris_contract::{Problem, detail};
use amoris_render::app::{Host, RunOptions, WalkInput, run};
use amoris_render::loader::FileAssets;
use amoris_render::{CameraState, FrameStats, Renderer};
use amoris_runtime::Project;
use amoris_runtime::thread::{GameHandle, GameThread, Pacing, ThreadOptions};
use amoris_runtime::walk::{StaticWalkWorld, WalkController};
use glam::{Mat4, Quat, Vec3};
use serde_json::json;

use crate::check::Outcome;
use crate::cli::{Args, Flag};

const FLAGS: &[Flag] = &[
    ("seed", true),
    ("speed", true),
    ("fly", false),
    ("walk", false),
    ("vsync", false),
    ("no-vsync", false),
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
    audio: Option<amoris_audio::Audio>,
    events: amoris_link::EventCursor,
    /// The project's `[sounds]`: event name (or `prefix.*`) to clip.
    sounds: std::collections::BTreeMap<String, String>,
    assets_set: Option<PathBuf>,
    /// `--capture PATH`: save the frame after `capture_after` frames, then close.
    capture: Option<(PathBuf, u32)>,
    frames: u32,
    walk: Option<WalkSession>,
    /// `--walk` on a game whose catalog is `first_person`: the window is seat `player`.
    seat: Option<SeatWalk>,
}

impl Host for Play {
    fn update(&mut self, r: &mut Renderer, now_s: f64) {
        if let Some(root) = self.assets_set.take() {
            r.splats.set_root(root.clone());
            r.set_asset_source(Box::new(FileAssets::new(root)));
        }
        r.apply(self.mailbox.take(), now_s);
        if let Some(walk) = self.walk.as_mut() {
            walk.update(r);
        }
        if let Some(seat) = self.seat.as_mut() {
            seat.update(r);
        }
        if let Some(audio) = self.audio.as_mut() {
            let cam = r.camera();
            audio.set_listener(cam.position, cam.rotation);
            audio.sync(&r.scene.audio);
            // The water's own sound, from what its spray's sources did a frame or two ago.
            r.set_water_acoustics(true);
            if let Some(heard) = r.water_acoustics() {
                audio.set_water(&water_sources(&heard));
            }
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
                            k.strip_suffix(".*")
                                .filter(|p| name.starts_with(&format!("{p}.")))
                                .map(|_| v.clone())
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
        // Capture warmup starts when the town and its collision geometry are both ready.
        if self.walk.as_ref().is_none_or(|w| w.ready) {
            self.frames += 1;
        }
    }

    fn walk_input(&mut self, input: WalkInput) {
        if let Some(walk) = self.walk.as_mut() {
            walk.input = input.clone();
        }
        if let Some(seat) = self.seat.as_mut() {
            seat.take(input);
        }
    }

    fn status(&self) -> Option<&str> {
        self.walk
            .as_ref()
            .map(|w| w.status.as_str())
            .or_else(|| self.seat.as_ref().map(|s| s.status.as_str()))
    }

    fn wants_capture(&mut self) -> bool {
        self.walk.as_ref().is_none_or(|w| w.ready)
            && self
                .capture
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

/// The renderer's tally of the water's sources, as the audio presenter takes it (same kinds, same
/// order: water.md 59).
fn water_sources(
    heard: &amoris_render::water_acoustics::WaterAcoustics,
) -> [amoris_audio::water::WaterSource; amoris_audio::water::KINDS] {
    std::array::from_fn(|k| amoris_audio::water::WaterSource {
        rate: heard.kinds[k].rate,
        left: heard.kinds[k].left,
        right: heard.kinds[k].right,
    })
}

fn fail(p: &Problem, code: i32) -> Outcome {
    Outcome {
        stdout: serde_json::to_string_pretty(&json!({"error": p})).unwrap_or_default(),
        code,
    }
}

/// `amoris play`.
pub fn play(raw: &[String]) -> Outcome {
    match play_inner(raw) {
        Ok(o) => o,
        Err(p) => fail(&p, 1),
    }
}

fn play_inner(raw: &[String]) -> Result<Outcome, Problem> {
    let args = Args::parse(raw, FLAGS)?;
    if args.has("walk") && args.has("fly") {
        return Err(Problem::new(
            "check.usage",
            "choose either --walk or --fly",
            detail([]),
        ));
    }
    let dir = args.positional.first().map(PathBuf::from).ok_or_else(|| {
        Problem::new(
            "check.usage",
            "amoris play needs a project directory",
            detail([]),
        )
    })?;
    let project = Project::load(&dir)?;
    // A first-person game is walked as its player seat (voxels.md 8); any other through the
    // presenter's own collision camera.
    let first_person = project
        .manifest
        .player
        .as_ref()
        .is_some_and(|p| p.actions == "first_person");
    let walk = if args.has("walk") && !first_person {
        Some(WalkSession::new(project.clone())?)
    } else {
        None
    };
    let seed = args.number::<u64>("seed")?.unwrap_or(project.manifest.seed);
    let speed = args.number::<f64>("speed")?.unwrap_or(1.0);
    let setup = Arc::new(project.setup(false)?);
    let start = Instant::now();
    let clock: amoris_runtime::thread::Clock =
        Arc::new(move || start.elapsed().as_secs_f64() * 1000.0);
    let feed = Feed::new();
    let mailbox = feed.subscribe();
    let mut options = ThreadOptions::new(clock);
    options.pacing = Pacing::RealTime { speed };
    options.feed = Some(feed);
    let game = GameThread::spawn(
        move || crate::water_compute::builder(setup, seed).build(),
        options,
    )?;
    let seat = if args.has("walk") && first_person {
        let game_camera = project
            .manifest
            .player
            .as_ref()
            .is_some_and(|p| p.camera == amoris_runtime::player::PlayerCamera::Game);
        let buttons = project
            .manifest
            .player
            .as_ref()
            .map(|p| p.buttons.iter().map(|b| b.keys.clone()).collect())
            .unwrap_or_default();
        Some(SeatWalk::new(
            game.client(amoris_link::Source::Player(0))?,
            game_camera,
            buttons,
        ))
    } else {
        None
    };
    let fly = args
        .has("fly")
        .then(|| CameraState::look_at(Vec3::new(-14.0, 7.0, 16.0), Vec3::new(8.0, 0.0, 0.0)));
    let report = run(
        Play {
            mailbox,
            game,
            audio: if args.has("mute") {
                None
            } else {
                amoris_audio::Audio::new(Some(dir.clone()))
            },
            events: amoris_link::EventCursor::start(),
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
            walk,
            seat,
        },
        RunOptions {
            title: format!("Amoris — {}", project.manifest.name),
            width: args.number::<u32>("width")?.unwrap_or(1280),
            height: args.number::<u32>("height")?.unwrap_or(720),
            backend: amoris_render::host_backend(),
            // Playing waits for the display, so frames pace evenly and never tear; a benchmark
            // (`--bench`) or `--no-vsync` draws as fast as it can.
            vsync: args.has("vsync") || (!args.has("no-vsync") && !args.has("bench")),
            fly_camera: fly,
            walk: args.has("walk"),
            bench: args.number::<u32>("bench")?.map(|n| (60, n)),
        },
    )
    .map_err(|e| Problem::new("render.failed", e, detail([])))?;
    Ok(Outcome {
        stdout: report.map(|r| format!("{r:#?}")).unwrap_or_default(),
        code: 0,
    })
}

/// The window as a first-person game's seat `player` (voxels.md 8): keys, the mouse and the wheel
/// become `player.act` controls and pulses of the walker in the world; the camera is the walker's,
/// turned at once by the window's own latest look so looking never waits on a tick.
struct SeatWalk {
    client: amoris_link::GameClient,
    input: WalkInput,
    /// Edges and steps gathered since the last update.
    pending: WalkInput,
    look: Option<(f32, f32)>,
    slot: u8,
    sent: Option<(f32, f32, bool, f32, f32, bool)>,
    status: String,
    /// The game's rules pose the scene's active camera; the mouse still steers the seat.
    game_camera: bool,
    /// The keys of each of the game's buttons (`[player] buttons`), and the held mask last sent.
    buttons: Vec<Vec<String>>,
    held: u32,
}

/// The mask of the game's buttons any of whose keys are among `names`.
fn button_mask(buttons: &[Vec<String>], names: &[String]) -> u32 {
    buttons
        .iter()
        .enumerate()
        .take(16)
        .filter(|(_, keys)| keys.iter().any(|k| names.contains(k)))
        .fold(0, |m, (i, _)| m | (1 << i))
}

impl SeatWalk {
    fn new(
        client: amoris_link::GameClient,
        game_camera: bool,
        buttons: Vec<Vec<String>>,
    ) -> SeatWalk {
        SeatWalk {
            client,
            input: WalkInput::default(),
            pending: WalkInput::default(),
            look: None,
            slot: 0,
            sent: None,
            status: "Click to play — Esc exits".into(),
            game_camera,
            buttons,
            held: 0,
        }
    }

    /// Keeps the frame's input; edges are kept until an update sends them.
    fn take(&mut self, input: WalkInput) {
        let p = &mut self.pending;
        p.primary |= input.primary;
        p.secondary |= input.secondary;
        p.jump |= input.jump;
        p.slot = input.slot.or(p.slot);
        p.slot_steps += input.slot_steps;
        p.look[0] += input.look[0];
        p.look[1] += input.look[1];
        p.pressed.extend(input.pressed.iter().cloned());
        self.input = input;
    }

    fn update(&mut self, r: &mut Renderer) {
        let input = std::mem::take(&mut self.pending);
        // The walker's camera, drawn between ticks with the world it sees, turned by the look.
        r.set_camera_override(None);
        r.set_look_override(None);
        // Before the first tick the camera has its body's pose from the scene file, not yet the
        // walker's look (`first_person` in amoris-viewport's web.rs).
        if !r.scene.cameras.iter().any(|c| c.active) || (self.look.is_none() && r.scene.tick == 0) {
            self.status = "Waiting for the player's camera…".into();
            return;
        }
        let mut camera = r.camera();
        let seen = {
            let f = camera.rotation * Vec3::NEG_Z;
            ((-f.x).atan2(-f.z), f.y.clamp(-1.0, 1.0).asin())
        };
        let (mut yaw, mut pitch) = self.look.unwrap_or(seen);
        // A game that poses the camera may turn it (a lock-on, animation.md 6): while the mouse is
        // still, the seat takes the camera's look as its own (web.rs `first_person` does the same).
        if self.game_camera && input.look == [0.0; 2] {
            let dyaw = (seen.0 - yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
                - std::f32::consts::PI;
            if dyaw.abs() > 0.02 || (seen.1 - pitch).abs() > 0.02 {
                (yaw, pitch) = seen;
            }
        }
        let captured = self.input.captured;
        if captured {
            yaw -= input.look[0] * 0.003;
            pitch = (pitch - input.look[1] * 0.003).clamp(-1.55, 1.55);
        }
        yaw = (yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        self.look = Some((yaw, pitch));
        let motion = if captured {
            self.input.motion
        } else {
            [0.0; 2]
        };
        let run = captured && self.input.fast;
        let rise = captured && self.input.rise;
        let mut actions = Vec::new();
        let now = (motion[1], motion[0], run, yaw, pitch, rise);
        if self.sent != Some(now) {
            actions.push(serde_json::json!({"do": "set", "controls": {
                "forward": motion[1], "strafe": motion[0], "run": run,
                "yaw": yaw, "pitch": pitch, "rise": rise}}));
            self.sent = Some(now);
        }
        let slot = match (input.slot, input.slot_steps) {
            (Some(s), _) => s,
            (None, 0) => self.slot,
            (None, n) => u8::try_from((i32::from(self.slot) + n).rem_euclid(9)).unwrap_or(0),
        };
        if slot != self.slot {
            self.slot = slot;
            actions.push(serde_json::json!({"do": "set", "controls": {"slot": slot.to_string()}}));
        }
        for (on, control) in [
            (input.jump, "jump"),
            (input.primary, "primary"),
            (input.secondary, "secondary"),
        ] {
            if on && captured {
                actions.push(serde_json::json!({"do": "pulse", "control": control}));
            }
        }
        // The game's own buttons (animation.md 6).
        if !self.buttons.is_empty() {
            let held = if captured {
                button_mask(&self.buttons, &self.input.held)
            } else {
                0
            };
            if held != self.held {
                self.held = held;
                actions.push(serde_json::json!({"do": "set", "controls": {"held": held}}));
            }
            let pressed = if captured {
                button_mask(&self.buttons, &input.pressed)
            } else {
                0
            };
            if pressed != 0 {
                actions.push(serde_json::json!({"do": "set", "controls": {"pressed": pressed}}));
            }
        }
        if !actions.is_empty() {
            let sent = self.client.send(
                "player.act",
                serde_json::json!({ "actions": actions }),
                None,
                |reply| {
                    if let Err(p) = reply {
                        eprintln!("player.act refused: {}", p.message);
                    }
                },
            );
            if let Err(p) = sent {
                eprintln!("player.act not sent: {}", p.message);
            }
        }
        if !self.game_camera {
            camera.rotation = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch);
            r.set_look_override(Some(camera.rotation));
        }
        // The targeted block's outline, from the renderer's own copy of the volumes.
        r.overlays.lines.clear();
        let dir = camera.rotation * Vec3::NEG_Z;
        if !self.game_camera
            && captured
            && let Some((corner, edge)) = r.voxel_target(camera.position.into(), dir.into(), 5.0)
        {
            let c = Vec3::from(corner) - Vec3::splat(0.002);
            let e = edge + 0.004;
            let p = |x: f32, y: f32, z: f32| (c + Vec3::new(x, y, z) * e).to_array();
            let corners = [
                p(0., 0., 0.),
                p(1., 0., 0.),
                p(1., 0., 1.),
                p(0., 0., 1.),
                p(0., 1., 0.),
                p(1., 1., 0.),
                p(1., 1., 1.),
                p(0., 1., 1.),
            ];
            for (a, b) in [
                (0, 1),
                (1, 2),
                (2, 3),
                (3, 0),
                (4, 5),
                (5, 6),
                (6, 7),
                (7, 4),
                (0, 4),
                (1, 5),
                (2, 6),
                (3, 7),
            ] {
                r.overlays.lines.push(amoris_render::overlay::OverlayLine {
                    a: corners[a],
                    b: corners[b],
                    color: [0.05, 0.05, 0.05, 0.85],
                    width: 2.0,
                });
            }
        }
        self.status = if captured {
            "Playing — Esc releases mouse"
        } else {
            "Click to play — Esc exits"
        }
        .into();
        const STATUS_ID: u64 = u64::MAX - 1;
        const CROSSHAIR_ID: u64 = u64::MAX - 2;
        r.scene
            .ui
            .retain(|u| u.id != STATUS_ID && u.id != CROSSHAIR_ID);
        r.scene.ui.push(walk_text(
            STATUS_ID,
            self.status.clone(),
            18.0,
            7,
            [0.0, 24.0],
        ));
        if captured && !self.game_camera {
            r.scene
                .ui
                .push(walk_text(CROSSHAIR_ID, "+".into(), 24.0, 4, [0.0, 0.0]));
        }
    }
}

/// A camera derived from the static scene, separate from the game's authoritative ECS state.
struct WalkSession {
    pending: Option<Receiver<Result<StaticWalkWorld, String>>>,
    world: Option<StaticWalkWorld>,
    controller: Option<WalkController>,
    camera: Option<CameraState>,
    spawn: Option<CameraState>,
    yaw: f32,
    pitch: f32,
    input: WalkInput,
    ready: bool,
    error: Option<String>,
    status: String,
    gi_enabled: bool,
}

impl WalkSession {
    fn new(project: Project) -> Result<Self, Problem> {
        let (tx, pending) = mpsc::channel();
        std::thread::Builder::new()
            .name("amoris-walk-collision".into())
            .spawn(move || {
                let _ = tx.send(load_walk_world(&project));
            })
            .map_err(|e| {
                Problem::new(
                    "walk.failed",
                    format!("start collision loader: {e}"),
                    detail([]),
                )
            })?;
        Ok(Self {
            pending: Some(pending),
            world: None,
            controller: None,
            camera: None,
            spawn: None,
            yaw: 0.0,
            pitch: 0.0,
            input: WalkInput::default(),
            ready: false,
            error: None,
            status: "Loading town…".into(),
            gi_enabled: true,
        })
    }

    fn fail(&mut self, error: String) {
        eprintln!("walk failed: {error}");
        self.status = format!("Walk unavailable: {error}");
        self.error = Some(error);
        self.ready = false;
    }

    fn respawn(&mut self) -> Result<(), String> {
        let camera = self.spawn.ok_or("walk spawn camera is not ready")?;
        let forward = camera.forward();
        self.yaw = forward.x.atan2(-forward.z);
        self.pitch = forward.y.clamp(-1.0, 1.0).asin();
        self.controller = Some(WalkController::new(camera.position.to_array())?);
        self.camera = Some(camera);
        Ok(())
    }

    fn update(&mut self, r: &mut Renderer) {
        if self.input.gi_toggle {
            self.gi_enabled = !self.gi_enabled;
        }
        // Keep the asset loaded for an instant comparison at the same camera pose.
        if let Some(environment) = r.scene.environment.as_mut() {
            environment.gi_intensity = if self.gi_enabled { 1.0 } else { 0.0 };
        }
        if let Some(pending) = &self.pending {
            match pending.try_recv() {
                Ok(result) => {
                    self.pending = None;
                    match result {
                        Ok(world) => self.world = Some(world),
                        Err(e) => self.fail(e),
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.fail("collision loader disconnected".into());
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if self.error.is_none() && self.world.is_some() {
            if r.scene.pending() != 0 || r.last.pending_assets != 0 || r.scene.instance_count() == 0
            {
                self.ready = false;
                self.status = "Loading town…".into();
            } else if self.controller.is_none() {
                if r.scene.cameras.iter().any(|c| c.active) {
                    let camera = r.camera();
                    match WalkController::new(camera.position.to_array()) {
                        Ok(controller) => {
                            let forward = camera.forward();
                            self.yaw = forward.x.atan2(-forward.z);
                            self.pitch = forward.y.clamp(-1.0, 1.0).asin();
                            self.camera = Some(camera);
                            self.spawn = Some(camera);
                            self.controller = Some(controller);
                            self.ready = true;
                        }
                        Err(e) => self.fail(e),
                    }
                } else {
                    self.status = "Waiting for the project's active camera…".into();
                }
            } else {
                self.ready = true;
            }
        }
        if self.ready {
            if self.input.reset {
                if let Err(e) = self.respawn() {
                    self.fail(e);
                }
                self.input.look = [0.0; 2];
                self.input.motion = [0.0; 2];
                self.input.jump = false;
            }
            self.yaw += self.input.look[0] * 0.003;
            self.pitch = (self.pitch - self.input.look[1] * 0.003).clamp(-1.55, 1.55);
            let yaw = Quat::from_rotation_y(-self.yaw);
            let speed = if self.input.fast { 6.0 } else { 3.0 };
            let motion = if self.input.captured {
                self.input.motion
            } else {
                [0.0; 2]
            };
            let velocity = yaw * Vec3::new(motion[0], 0.0, -motion[1]) * speed;
            let result = self.controller.as_mut().expect("ready controller").step(
                self.world.as_ref().expect("ready collision world"),
                self.input.dt_s,
                velocity.to_array(),
                self.input.jump && self.input.captured,
            );
            match result {
                Ok(state) => {
                    if state.feet[1] < -30.0 {
                        if let Err(e) = self.respawn() {
                            self.fail(e);
                        }
                    } else {
                        let camera = self.camera.as_mut().expect("ready camera");
                        camera.position = Vec3::from(state.eye);
                        camera.rotation = yaw * Quat::from_rotation_x(self.pitch);
                    }
                    let camera = self.camera.as_ref().expect("ready camera");
                    r.set_camera_override(Some(*camera));
                    self.status = if self.input.captured {
                        "Walking — Esc releases mouse"
                    } else {
                        "Click to walk — Esc exits"
                    }
                    .into();
                }
                Err(e) => self.fail(e),
            }
        }
        self.input.look = [0.0; 2];
        self.input.jump = false;
        self.input.reset = false;
        self.input.gi_toggle = false;
        // Presenter HUD is rebuilt by stable IDs; the scene's own UI remains intact.
        const STATUS_ID: u64 = u64::MAX - 1;
        const CROSSHAIR_ID: u64 = u64::MAX - 2;
        const GI_STATUS_ID: u64 = u64::MAX - 3;
        const GI_BACK_ID: u64 = u64::MAX - 4;
        r.scene
            .ui
            .retain(|item| ![STATUS_ID, CROSSHAIR_ID, GI_STATUS_ID, GI_BACK_ID].contains(&item.id));
        r.scene.ui.push(walk_text(
            STATUS_ID,
            self.status.clone(),
            18.0,
            7,
            [0.0, 24.0],
        ));
        let configured = r
            .scene
            .environment
            .as_ref()
            .is_some_and(|e| !e.baked_gi.is_empty() || !e.neural_gi.is_empty());
        let gi_status = walk_gi_status(self.gi_enabled, configured, r.gi_loaded(), r.gi_error());
        r.scene.ui.push(UiView {
            id: GI_BACK_ID,
            text: String::new(),
            fill: 0.0,
            bar: true,
            size: [300.0, 32.0],
            color: [0.0; 4],
            back: [0.0, 0.0, 0.0, 0.65],
            anchor: 0,
            offset: [16.0, 52.0],
            world: [0.0; 3],
        });
        r.scene
            .ui
            .push(walk_text(GI_STATUS_ID, gi_status, 16.0, 0, [24.0, 60.0]));
        if self.ready && self.input.captured {
            r.scene
                .ui
                .push(walk_text(CROSSHAIR_ID, "+".into(), 24.0, 4, [0.0, 0.0]));
        }
    }
}

fn walk_gi_status(enabled: bool, configured: bool, loaded: bool, error: Option<&str>) -> String {
    if let Some(error) = error {
        return format!("GI FAILED: {error}");
    }
    if !configured {
        return "GI unavailable: no lighting asset".into();
    }
    if !loaded {
        return "GI loading… · G toggle".into();
    }
    format!("GI {} · G toggle", if enabled { "ON" } else { "OFF" })
}

fn walk_text(id: u64, text: String, size: f32, anchor: u32, offset: [f32; 2]) -> UiView {
    UiView {
        id,
        text,
        fill: 0.0,
        bar: false,
        size: [size, size],
        color: [1.0, 1.0, 1.0, 1.0],
        back: [0.0; 4],
        anchor,
        offset,
        world: [0.0; 3],
    }
}

fn load_walk_world(project: &Project) -> Result<StaticWalkWorld, String> {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut cache = BTreeMap::<String, RawCollisionMesh>::new();
    for entity in &project.scene.entities {
        let mut components: BTreeMap<String, serde_json::Value> = entity
            .prefab
            .as_ref()
            .map(amoris_runtime::prefab_components)
            .unwrap_or_default()
            .into_iter()
            .map(|(name, value)| (name.into(), value))
            .collect();
        for (name, value) in &entity.components {
            if let Some(base) = components.get_mut(name) {
                if let Some(patch) = value.as_object() {
                    amoris_runtime::merge(base, patch)?;
                } else {
                    *base = value.clone();
                }
            } else {
                components.insert(name.clone(), value.clone());
            }
        }
        if components.contains_key("Animator")
            || components
                .get("RigidBody")
                .is_some_and(|v| v["kind"].as_str() != Some("Fixed"))
        {
            continue;
        }
        let Some(value) = components.get("Model") else {
            continue;
        };
        let model: amoris_assets::Model =
            serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        // A blended Model (alpha below 1: water, clouds) is not solid, as glTF BLEND geometry.
        if !model.visible || model.color[3] < 1.0 {
            continue;
        }
        if !cache.contains_key(&model.mesh) {
            let mesh = if let Some(mesh) = collision_primitive(&model.mesh) {
                mesh
            } else {
                let (path, selector) = model
                    .mesh
                    .split_once('#')
                    .map_or((model.mesh.as_str(), None), |(path, selector)| {
                        (path, Some(selector))
                    });
                load_collision_gltf(&project.root.join(path), selector).map_err(|p| p.message)?
            };
            cache.insert(model.mesh.clone(), mesh);
        }
        let transform = components.get("Transform");
        let position = json_vector(transform.and_then(|t| t.get("position")), [0.0; 3])?;
        let rotation = json_vector(
            transform.and_then(|t| t.get("rotation")),
            [0.0, 0.0, 0.0, 1.0],
        )?;
        let rotation = Quat::from_array(rotation);
        if !rotation.is_finite() || rotation.length_squared() < 1e-12 {
            return Err("walk model has invalid rotation".into());
        }
        let matrix = Mat4::from_scale_rotation_translation(
            Vec3::from(model.scale.map(|s| s as f32)),
            rotation.normalize(),
            Vec3::from(position),
        );
        let mut mesh = cache[&model.mesh].clone();
        mesh.apply_transform(matrix).map_err(|p| p.message)?;
        let start = u32::try_from(vertices.len()).map_err(|_| "walk mesh exceeds u32 indices")?;
        vertices.extend_from_slice(&mesh.vertices);
        for triangle in mesh.indices {
            let mut shifted = [0; 3];
            for i in 0..3 {
                shifted[i] = triangle[i]
                    .checked_add(start)
                    .ok_or("walk index overflow")?;
            }
            indices.push(shifted);
        }
    }
    StaticWalkWorld::new(vertices, indices)
}

fn json_vector<const N: usize>(
    value: Option<&serde_json::Value>,
    default: [f32; N],
) -> Result<[f32; N], String> {
    let Some(value) = value else {
        return Ok(default);
    };
    let array = value
        .as_array()
        .filter(|a| a.len() == N)
        .ok_or("walk transform vector has wrong length")?;
    let mut out = [0.0; N];
    for (i, v) in array.iter().enumerate() {
        let n = v
            .as_f64()
            .ok_or("walk transform contains non-numeric value")? as f32;
        if !n.is_finite() {
            return Err("walk transform is not finite".into());
        }
        out[i] = n;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gi_hud_reports_success_only_for_loaded_data() {
        assert_eq!(
            walk_gi_status(true, true, false, None),
            "GI loading… · G toggle"
        );
        assert_eq!(walk_gi_status(true, true, true, None), "GI ON · G toggle");
        assert_eq!(walk_gi_status(false, true, true, None), "GI OFF · G toggle");
        assert_eq!(
            walk_gi_status(true, false, false, None),
            "GI unavailable: no lighting asset"
        );
        assert_eq!(
            walk_gi_status(true, true, false, Some("asset missing")),
            "GI FAILED: asset missing"
        );
    }

    #[test]
    fn walk_world_uses_transformed_static_models_and_excludes_dynamic_models() {
        let root = std::env::temp_dir().join(format!(
            "amoris-walk-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        std::fs::write(root.join("project.toml"), "name = 'Walk fixture'\n").unwrap();
        std::fs::write(root.join("scene.json"), json!({"format":"amoris-scene", "version":1,
            "entities":[
                {"components":{"Model":{"mesh":"plane","scale":[20,1,20]}, "Transform":{"position":[0,0,0]}}},
                {"components":{"Model":{"mesh":"cube","scale":[1,2,1]}, "Transform":{"position":[0,1,0]}}},
                {"components":{"Model":{"mesh":"cube","scale":[10,10,10]}, "RigidBody":{"kind":"Dynamic"}}},
                {"components":{"Model":{"mesh":"cube","visible":false}}},
                {"components":{"Model":{"mesh":"cube","scale":[10,10,10],"color":[0,0,1,0.5]}, "Transform":{"position":[-2,1,0]}}}
            ]}).to_string()).unwrap();
        let project = Project::load(&root).unwrap();
        let world = load_walk_world(&project).unwrap();
        let mut player = WalkController::new([2.0, 1.65, 0.0]).unwrap();
        for _ in 0..120 {
            player
                .step(&world, 1.0 / 120.0, [-3.0, 0.0, 0.0], false)
                .unwrap();
        }
        let state = player.state();
        assert!(state.grounded);
        assert!(
            state.eye[0] > 0.79 && state.eye[0] < 0.87,
            "{:?}",
            state.eye
        );
        assert!((state.eye[1] - 1.65).abs() < 0.1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "requires an acquired Bistro project in AMORIS_WALK_SMOKE_PROJECT"]
    fn bistro_walk_spawn_and_movement_smoke() {
        // The full check runs ignored tests too (checks.md 6.2): without the project, it skips.
        let Some(path) = std::env::var_os("AMORIS_WALK_SMOKE_PROJECT") else {
            eprintln!("AMORIS_WALK_SMOKE_PROJECT is not set: skipped");
            return;
        };
        let project = Project::load(&PathBuf::from(path)).unwrap();
        let start = Instant::now();
        let world = load_walk_world(&project).unwrap();
        let load_ms = start.elapsed().as_secs_f64() * 1000.0;
        let mut player = WalkController::new([12.0, 2.1, 18.0]).unwrap();
        for _ in 0..120 {
            player.step(&world, 1.0 / 120.0, [0.0; 3], false).unwrap();
        }
        let settled = player.state();
        let velocity = Vec3::new(-12.0, 0.0, -18.0).normalize() * 3.0;
        let mut grounded_frames = 0;
        for _ in 0..120 {
            let state = player
                .step(&world, 1.0 / 60.0, velocity.to_array(), false)
                .unwrap();
            grounded_frames += u32::from(state.grounded);
        }
        let moved = player.state();
        let distance = Vec3::from(moved.eye).distance(Vec3::from(settled.eye));
        println!(
            "{}",
            json!({"triangles":world.triangle_count(), "collision_load_ms":load_ms,
            "spawn":[12.0,2.1,18.0], "settled_eye":settled.eye, "settled_grounded":settled.grounded,
            "moved_eye":moved.eye, "movement_distance":distance, "grounded_frames":grounded_frames,
            "movement_frames":120})
        );
        assert!(
            settled.grounded,
            "spawn did not reach solid ground: {settled:?}"
        );
        assert!(
            (settled.eye[0] - 12.0).abs() < 0.5 && (settled.eye[2] - 18.0).abs() < 0.5,
            "spawn was embedded in the model: {settled:?}"
        );
        assert!(
            distance > 1.0,
            "forward movement is blocked at spawn: {moved:?}"
        );
        assert!(
            grounded_frames >= 100,
            "walk left the solid street: {moved:?}"
        );
    }
}
