//! Block volumes end to end (docs/spec/voxels.md): a project loads its volume from a cells file,
//! scripts fill, set, read and ray-cast it through `ctx.voxels` with their own edits visible to
//! their later reads, a throwing system's edits are discarded, a walker lands on the blocks, and the
//! world hash changes with one cell and not without edits.
#![cfg(feature = "transpile")]
mod common;

use std::path::PathBuf;
use std::sync::Arc;

use amoris_link::Source;
use amoris_runtime::voxel_file::write_cells;
use amoris_runtime::{Command, Game, Project};
use amoris_sim::voxels::{CHUNK_CELLS, Cells, Chunk, Voxels};
use serde_json::{Value, json};

const MAIN: &str = r#"
import { component, field, game, system } from "amoris";

export const Land = component("Land", { version: 1, doc: "The volume.", fields: {} });
export const Probe = component("Probe", {
    version: 1, doc: "What the systems saw.",
    fields: {
        staged: field.i32(-1, "A cell read after this call set it."),
        hit_y: field.i32(-99, "The cell a ray down hit."),
        hit_normal_y: field.i32(-99, "The face it entered by."),
        loaded: field.i32(-1, "A cell from the cells file."),
        stone: field.i32(-1, "The stone's palette index."),
    },
});

export const build = system({
    name: "build", phase: "update", doc: "Fills a floor, then reads and casts against it.",
    when: "start",
    run(ctx) {
        const v = ctx.single("Land");
        const p = ctx.single("Probe");
        const stone = ctx.voxels.block(v, "stone");
        ctx.voxels.fill(v, [0, 4, 0], [7, 4, 7], stone);
        ctx.voxels.set(v, 2, 5, 2, stone);
        const hit = ctx.voxels.raycast(v, [2.5, 9.5, 2.5], [0, -1, 0], 20, { solidOnly: true });
        ctx.world.set(p, "Probe", {
            staged: ctx.voxels.get(v, 2, 5, 2),
            hit_y: hit === null ? -1 : hit.cell[1],
            hit_normal_y: hit === null ? -1 : hit.normal[1],
            loaded: ctx.voxels.get(v, 0, 0, 0),
            stone,
        });
        const hud = ctx.single("UiText");
        ctx.world.set(hud, "UiText", { text: `stone is ${stone}`, color: { x: 1, y: 0.5, z: 0, w: 1 } });
    },
});

export const boom = system({
    name: "boom", phase: "update", doc: "Edits, then throws: nothing applies.",
    when: { every: 2 },
    run(ctx) {
        const v = ctx.single("Land");
        ctx.voxels.set(v, 7, 7, 7, 1);
        throw new Error("boom");
    },
});

export default game({ components: [Land, Probe], systems: [build, boom] });
"#;

fn project() -> PathBuf {
    let root = std::env::temp_dir().join(format!("amoris-voxels-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    std::fs::write(
        root.join("project.toml"),
        "name = 'voxels'\nrate = 60\nseed = 1\n",
    )
    .unwrap();
    std::fs::write(root.join("scripts/main.ts"), MAIN).unwrap();
    let mut cells = vec![0u8; CHUNK_CELLS];
    cells[0] = 2; // dirt at (0, 0, 0)
    std::fs::write(
        root.join("land.cells"),
        write_cells(&[Chunk {
            key: [0, 0, 0],
            cells: Cells(cells.into_boxed_slice()),
            edited: 0,
            states: None,
        }]),
    )
    .unwrap();
    let scene = json!({"format": "amoris-scene", "version": 1, "entities": [
        {"name": "Land", "components": {
            "Transform": {"position": [0, 0, 0]},
            "Land": {},
            "Voxels": {"source": "land.cells", "palette": [
                {"name": "air"},
                {"name": "stone", "solid": true, "faces": ["b#stone", "b#stone", "b#stone"]},
                {"name": "dirt", "solid": true, "faces": ["b#dirt", "b#dirt", "b#dirt"]}]}}},
        {"name": "Probe", "components": {"Probe": {}}},
        {"name": "Hud", "components": {"UiText": {"text": "", "anchor": "bottom_left"}}},
        {"name": "Steve", "components": {
            "Transform": {"position": [3.5, 9.0, 3.5]},
            "Walker": {}}}
    ]});
    std::fs::write(root.join("scene.json"), scene.to_string()).unwrap();
    root
}

fn get(g: &mut Game, entity: &str, fields: &[&str]) -> Value {
    g.apply(&Command::new(
        Source::Developer(9),
        1,
        "world.get",
        json!({"entity": entity, "fields": fields}),
    ))
    .unwrap_or_else(|e| panic!("{e:#?}"))["fields"]
        .clone()
}

fn volume(g: &Game) -> Voxels {
    let mut q = g.world().try_query::<&Voxels>().unwrap();
    q.single(g.world()).unwrap().clone()
}

#[test]
fn scripts_edit_volumes_and_a_walker_lands_on_them() {
    common::big_stack(|| {
        let root = project();
        let p = Project::load(&root).unwrap_or_else(|e| panic!("{e:#?}"));
        let setup = Arc::new(p.setup(false).unwrap_or_else(|e| panic!("{e:#?}")));
        let mut g = Game::new(setup, 1).unwrap_or_else(|e| panic!("{e:#?}"));
        let before = g.world_hash().unwrap();
        // The render feed: a full frame sends the loaded chunk, the build's tick its changes.
        let feed = amoris_assets::Feed::new();
        let viewport = feed.subscribe();
        let mut x = amoris_runtime::Extractor::new();
        g.present(&mut x, &feed);
        let full = viewport.take();
        assert_eq!(full.voxels.len(), 1);
        assert_eq!(full.voxels[0].chunks.len(), 1);
        assert_eq!(full.voxels[0].blocks.as_ref().map(Vec::len), Some(3));
        g.step().unwrap();
        g.present(&mut x, &feed);
        let delta = viewport.take();
        assert_eq!(delta.voxels.len(), 1, "{:?}", delta.voxels.len());
        assert!(
            delta.voxels[0].blocks.is_none(),
            "the palette did not change"
        );
        assert_eq!(
            delta.voxels[0].chunks.len(),
            1,
            "the floor is in the loaded chunk 0"
        );
        assert_eq!(delta.voxels[0].chunks[0].key, [0, 0, 0]);
        g.step().unwrap();
        g.present(&mut x, &feed);
        assert!(
            viewport.take().voxels.is_empty(),
            "a tick without edits sends no volume"
        );
        let probe = get(
            &mut g,
            "Probe",
            &[
                "Probe.staged",
                "Probe.hit_y",
                "Probe.hit_normal_y",
                "Probe.loaded",
                "Probe.stone",
            ],
        );
        assert_eq!(probe["Probe.stone"], 1);
        assert_eq!(probe["Probe.staged"], 1, "a read sees the call's own edit");
        assert_eq!(
            probe["Probe.hit_y"], 5,
            "the ray met the staged block first"
        );
        assert_eq!(probe["Probe.hit_normal_y"], 1);
        assert_eq!(probe["Probe.loaded"], 2, "the cells file's block");
        let hud = get(&mut g, "Hud", &["UiText.text", "UiText.color"]);
        assert_eq!(hud["UiText.text"], "stone is 1", "scripts write UiText");
        assert_eq!(hud["UiText.color"], json!([1.0, 0.5, 0.0, 1.0]), "{hud}");
        let v = volume(&g);
        assert_eq!(v.get([7, 4, 7]), 1);
        assert_eq!(
            v.get([7, 7, 7]),
            0,
            "the throwing system's edit was discarded"
        );
        assert_ne!(g.world_hash().unwrap(), before);

        // The walker falls from 9 m onto the floor at y 4 (its top at 5).
        for _ in 0..90 {
            g.step().unwrap();
        }
        let eye = get(
            &mut g,
            "Steve",
            &["Transform.position.1", "Walker.grounded"],
        );
        assert_eq!(eye["Walker.grounded"], true);
        let y = eye["Transform.position.1"].as_f64().unwrap();
        assert!((y - (5.0 + 1.62)).abs() < 1e-5, "eye {y}");
        assert_eq!(volume(&g).get([7, 7, 7]), 0);

        // Without edits the volume's section stays as it was.
        let a = volume(&g);
        g.step().unwrap();
        assert_eq!(volume(&g), a);
        std::fs::remove_dir_all(root).unwrap();
    });
}

const DIG: &str = r#"
import { component, game, system } from "amoris";

export const Land = component("Land", { version: 1, doc: "The volume.", fields: {} });

export const dig = system({
    name: "dig", phase: "update", doc: "At tick 90, digs through the floor under the first box.",
    run(ctx) {
        if (ctx.tick !== 90) return;
        ctx.voxels.fill(ctx.single("Land"), [6, 0, 6], [9, 2, 9], 0);
    },
});

export default game({ components: [Land], systems: [dig] });
"#;

/// A floor of stone, 3 cells deep, across chunks 0 and 1 along x; boxes over cell 8 and over the
/// chunk border at cell 16.
fn bodies_project() -> PathBuf {
    let root = std::env::temp_dir().join(format!("amoris-voxels-bodies-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    std::fs::write(
        root.join("project.toml"),
        "name = 'voxel-bodies'\nrate = 60\nseed = 1\n",
    )
    .unwrap();
    std::fs::write(root.join("scripts/main.ts"), DIG).unwrap();
    let mut land = Voxels::default();
    land.palette.push(amoris_sim::voxels::Block {
        name: "stone".into(),
        solid: true,
        faces: ["b#stone".into(), "b#stone".into(), "b#stone".into()],
        ..Default::default()
    });
    land.fill([0, 0, 0], [31, 2, 15], 1, 0).unwrap();
    std::fs::write(root.join("land.cells"), write_cells(&land.chunks)).unwrap();
    let body = |name: &str, x: f64| {
        json!({"name": name, "components": {
            "Transform": {"position": [x, 5.0, 8.0]},
            "Velocity": {},
            "RigidBody": {"kind": "Dynamic"},
            "Collider": {"shape": {"Cuboid": {"half_extents": [0.5, 0.5, 0.5]}}, "density": 1000.0}}})
    };
    let scene = json!({"format": "amoris-scene", "version": 1, "entities": [
        {"name": "Land", "components": {
            "Transform": {"position": [0, 0, 0]},
            "Land": {},
            "Voxels": {"source": "land.cells", "palette": [
                {"name": "air"},
                {"name": "stone", "solid": true, "faces": ["b#stone", "b#stone", "b#stone"]}]}}},
        body("Over", 8.0),
        body("Border", 16.0)
    ]});
    std::fs::write(root.join("scene.json"), scene.to_string()).unwrap();
    root
}

fn height(g: &mut Game, entity: &str) -> f64 {
    get(g, entity, &["Transform.position.1"])["Transform.position.1"]
        .as_f64()
        .unwrap()
}

/// voxels.md 5.1 through the engine's own snapshots: Rapier boxes land on a volume loaded from a
/// cells file (one across a chunk border); a fork, and a restore made after the original world is
/// gone, step with the original's world hashes tick for tick through the script's dig, which drops
/// the box over it through the floor.
#[test]
fn bodies_land_on_volumes_through_snapshots_and_forks() {
    common::big_stack(|| {
        let root = bodies_project();
        let p = Project::load(&root).unwrap_or_else(|e| panic!("{e:#?}"));
        let setup = Arc::new(p.setup(false).unwrap_or_else(|e| panic!("{e:#?}")));
        let mut g = Game::new(setup.clone(), 1).unwrap_or_else(|e| panic!("{e:#?}"));
        for _ in 0..80 {
            g.step().unwrap();
        }
        for name in ["Over", "Border"] {
            let y = height(&mut g, name);
            assert!((y - 3.5).abs() < 0.02, "{name} rests at {y}");
        }
        let snap = g.snapshot().unwrap();
        let mut fork = g.fork().unwrap();
        let mut want = Vec::new();
        for _ in 0..60 {
            g.step().unwrap();
            fork.step().unwrap();
            want.push(g.world_hash().unwrap());
            assert_eq!(fork.world_hash().unwrap(), *want.last().unwrap());
        }
        assert!(height(&mut g, "Over") < 2.0, "the box fell into the hole");
        assert!((height(&mut g, "Border") - 3.5).abs() < 0.02);
        drop((g, fork));

        let mut cold = Game::new(setup, 1).unwrap_or_else(|e| panic!("{e:#?}"));
        cold.restore(&snap).unwrap();
        assert_eq!(cold.world_hash().unwrap(), snap.world_hash());
        for (k, h) in want.iter().enumerate() {
            cold.step().unwrap();
            assert_eq!(cold.world_hash().unwrap(), *h, "tick 80 + {}", k + 1);
        }
        std::fs::remove_dir_all(root).unwrap();
    });
}

const HANDS: &str = r#"
import { component, game, system } from "amoris";

export const Land = component("Land", { version: 1, doc: "The volume.", fields: {} });

export const hands = system({
    name: "hands", phase: "update", doc: "Breaks the looked-at block, or places one against it.",
    queries: {
        players: {
            with: ["Walker", "Transform"],
            fields: ["Walker.primary", "Walker.secondary", "Walker.yaw", "Walker.pitch",
                     "Walker.slot", "Transform.position"],
        },
    },
    run(ctx, { players }) {
        const v = ctx.single("Land");
        const w = players.cols.Walker;
        const p = players.cols.Transform.position;
        for (let r = 0; r < players.len; r++) {
            if (w.primary[r] === 0 && w.secondary[r] === 0) continue;
            const cp = Math.cos(w.pitch[r]);
            const dir = [-Math.sin(w.yaw[r]) * cp, Math.sin(w.pitch[r]), -Math.cos(w.yaw[r]) * cp] as const;
            const hit = ctx.voxels.raycast(v, [p.x[r], p.y[r], p.z[r]], dir, 5, { solidOnly: true });
            if (hit === null) continue;
            const [x, y, z] = hit.cell;
            if (w.primary[r] !== 0) {
                ctx.voxels.set(v, x, y, z, 0);
            } else {
                const [nx, ny, nz] = hit.normal;
                ctx.voxels.set(v, x + nx, y + ny, z + nz, w.slot[r] + 1);
            }
        }
    },
});

export default game({ components: [Land], systems: [hands] });
"#;

const PERCEPTION: &str = r#"{
  "observers": [{"name": "player", "doc": "A first-person player.",
    "sight": {"range_m": 64, "fov_deg": 90, "eye_m": {"x": 0, "y": 0, "z": 0}, "occlusion": false},
    "attention_m": 16, "memory_s": 60, "memory_capacity": 16, "event_capacity": 16,
    "budget_tokens": 200, "instruments": []}],
  "instruments": [], "kinds": [], "events": []
}"#;

fn player_project() -> PathBuf {
    player_project_in("player")
}

fn player_project_in(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("amoris-voxels-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    std::fs::write(
        root.join("project.toml"),
        "name = 'hands'\nrate = 60\nseed = 1\n[player]\nactions = 'first_person'\n",
    )
    .unwrap();
    std::fs::write(root.join("scripts/main.ts"), HANDS).unwrap();
    std::fs::write(root.join("perception.json"), PERCEPTION).unwrap();
    let mut land = Voxels::default();
    for name in ["stone", "dirt"] {
        land.palette.push(amoris_sim::voxels::Block {
            name: name.into(),
            solid: true,
            faces: ["b#x".into(), "b#x".into(), "b#x".into()],
            ..Default::default()
        });
    }
    land.fill([-8, -1, -8], [8, 0, 8], 1, 0).unwrap();
    let scene = json!({"format": "amoris-scene", "version": 1, "entities": [
        {"name": "Land", "components": {"Land": {}, "Voxels": land}},
        {"name": "Steve", "components": {
            "Transform": {"position": [0.5, 2.62, 0.5]},
            "Walker": {},
            "Observer": {"profile": "player", "seat": "player"}}}
    ]});
    std::fs::write(root.join("scene.json"), scene.to_string()).unwrap();
    root
}

#[test]
fn a_player_breaks_and_places_blocks_and_the_run_replays() {
    use amoris_persist::replay::{MemorySink, RecordOptions, Replay};
    common::big_stack(|| {
        let root = player_project();
        let p = Project::load(&root).unwrap_or_else(|e| panic!("{e:#?}"));
        let setup = Arc::new(p.setup(false).unwrap_or_else(|e| panic!("{e:#?}")));
        let mut g = Game::new(setup, 1).unwrap_or_else(|e| panic!("{e:#?}"));
        let sink = MemorySink::new();
        g.record(Box::new(sink.clone()), RecordOptions::default())
            .unwrap();
        let mut seq = 0;
        let mut act = |g: &mut Game, actions: Value| {
            seq += 1;
            g.apply(&Command::new(
                Source::Player(0),
                seq,
                "player.act",
                json!({ "actions": actions }),
            ))
            .unwrap_or_else(|e| panic!("{e:#?}"))
        };
        for _ in 0..30 {
            g.step().unwrap();
        }
        // Standing at (0.5, 0.5), eye 2.62 m up: look 0.8 rad down along -z at the cell two ahead.
        let aim = json!({"pitch": -0.8, "yaw": 0.0});
        act(
            &mut g,
            json!([{"do": "set", "controls": aim}, {"do": "pulse", "control": "primary"}]),
        );
        g.step().unwrap();
        let v = volume(&g);
        assert_eq!(v.get([0, 0, -2]), 0, "broken");
        assert_eq!(
            (v.get([0, 0, -1]), v.get([0, 0, 0])),
            (1, 1),
            "only that one"
        );
        let w = get(&mut g, "Steve", &["Walker.primary", "Walker.pitch"]);
        assert_eq!(w["Walker.primary"], false, "a pulse acts for one tick");
        assert_eq!(w["Walker.pitch"], -0.8);
        g.step().unwrap();
        assert_eq!(
            volume(&g).get([0, 0, -2]),
            0,
            "no second break without a pulse"
        );
        // The same aim now meets the hole's floor: dirt (slot 1) goes back on its top face.
        act(
            &mut g,
            json!([{"do": "set", "controls": {"slot": "1"}}, {"do": "pulse", "control": "secondary"}]),
        );
        g.step().unwrap();
        assert_eq!(volume(&g).get([0, 0, -2]), 2, "placed dirt");
        g.finish_recording().unwrap();
        let replay = Replay::read(&sink.bytes()).unwrap();
        let mut fresh = Game::for_replay(&replay).unwrap();
        let v = amoris_persist::replay::verify(
            &replay,
            &mut fresh,
            amoris_persist::replay::VerifyOptions::mode(amoris_persist::replay::ReplayMode::Verify),
        );
        assert_eq!(
            v.outcome,
            amoris_persist::replay::ReplayOutcome::Identical,
            "{v:#?}"
        );
        std::fs::remove_dir_all(root).unwrap();
    });
}

/// A gamepad session (docs/spec/input.md 1): the standard pad's sticks and buttons go through the
/// project's input map and the seat to `player.act`, as the native window and the page send them.
/// The press that starts play does not jump; the left stick walks, the right stick turns, South
/// jumps and R1 steps the hotbar. The recording names only controls, so it replays identically,
/// and the same session played twice ends in the same world.
#[test]
fn a_gamepad_session_moves_the_walker_and_replays() {
    use amoris_assets::input::{DeviceFrame, LOOK_RAD_PER_COUNT, Pad, Seat};
    use amoris_persist::replay::{MemorySink, RecordOptions, Replay};
    common::big_stack(|| {
        let root = player_project_in("gamepad");
        let p = Project::load(&root).unwrap_or_else(|e| panic!("{e:#?}"));
        let setup = Arc::new(p.setup(false).unwrap_or_else(|e| panic!("{e:#?}")));
        let map = p.manifest.player.as_ref().unwrap().input_map().unwrap();
        // The pad at frame `n`: South held while play starts, then the left stick up, the right
        // stick part right, South pressed again, R1 pressed.
        let pad = |n: u32| {
            let mut buttons = vec![0.0f32; 17];
            let mut axes = vec![0.0f32; 4];
            if n < 3 || (45..48).contains(&n) {
                buttons[0] = 1.0;
            }
            if (55..58).contains(&n) {
                buttons[5] = 1.0;
            }
            if (3..40).contains(&n) {
                axes[1] = -1.0;
            }
            if (10..20).contains(&n) {
                axes[2] = 0.6;
            }
            Pad { buttons, axes }
        };
        let play = |sink: Option<MemorySink>| {
            let mut g = Game::new(setup.clone(), 1).unwrap_or_else(|e| panic!("{e:#?}"));
            if let Some(s) = &sink {
                g.record(Box::new(s.clone()), RecordOptions::default())
                    .unwrap();
            }
            for _ in 0..30 {
                g.step().unwrap();
            }
            let start = get(&mut g, "Steve", &["Transform.position"])["Transform.position"].clone();
            let mut seat = Seat::new(map.clone());
            let (mut yaw, mut pitch) = (0.0f32, 0.0f32);
            let (mut seq, mut top) = (0, f64::MIN);
            for n in 0..90 {
                let frame = DeviceFrame {
                    pad: Some(pad(n)),
                    dt_s: 1.0 / 60.0,
                    ..Default::default()
                };
                let input = seat.frame(&frame, n >= 2);
                yaw -= input.look[0] * LOOK_RAD_PER_COUNT;
                pitch = (pitch - input.look[1] * LOOK_RAD_PER_COUNT).clamp(-1.55, 1.55);
                let actions = seat.acts(&input, yaw, pitch);
                if !actions.is_empty() {
                    seq += 1;
                    g.apply(&Command::new(
                        Source::Player(0),
                        seq,
                        "player.act",
                        json!({ "actions": actions }),
                    ))
                    .unwrap_or_else(|e| panic!("{e:#?}"));
                }
                g.step().unwrap();
                let y = get(&mut g, "Steve", &["Transform.position"])["Transform.position"][1]
                    .as_f64()
                    .unwrap();
                if n < 45 {
                    // On flat ground: the press that started play did not jump.
                    let ground = start[1].as_f64().unwrap();
                    assert!((y - ground).abs() < 0.01, "frame {n}: {y} against {ground}");
                } else {
                    top = amoris_sim::math::max(top, y);
                }
            }
            (g, start, top)
        };
        let sink = MemorySink::new();
        let (mut g, start, top) = play(Some(sink.clone()));
        let w = get(
            &mut g,
            "Steve",
            &[
                "Transform.position",
                "Walker.yaw",
                "Walker.slot",
                "Walker.forward",
            ],
        );
        let at = &w["Transform.position"];
        let walked = amoris_sim::math::hypot(
            at[0].as_f64().unwrap() - start[0].as_f64().unwrap(),
            at[2].as_f64().unwrap() - start[2].as_f64().unwrap(),
        );
        assert!(walked > 1.0, "walked {walked} m");
        let yaw = w["Walker.yaw"].as_f64().unwrap();
        assert!(yaw < -0.1, "turned right: {yaw}");
        assert!(top > start[1].as_f64().unwrap() + 0.1, "jumped: {top}");
        assert_eq!(w["Walker.slot"], 1, "R1 stepped the hotbar");
        assert_eq!(w["Walker.forward"], 0.0, "the stick let go");
        g.finish_recording().unwrap();
        let replay = Replay::read(&sink.bytes()).unwrap();
        let mut fresh = Game::for_replay(&replay).unwrap();
        let v = amoris_persist::replay::verify(
            &replay,
            &mut fresh,
            amoris_persist::replay::VerifyOptions::mode(amoris_persist::replay::ReplayMode::Verify),
        );
        assert_eq!(
            v.outcome,
            amoris_persist::replay::ReplayOutcome::Identical,
            "{v:#?}"
        );
        let (again, _, _) = play(None);
        assert_eq!(g.world_hash().unwrap(), again.world_hash().unwrap());
        std::fs::remove_dir_all(root).unwrap();
    });
}

/// A project's `[player.bindings]` and `[player.touch]` (docs/spec/input.md 4) load with the
/// project, and a wrong name refuses the project at its pointer.
#[test]
fn a_projects_input_map_loads_and_is_checked() {
    let root = player_project_in("bindings");
    let manifest = |extra: &str| {
        std::fs::write(
            root.join("project.toml"),
            format!(
                "name = 'hands'\nrate = 60\nseed = 1\n[player]\nactions = 'first_person'\n\
                 buttons = [{{ name = 'dodge', keys = ['KeyQ', 'PadEast'] }}]\n{extra}"
            ),
        )
        .unwrap();
        Project::load(&root)
    };
    let p = manifest(
        "[player.bindings]\njump = ['PadNorth']\n[player.touch]\nbuttons = [{ label = 'Roll', input = 'dodge' }]\n",
    )
    .unwrap_or_else(|e| panic!("{e:#?}"));
    let map = p.manifest.player.unwrap().input_map().unwrap();
    assert_eq!(map.bindings["jump"], ["PadNorth"]);
    assert_eq!(map.buttons[0].keys, ["KeyQ", "PadEast"]);
    assert_eq!(map.touch.buttons[0].label, "Roll");
    let e = manifest("[player.bindings]\njump = ['PadA']\n").unwrap_err();
    assert_eq!(e.code, "request.invalid_value");
    assert_eq!(e.detail["path"], "/player/bindings/jump/0");
    let e = manifest("[player.touch]\nbuttons = [{ label = 'x', input = 'dash' }]\n").unwrap_err();
    assert_eq!(e.detail["path"], "/player/touch/buttons/0/input");
    std::fs::remove_dir_all(root).unwrap();
}

/// `player.blocks` (voxels.md 11.1): the seat sees the surface cells its rays reach over its field
/// of view and within range, each once, nearest first within the budget, never a cell behind
/// another; the cell it looks at is the one its hands would break. A developer naming the seat
/// reads the same bytes; a player reads no other seat.
#[test]
fn a_seat_sees_the_surface_cells_its_rays_reach() {
    common::big_stack(|| {
        let root = player_project_in("blocks");
        let p = Project::load(&root).unwrap_or_else(|e| panic!("{e:#?}"));
        let setup = Arc::new(p.setup(false).unwrap_or_else(|e| panic!("{e:#?}")));
        let mut g = Game::new(setup, 1).unwrap_or_else(|e| panic!("{e:#?}"));
        let mut seq = 0;
        let mut call = |g: &mut Game, source: Source, name: &str, params: Value| {
            seq += 1;
            g.apply(&Command::new(source, seq, name, params))
        };
        for _ in 0..30 {
            g.step().unwrap();
        }
        let aim = json!({"actions": [{"do": "set", "controls": {"pitch": -0.8, "yaw": 0.0}}]});
        call(&mut g, Source::Player(0), "player.act", aim).unwrap();
        g.step().unwrap();
        let seen = call(
            &mut g,
            Source::Player(0),
            "player.blocks",
            json!({"budget_tokens": 8000}),
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        // The cell two ahead, where the hands test breaks one, on its top face.
        assert_eq!(seen["looking_at"]["block"], json!("stone"), "{seen:#}");
        assert_eq!(
            seen["looking_at"]["at"],
            json!([0.5, 0.5, -1.5]),
            "{seen:#}"
        );
        assert_eq!(seen["looking_at"]["face"], json!("+y"), "{seen:#}");
        let blocks = seen["blocks"].as_array().unwrap();
        assert!(blocks.len() > 20, "{seen:#}");
        assert!(seen.get("omitted").is_none(), "{seen:#}");
        let mut last = 0.0;
        let mut cells = std::collections::BTreeSet::new();
        for b in blocks {
            let d = b["distance_m"].as_f64().unwrap();
            assert!(d >= last && d <= 16.0, "nearest first, within range: {b}");
            last = d;
            assert!(cells.insert(b["at"].to_string()), "each cell once: {b}");
            // The floor's lower layer lies under its top one: no ray reaches it.
            assert_eq!(b["at"][1], json!(0.5), "{b}");
            assert_eq!(b["face"], json!("+y"), "{b}");
            // Ahead of the eye, within the 90 degree field of view about -z.
            let (x, z) = (b["at"][0].as_f64().unwrap(), b["at"][2].as_f64().unwrap());
            assert!(z < 0.5 + 1.0 && (x - 0.5).abs() <= (0.5 - z) + 1.0, "{b}");
        }
        // A smaller budget keeps the nearest and counts the rest; a shorter range sees less.
        let small = call(
            &mut g,
            Source::Player(0),
            "player.blocks",
            json!({"budget_tokens": 120}),
        )
        .unwrap();
        let kept = small["blocks"].as_array().unwrap();
        assert!(!kept.is_empty() && kept.len() < blocks.len(), "{small:#}");
        assert_eq!(kept[..], blocks[..kept.len()], "the nearest first");
        assert_eq!(
            small["omitted"].as_u64().unwrap() as usize,
            blocks.len() - kept.len()
        );
        assert!(small["tokens"].as_u64().unwrap() <= 120, "{small:#}");
        let near = call(
            &mut g,
            Source::Player(0),
            "player.blocks",
            json!({"range_m": 3}),
        )
        .unwrap();
        assert!(
            near["blocks"]
                .as_array()
                .unwrap()
                .iter()
                .all(|b| b["distance_m"].as_f64().unwrap() <= 3.0)
        );
        // The text projection says the same.
        let text = call(
            &mut g,
            Source::Player(0),
            "player.blocks",
            json!({"projection": "text", "budget_tokens": 8000}),
        )
        .unwrap();
        let text = text.as_str().unwrap();
        assert!(
            text.starts_with("blocks tick 31 seat player eye 0.5,2.62,0.5"),
            "{text}"
        );
        assert!(
            text.contains("\nlooking_at stone 0.5,0.5,-1.5 +y "),
            "{text}"
        );
        assert_eq!(text.lines().count(), 2 + blocks.len(), "{text}");
        // A developer naming the seat reads what the player reads; a player names no other seat.
        let dev = call(
            &mut g,
            Source::Developer(0),
            "player.blocks",
            json!({"seat": "player", "budget_tokens": 8000}),
        )
        .unwrap();
        assert_eq!(dev, seen);
        let e = call(
            &mut g,
            Source::Player(0),
            "player.blocks",
            json!({"seat": "other"}),
        )
        .unwrap_err();
        assert_eq!(e.code, "seat.not_yours", "{e:#?}");
        // A read: the world does not change.
        let before = g.world_hash().unwrap();
        call(&mut g, Source::Player(0), "player.blocks", json!({})).unwrap();
        assert_eq!(g.world_hash().unwrap(), before);
        std::fs::remove_dir_all(root).unwrap();
    });
}
