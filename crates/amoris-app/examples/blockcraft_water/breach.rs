//! Blockcraft's broken bank (docs/spec/water.md 14), shared by the `blockcraft_water` example of
//! amoris-app (the shots and the drawing's cost) and amoris-runtime's `water_bench` (the solver's,
//! the snapshot's and the feed's): a copy of the project whose script digs a pit beside the water at
//! tick [`DIG`] and breaks the bank between them at tick [`BREAK`], so the water floods the pit.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use amoris_runtime::{Game, Project};
use amoris_sim::voxels::Voxels;

/// The tick the pit is dug (the region has its tiles from tick 1).
pub const DIG: u64 = 2;
/// The tick the bank breaks: two seconds in, the water settled and asleep after the dig.
pub const BREAK: u64 = 120;
/// The sea's level in blockcraft: water fills up to the top of block y = SEA - 1.
const SEA: i32 = 30;

/// Where the bank breaks, in the volume's cells and in the world.
#[derive(Clone, Copy, Debug)]
pub struct Site {
    /// The water's column beside the bank (x, z of its first row).
    pub cell: [i32; 2],
    /// The breach's centre at the water's surface, world metres.
    pub world: [f64; 3],
}

const SCRIPT: &str = r#"
export const breach = system({
    name: "breach", phase: "update", doc: "Digs a pit beside the water, then breaks its bank.",
    queries: {},
    run(ctx) {
        const land = ctx.single("Land");
        if (ctx.tick === DIG) ctx.voxels.fill(land, [PX0, LOW, Z0], [PX1, 70, Z1], 0);
        if (ctx.tick === BREAK) ctx.voxels.fill(land, [BX, LOW, BZ0], [BX, 70, BZ1], 0);
    },
});
"#;

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("a directory");
    for e in std::fs::read_dir(from).expect("the project").flatten() {
        let path = e.path();
        let name = e.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            copy_dir(&path, &to.join(&name));
        } else {
            std::fs::copy(&path, to.join(&name)).expect("a file");
        }
    }
}

/// The highest solid block of column (x, z), or -1.
fn top(v: &Voxels, x: i32, z: i32) -> i32 {
    (0..80)
        .rev()
        .find(|&y| v.solid(v.get([x, y, z])))
        .unwrap_or(-1)
}

/// The first place, scanning x then z, where six columns of water along z border land for ten
/// columns along +x whose ground stands up to ten blocks over the water.
fn find(v: &Voxels) -> Option<[i32; 2]> {
    let water = v.block("water")?;
    for x in 4..110 {
        for z in 4..110 {
            let shore = (0..6).all(|dz| v.get([x, SEA - 1, z + dz]) == water);
            let land = (0..6).all(|dz| {
                (1..=10).all(|dx| {
                    let t = top(v, x + dx, z + dz);
                    (SEA - 1..=SEA + 10).contains(&t) && v.get([x + dx, SEA - 1, z + dz]) != water
                })
            });
            if shore && land {
                return Some([x, z]);
            }
        }
    }
    None
}

/// A copy of the project at `src` with the breach's system, and where it breaks.
pub fn prepare(src: &Path) -> (PathBuf, Site) {
    let p = Project::load(src).unwrap_or_else(|e| panic!("{e:#?}"));
    let setup = Arc::new(p.setup(false).unwrap_or_else(|e| panic!("{e:#?}")));
    let game = Game::new(setup, 1).unwrap_or_else(|e| panic!("{e:#?}"));
    let mut q = game.world().try_query::<&Voxels>().expect("a volume");
    let v = q.single(game.world()).expect("one volume");
    let [x, z] = find(v).expect("a shore with land beside it");
    let scene: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(src.join("scene.json")).expect("the scene"))
            .expect("JSON");
    let origin = scene["entities"]
        .as_array()
        .and_then(|es| es.iter().find(|e| e["components"]["Voxels"].is_object()))
        .and_then(|e| e["components"]["Transform"]["position"].as_array())
        .map_or([0.0; 3], |p| {
            [0, 1, 2].map(|i| p.get(i).and_then(serde_json::Value::as_f64).unwrap_or(0.0))
        });
    let root = std::env::temp_dir().join(format!("amoris-blockcraft-water-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    copy_dir(src, &root);
    let script = SCRIPT
        .replace("DIG", &DIG.to_string())
        .replace("BREAK", &BREAK.to_string())
        .replace("LOW", &(SEA - 6).to_string())
        .replace("PX0", &(x + 2).to_string())
        .replace("PX1", &(x + 9).to_string())
        .replace("BX", &(x + 1).to_string())
        .replace("BZ0", &(z + 1).to_string())
        .replace("BZ1", &(z + 4).to_string())
        .replace("Z0", &z.to_string())
        .replace("Z1", &(z + 5).to_string());
    let main = std::fs::read_to_string(root.join("scripts/main.ts")).expect("main.ts");
    let main = main.replacen("systems: [", "systems: [breach, ", 1);
    let at = main.find("export default").expect("the game's export");
    let main = format!("{}{script}\n{}", &main[..at], &main[at..]);
    std::fs::write(root.join("scripts/main.ts"), main).expect("main.ts");
    let site = Site {
        cell: [x, z],
        world: [
            origin[0] + f64::from(x) + 1.5,
            origin[1] + f64::from(SEA),
            origin[2] + f64::from(z) + 3.0,
        ],
    };
    (root, site)
}
