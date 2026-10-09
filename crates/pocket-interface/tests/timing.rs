//! What the action layer costs (shared/contract/actions.md, Performance: `act.call`, `intent.tick`),
//! measured, not checked: run with `cargo test --release -p pocket-interface --test timing --
//! --ignored --nocapture`. The budgets are spec-arch's (`docs/spec/budgets.md`); these print the
//! medians the first calibrated `perf` run would set their reference figures from.

mod common;

use common::play::*;
use serde_json::json;

// The wall clock is what these measure: the work between ticks and one system's, never a tick.
#[allow(clippy::disallowed_methods)]
fn timed(f: impl FnOnce()) -> f64 {
    let t = std::time::Instant::now();
    f();
    t.elapsed().as_secs_f64() * 1e6
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn scene() -> pocket_sim::Sim {
    let (mut sim, _) = world(90.0, 270.0, 6.0, 1);
    crate_at(&mut sim, "Crate4", [2.0, 0.0, 0.0]);
    mark(&mut sim, "Mark1", [0.0, 0.0, -300.0]);
    for _ in 0..2 {
        sim.step(&mut pocket_sim::NoHooks).unwrap();
    }
    pocket_interface::action::act(
        sim.world_mut(),
        &skipper(),
        &json!({"actions": [{"do": "start", "intent": "come_to_heading", "params": {"heading_deg": 0}}]}),
    )
    .unwrap();
    sim
}

#[test]
#[ignore = "a measurement: run in release"]
fn act_call_and_intent_tick() {
    // `act.call`: validating and applying a call of 4 actions in the sailing scene.
    let call = json!({"actions": [
        {"do": "start", "intent": "trim_sail", "params": {"keep": true}},
        {"do": "start", "intent": "sail_to", "target": "Mark1", "params": {"trim": "manual"}},
        {"do": "pulse", "control": "interact", "target": "Crate4"},
        {"do": "cancel", "intent_id": 1}]});
    let mut worlds: Vec<pocket_sim::Sim> = (0..101).map(|_| scene()).collect();
    let act: Vec<f64> = worlds
        .iter_mut()
        .map(|sim| {
            timed(|| {
                pocket_interface::action::act(sim.world_mut(), &skipper(), &call).unwrap();
            })
        })
        .collect();
    println!("act.call (4 actions) median {:.1} us", median(act));
    // `intent.tick`: interface.intents with trim_sail holding and sail_to active.
    let sim = &mut worlds[0];
    let tick: Vec<f64> = (0..201)
        .map(|_| {
            timed(|| {
                pocket_interface::action::executor::run_intents(sim.world_mut()).unwrap();
            })
        })
        .collect();
    println!(
        "intent.tick (trim_sail and sail_to live) median {:.1} us",
        median(tick)
    );
}
