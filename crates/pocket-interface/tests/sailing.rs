//! The sailing intents (shared/contract/sailing.md, Actions and Checks, "Sailing acceptance" and
//! "Golden cases"): `come_to_heading` between headings 45 degrees apart, `trim_sail` to the best
//! trim on each point of sail, `sail_to` a mark dead upwind with a tack, sailing at an island,
//! a timeout, a crate out of reach, and a `sail_to` that makes no progress, each identical for a
//! seed run twice. The worlds are the game's: the skipper acts through its perception (the sailing
//! declarations and `game_catalog`, `common::play`), so an entity a test lays is known only once a
//! tick's perception update has seen it. The full matrix at five seeds and the 500 m beat are
//! `#[ignore]`d for debug builds; the local check's test step runs them (`cargo test --release
//! --workspace -- --include-ignored`), as does `cargo test --release -p pocket-interface --test
//! sailing -- --include-ignored`.

mod common;

use common::play::{crate_at, island, mark, world_sail as world};
use common::{Delivered, act, boat, intent, ledger_hash, pos, reading, started, step};
use pocket_interface::action::IntentStatus::{self, *};
use pocket_sim::Sim;
use serde_json::{Value, json};

/// The wind of every case: from the west at 6 m/s (sailing.md, Checks).
const WIND_FROM: f64 = 270.0;
const WIND: f64 = 6.0;

/// The tick limits slice 2 sets from the sailing simulation (sailing.md, Checks, and its open
/// choices): 30 s for a turn (the slowest measured is printed by the matrix), and 40 minutes for
/// a 500 m beat, which the slice 1 boat sails in about 27 (0.3 m/s made good to windward: about 14
/// degrees of leeway close-hauled and 25 to 30 s a tack).
const TURN_LIMIT: u64 = 1800;
const BEAT_LIMIT: u64 = 144000;

fn start(sim: &mut Sim, intent: &str, params: Value) -> u64 {
    let done = act(
        sim,
        json!({"actions": [{"do": "start", "intent": intent, "params": params}]}),
    )
    .unwrap_or_else(|p| panic!("{intent} {params}: {p:?}"));
    started(&done)
}

/// Steps until the intent leaves `Active` or `max` ticks pass: its status and the ticks run.
fn run(sim: &mut Sim, id: u64, max: u64) -> (IntentStatus, u64) {
    for t in 1..=max {
        step(sim);
        let s = intent(sim, id).status;
        if s != Active {
            return (s, t);
        }
    }
    (Active, max)
}

/// The boat set and trimmed (`trim_sail` best, kept) and holding `heading`.
fn settled(heading: f64, seed: u64) -> (Sim, pocket_sim::EntityId) {
    let (mut sim, id) = world(heading, WIND_FROM, WIND, true, seed);
    start(&mut sim, "trim_sail", json!({"keep": true}));
    let h = start(
        &mut sim,
        "come_to_heading",
        json!({"heading_deg": heading, "keep": true}),
    );
    let (s, t) = run(&mut sim, h, TURN_LIMIT);
    assert_eq!(s, Holding, "settling on {heading} took {t} ticks");
    (sim, id)
}

/// `come_to_heading` from `from` to `to`: the ticks it took to reach, and the world hash then.
fn turn(from: f64, to: f64, seed: u64) -> (u64, u64) {
    let (mut sim, _) = settled(from, seed);
    let id = start(&mut sim, "come_to_heading", json!({"heading_deg": to}));
    let (s, t) = run(&mut sim, id, TURN_LIMIT);
    let i = intent(&sim, id);
    assert_eq!(
        s, Succeeded,
        "{from} -> {to} seed {seed}: {:?} after {t} ticks",
        i.failure
    );
    (t, ledger_hash(&sim))
}

fn outside_no_go(h: f64) -> bool {
    pocket_interface::action::affordance::relative(h, WIND_FROM).abs() > 45.0
}

#[test]
fn come_to_heading_turns_and_holds() {
    for (from, to) in [(0.0, 90.0), (90.0, 0.0), (180.0, 45.0), (45.0, 180.0)] {
        let (a, ha) = turn(from, to, 1);
        let (b, hb) = turn(from, to, 1);
        assert_eq!(
            (a, ha),
            (b, hb),
            "{from} -> {to} is not identical run twice"
        );
    }
}

#[test]
#[ignore = "the full matrix: run in release"]
fn come_to_heading_between_every_pair_at_five_seeds() {
    let headings: Vec<f64> = (0..8)
        .map(|i| f64::from(i) * 45.0)
        .filter(|h| outside_no_go(*h))
        .collect();
    let mut worst = (0, 0.0, 0.0);
    for seed in 1..=5 {
        for &from in &headings {
            for &to in &headings {
                if from == to {
                    continue;
                }
                let first = turn(from, to, seed);
                assert_eq!(first, turn(from, to, seed), "{from} -> {to} seed {seed}");
                if first.0 > worst.0 {
                    worst = (first.0, from, to);
                }
            }
        }
    }
    println!(
        "slowest turn: {} ticks, {} -> {}",
        worst.0, worst.1, worst.2
    );
}

/// `trim_sail {sheet: "best"}` on a heading: the drive once reached and settled.
fn trimmed_drive(heading: f64, seed: u64) -> f64 {
    let (mut sim, id) = world(heading, WIND_FROM, WIND, true, seed);
    start(
        &mut sim,
        "come_to_heading",
        json!({"heading_deg": heading, "keep": true}),
    );
    let t = start(
        &mut sim,
        "trim_sail",
        json!({"sheet": "best", "keep": true}),
    );
    let (s, ticks) = run(&mut sim, t, 1200);
    assert_eq!(s, Holding, "trim on {heading} after {ticks}");
    for _ in 0..600 {
        step(&mut sim);
    }
    boat(&sim, id).drive
}

#[test]
fn trim_sail_best_drives_on_every_point_of_sail() {
    // Close-hauled, close reach, beam reach, broad reach and running, on both tacks.
    for off in [55.0, 70.0, 90.0, 125.0, 180.0] {
        for side in [1.0, -1.0] {
            let heading = (WIND_FROM + side * off).rem_euclid(360.0);
            let d = trimmed_drive(heading, 1);
            assert!(
                d >= 0.9,
                "heading {heading} ({off} off the wind): drive {d}"
            );
        }
    }
}

/// `sail_to` a mark `upwind_m` dead upwind of the boat, from a beam reach with way on: the ticks
/// it took, its tacks, and the world hash then.
fn beat(upwind_m: f64, seed: u64) -> (u64, f64, u64) {
    let (mut sim, boat_id) = settled(0.0, seed);
    let at = pos(&sim, boat_id);
    let mark = mark(&mut sim, "Mark1", [at[0] - upwind_m, 0.0, at[2]]);
    step(&mut sim);
    let done = act(
        &mut sim,
        json!({"actions": [{"do": "start", "intent": "sail_to", "target": "Mark1",
                            "params": {"timeout_s": 3600}}]}),
    )
    .unwrap();
    assert_eq!(
        done.outcomes[0]["target"],
        json!({"id": mark.get(), "name": "Mark1"})
    );
    let id = started(&done);
    let (s, t) = run(&mut sim, id, BEAT_LIMIT);
    let i = intent(&sim, id);
    assert_eq!(
        s, Succeeded,
        "{upwind_m} m, seed {seed}, after {t} ticks: {:?} {:?}",
        i.failure, i.progress
    );
    let tacks = reading(&i, "tacks").unwrap();
    assert!(tacks >= 1.0, "no tack: {:?}", i.progress);
    (t, tacks, ledger_hash(&sim))
}

#[test]
fn sail_to_a_mark_upwind_beats_with_a_tack() {
    let a = beat(150.0, 1);
    assert_eq!(a, beat(150.0, 1));
    println!("150 m upwind: {} ticks, {} tacks", a.0, a.1);
}

#[test]
#[ignore = "about 100,000 ticks a run: run in release"]
fn sail_to_a_mark_500_m_dead_upwind_at_five_seeds() {
    for seed in 1..=5 {
        let a = beat(500.0, seed);
        assert_eq!(a, beat(500.0, seed), "seed {seed}");
        println!("500 m upwind, seed {seed}: {} ticks, {} tacks", a.0, a.1);
    }
}

#[test]
fn sailing_at_an_island_runs_aground() {
    let (mut sim, _) = world(90.0, WIND_FROM, WIND, true, 1);
    island(&mut sim, "Isle", [120.0, 0.0, 0.0], 30.0);
    let id = start(
        &mut sim,
        "come_to_heading",
        json!({"heading_deg": 90, "keep": true}),
    );
    start(&mut sim, "trim_sail", json!({"keep": true}));
    let mut t = 0;
    while intent(&sim, id).status.is_live() && t < 6000 {
        step(&mut sim);
        t += 1;
    }
    let i = intent(&sim, id);
    let s = i.status;
    let f = i.failure.as_ref().map(|f| f.code.as_str());
    assert_eq!((s, f), (Failed, Some("sail.aground")), "after {t} ticks");
}

#[test]
fn a_one_second_timeout_ends_in_intent_timeout() {
    let (mut sim, _) = world(0.0, WIND_FROM, WIND, true, 1);
    let id = start(
        &mut sim,
        "come_to_heading",
        json!({"heading_deg": 180, "timeout_s": 1}),
    );
    let (s, t) = run(&mut sim, id, 600);
    let f = intent(&sim, id).failure.map(|f| f.code);
    assert_eq!((s, t, f.as_deref()), (Failed, 61, Some("intent.timeout")));
}

#[test]
fn an_interact_pulse_at_a_crate_14_m_away_is_refused() {
    let (mut sim, _) = world(90.0, WIND_FROM, WIND, false, 1);
    crate_at(&mut sim, "Crate7", [0.0, 0.0, 14.0]);
    let near = crate_at(&mut sim, "Crate4", [2.0, 0.0, 0.0]);
    step(&mut sim);
    let p = act(
        &mut sim,
        json!({"actions": [{"do": "pulse", "control": "interact", "target": "Crate7"}]}),
    )
    .unwrap_err();
    assert_eq!(p.code, "action.unavailable");
    let unmet = &p.detail["unmet"][0];
    assert_eq!(unmet["code"], json!("action.out_of_reach"));
    assert_eq!(unmet["detail"]["range_m"].as_f64(), Some(14.0));
    assert_eq!(unmet["detail"]["max_m"].as_f64(), Some(3.0));
    assert!(
        unmet["message"].as_str().unwrap().contains("Crate7"),
        "{p:?}"
    );
    // In reach, the same pulse is accepted, and `use` sends it.
    let done = act(
        &mut sim,
        json!({"actions": [{"do": "use", "entity": "Crate4", "verb": "take_aboard"}]}),
    )
    .unwrap();
    assert_eq!(done.outcomes[0]["then"]["did"], json!("pulse"));
    step(&mut sim);
    let delivered = sim.world().resource::<Delivered>().0.clone();
    assert_eq!(delivered, vec![(2, json!(near.get()))]);
}

#[test]
fn sail_to_without_progress_fails_with_its_code_message_and_detail() {
    // The sail furled and `trim: manual`: the boat lies still and comes no nearer.
    let (mut sim, _) = world(90.0, WIND_FROM, WIND, false, 1);
    mark(&mut sim, "Mark1", [0.0, 0.0, -400.0]);
    step(&mut sim);
    let done = act(
        &mut sim,
        json!({"actions": [{"do": "start", "intent": "sail_to", "target": "Mark1",
                            "params": {"trim": "manual"}}]}),
    )
    .unwrap();
    assert_eq!(done.warnings[0].code, "sail.not_set");
    let id = started(&done);
    let (s, t) = run(&mut sim, id, 4000);
    assert_eq!((s, t), (Failed, 3601), "60 s without coming 5 m nearer");
    let caller = pocket_interface::action::Caller::Player {
        seat: "skipper".into(),
    };
    let answer =
        pocket_interface::action::intents(sim.world(), &caller, &json!({"ids": [id]})).unwrap();
    let failure = &answer["intents"][0]["failure"];
    assert_eq!(failure["code"], json!("sail.no_progress"));
    assert_eq!(
        failure["message"],
        json!("No progress toward the target: still 400 m away after 60 s.")
    );
    assert_eq!(
        failure["detail"],
        json!({"distance_m": 400, "needed_m": 5, "window_s": 60})
    );
    let e = sim
        .world()
        .resource::<pocket_sim::EventInbox>()
        .events()
        .iter()
        .find(|e| e.kind.as_str() == "intent.failed")
        .cloned()
        .unwrap();
    assert_eq!(
        e.data.get("code").map(|v| v.to_json()),
        Some(json!("sail.no_progress"))
    );
    assert_eq!(
        e.data.get("message").map(|v| v.to_json()),
        Some(failure["message"].clone())
    );
}
