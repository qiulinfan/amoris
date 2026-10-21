//! Native f64 bed values from actual Game initialization, for the independent browser ray check.
#[path = "../src/water_compute.rs"]
mod water_compute;
use amoris_link::Source;
use amoris_runtime::{Command, Project};
use serde_json::{Value, json};
use std::sync::Arc;
fn run() {
    let args = std::env::args().collect::<Vec<_>>();
    let out = args
        .windows(2)
        .find(|a| a[0] == "--out")
        .map_or("out/water-collider-bed/rays.json", |a| a[1].as_str());
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/water-slope");
    let base = Project::load(&root).unwrap().setup(false).unwrap();
    let mut cases = Vec::new();
    for name in ["rotated-box", "triangulated-saddle", "compound", "no-hit"] {
        let mut setup = base.clone();
        setup.scene.entities.retain(|e| {
            !e.components.contains_key("Floater") && !e.components.contains_key("WaterProbe")
        });
        let r = &mut setup.scene.entities[1].components["WaterRegion"];
        r["solver"] = json!("cpu");
        r["size"] = json!([16, 16]);
        r["cell_m"] = json!(0.25);
        r["tiles"] = json!([]);
        r["edges"] = json!(["Wall", "Wall", "Wall", "Wall"]);
        r["bed"] = json!({"Colliders":{"top":2,"bottom":-1}});
        let terrain = &mut setup.scene.entities[0].components;
        match name {
            "rotated-box" => {}
            "triangulated-saddle" => {
                let vertices = (0..17)
                    .flat_map(|z| {
                        (0..17).map(move |x| {
                            let x = x as f64 * 0.25;
                            let z = z as f64 * 0.25;
                            [x, 0.125 * x + 0.0625 * z + 0.015625 * x * z, z]
                        })
                    })
                    .collect::<Vec<_>>();
                let mut indices = Vec::<[u32; 3]>::new();
                for z in 0..16 {
                    for x in 0..16 {
                        let a = x + 17 * z;
                        let b = a + 1;
                        let c = a + 17;
                        let d = c + 1;
                        if (x + z) % 2 == 0 {
                            indices.extend([[a, c, d], [a, d, b]]);
                        } else {
                            indices.extend([[a, c, b], [b, c, d]]);
                        }
                    }
                }
                terrain["Transform"] = json!({"position":[0,0,0]});
                terrain["Collider"] =
                    json!({"shape":{"TriMesh":{"vertices":vertices,"indices":indices}}});
            }
            "compound" => {
                terrain["Transform"] = json!({"position":[0,0,0]});
                terrain["Collider"] = json!({"shape":{"Compound":{"parts":[{"position":[2,-1,2],"rotation":[0,0,0,1],"shape":{"Ball":{"radius":1.25}}},{"position":[2,0.5,2],"rotation":[0,0,0,1],"shape":{"Cuboid":{"half_extents":[0.5,0.1,0.5]}}}]}}});
            }
            _ => {
                terrain["Transform"] = json!({"position":[20,0,20]});
            }
        }
        let scene = serde_json::to_value(&setup.scene).unwrap();
        let mut g = water_compute::builder(Arc::new(setup), 1).build().unwrap();
        let report = g.step().unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        let fields = g
            .apply(&Command::new(
                Source::Developer(0),
                1,
                "world.get",
                json!({"entity":"Shore","fields":["WaterRegion","WaterColliderBed"]}),
            ))
            .unwrap()["fields"]
            .clone();
        let heights = fields["WaterRegion"]["tiles"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|t| t["bed"].as_array().unwrap().iter().cloned())
            .collect::<Vec<Value>>();
        cases.push(json!({"name":name,"scene":scene,"expected_bed":heights,"expected_source":fields["WaterColliderBed"]}));
    }
    let path = std::path::Path::new(out);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path,serde_json::to_vec_pretty(&json!({"scope":"actual Game CPU collider-bed initialization; f64 values are queried f32 hit coordinates","cases":cases})).unwrap()).unwrap();
    println!("collider-bed rays: {}", path.display());
}
fn main() {
    std::thread::Builder::new()
        .stack_size(amoris_runtime::GAME_STACK_BYTES)
        .spawn(run)
        .unwrap()
        .join()
        .unwrap();
}
