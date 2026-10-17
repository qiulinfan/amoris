//! `scripts.profile` (docs/spec/debugger.md 14) on the sailing sample: it profiles the ticks the
//! world would run next on a fork, so the world does not move; each system's steps are those the
//! world then takes; the same state answers the same bytes; bad parameters are refused.
#![cfg(feature = "transpile")]
mod common;

use std::collections::BTreeMap;

use amoris_link::Source;
use amoris_runtime::{Command, Game};
use serde_json::{Value, json};

fn profile(g: &mut Game, params: Value) -> Result<Value, amoris_contract::Problem> {
    g.apply(&Command::new(
        Source::Developer(0),
        1,
        "scripts.profile",
        params,
    ))
}

#[test]
fn a_profile_runs_the_next_ticks_on_a_fork() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 1).unwrap_or_else(|e| panic!("{e:#?}"));
        for _ in 0..10 {
            g.step().unwrap();
        }
        let (tick, hash) = (g.tick(), g.world_hash().unwrap());
        let v = profile(&mut g, json!({"ticks": 30, "interval": 50, "folded": true}))
            .unwrap_or_else(|e| panic!("{e:#?}"));
        // The world did not move.
        assert_eq!((g.tick(), g.world_hash().unwrap()), (tick, hash));
        assert_eq!(v["from_tick"], json!(10));
        assert_eq!(v["to_tick"], json!(40));
        assert_eq!(v["interval"], json!(50));
        assert_eq!(v["estimated"], json!(true));
        // The same state, the same answer.
        let again = profile(&mut g, json!({"ticks": 30, "interval": 50, "folded": true})).unwrap();
        assert_eq!(v.to_string(), again.to_string());
        // Each system's steps are what the world's own next 30 ticks take.
        let mut world: BTreeMap<String, u64> = BTreeMap::new();
        for _ in 0..30 {
            g.step().unwrap();
            for (name, steps) in amoris_script::scripts::last_steps(g.world()) {
                *world.entry(name).or_default() += steps;
            }
        }
        let systems = v["systems"].as_array().unwrap();
        assert!(!systems.is_empty(), "{v}");
        let profiled: BTreeMap<String, u64> = systems
            .iter()
            .map(|s| {
                (
                    s["system"].as_str().unwrap().to_owned(),
                    s["steps"].as_u64().unwrap(),
                )
            })
            .collect();
        let ran: BTreeMap<String, u64> = world.into_iter().filter(|(_, s)| *s > 0).collect();
        assert_eq!(profiled, ran);
        assert_eq!(v["steps"].as_u64().unwrap(), ran.values().sum::<u64>());
        // The most steps first; positions in TypeScript; folded text as flame graphs read it.
        let steps: Vec<u64> = systems
            .iter()
            .map(|s| s["steps"].as_u64().unwrap())
            .collect();
        assert!(steps.windows(2).all(|w| w[0] >= w[1]), "{steps:?}");
        let top = &systems[0];
        assert!(
            top["lines"][0]["at"]
                .as_str()
                .is_some_and(|at| at.starts_with("scripts/") || at.starts_with("amoris:")),
            "{top}"
        );
        // Without an interval, one that aims at 20,000 samples: this light game is sampled at
        // every step, so the samples are the steps.
        let exact = profile(&mut g, json!({"ticks": 30})).unwrap();
        assert_eq!(exact["interval"], json!(1), "{exact}");
        assert_eq!(exact["estimated"], json!(false));
        assert_eq!(exact["samples"], v["steps"]);
        let folded = v["folded"].as_str().unwrap();
        let first = folded.lines().next().unwrap();
        assert!(
            first.rsplit(' ').next().unwrap().parse::<u64>().is_ok(),
            "{first}"
        );
        println!("{}", serde_json::to_string_pretty(top).unwrap());
    });
}

#[test]
fn bad_parameters_are_refused() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 1).unwrap_or_else(|e| panic!("{e:#?}"));
        for bad in [
            json!({"interval": 0}),
            json!({"interval": 1_000_001}),
            json!({"ticks": 0}),
            json!({"ticks": 3601}),
        ] {
            let e = profile(&mut g, bad.clone()).unwrap_err();
            assert_eq!(e.code, "request.out_of_range", "{bad}: {e:#?}");
        }
        let e = profile(&mut g, json!({"tick": 3})).unwrap_err();
        assert_eq!(e.code, "request.unknown_field", "{e:#?}");
        let names: Vec<String> = amoris_runtime::catalog_json(&[])
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap().to_owned())
            .collect();
        assert!(names.contains(&"scripts.profile".to_owned()));
    });
}
