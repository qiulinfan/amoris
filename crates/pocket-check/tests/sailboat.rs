//! Slice 1's acceptance on the sailing sample (charter 10; master's `sailboat` task,
//! tools/scripts/agent_eval.py `sailboat_check`): a sea at height 0, a steady wind of 6 m/s toward
//! +x, a sloop with sail and rudder and no engine, bow toward +x with its sail set; sailing before
//! the wind, 300 ticks (five seconds) carry it at least 5 units along +x, and it is afloat. Run for
//! every seed of the sample's `check.toml`, through commands as an agent would read the world.
#![cfg(feature = "native")]
mod common;

use pocket_link::Source;
use pocket_runtime::{Command, Game};
use serde_json::{Value, json};

fn get(g: &mut Game, entity: &str, component: &str) -> Value {
    let r = g
        .apply(&Command::new(
            Source::Developer(0),
            1,
            "world_get",
            json!({"entity": entity, "components": [component]}),
        ))
        .unwrap_or_else(|e| panic!("{entity}.{component}: {e:#?}"));
    r["components"][component].clone()
}

#[test]
fn the_sloop_sails_before_the_wind() {
    common::big_stack(|| {
        let s = common::subject("samples/sailing");
        for &seed in &s.config.run.seeds {
            let mut g = s.game(seed).unwrap();
            // The wind blows toward +x at 6 m/s, steadily; the sea's surface is at 0.
            let wind = get(&mut g, "Breeze", "Wind");
            assert_eq!(
                (wind["from_deg"].clone(), wind["speed"].clone()),
                (json!(270.0), json!(6.0))
            );
            assert_eq!(wind["gust"], json!(0.0));
            assert_eq!(get(&mut g, "Sea", "Sea")["level"], json!(0.0));
            // The Sloop: bow toward +x, its sail set, driven by nothing but the wind.
            g.step().unwrap();
            let boat = get(&mut g, "Sloop", "Boat");
            assert!(
                (boat["heading_deg"].as_f64().unwrap() - 90.0).abs() < 1.0,
                "{boat}"
            );
            assert_eq!(boat["hoist"], json!(1.0));
            assert!(
                boat.get("throttle").is_none(),
                "the Boat has no engine: {boat}"
            );
            let x0 = get(&mut g, "Sloop", "Transform")["position"][0]
                .as_f64()
                .unwrap();
            for _ in 0..300 {
                g.step().unwrap();
            }
            let at = get(&mut g, "Sloop", "Transform")["position"].clone();
            let run = at[0].as_f64().unwrap() - x0;
            let boat = get(&mut g, "Sloop", "Boat");
            println!(
                "seed {seed}: {run:.2} along +x in 300 ticks, afloat {}",
                boat["afloat"]
            );
            assert_eq!(boat["afloat"], json!(true), "at y {}", at[1]);
            assert!(
                run >= 5.0,
                "300 ticks took the Sloop {run:.2} along +x, not 5 or more"
            );
        }
    });
}
