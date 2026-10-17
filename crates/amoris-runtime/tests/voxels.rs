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
    let root = std::env::temp_dir().join(format!("amoris-voxels-player-{}", std::process::id()));
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
