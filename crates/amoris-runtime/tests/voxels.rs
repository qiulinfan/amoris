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
        g.step().unwrap();
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
