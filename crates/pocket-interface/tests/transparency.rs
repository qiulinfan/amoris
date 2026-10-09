//! `contract.intents.transparency` for the Rust executors (shared/contract/actions.md, Checks): a
//! seeded sailing run drives the boat with intents only (trim, a series of headings, sail to two
//! marks, one dead upwind); every control the executors wrote is recorded as a `set` at the
//! boundary before the tick it was written in; a second run applies those sets and starts no
//! intent. At every tick the two worlds' sections must be equal but for the intents' own state
//! (`IntentTable`, the seat's perceived events, and the event counter and inbox, which the intents'
//! events also fill: Slice 2, actions.md's open choices), and their event records equal once the
//! `intent.*` events are left out. The five-minute run of the check is `#[ignore]`d for debug
//! builds; a one-minute run is not.

mod common;

use common::ledger;
use common::play::*;
use pocket_physics::Boat;
use pocket_sim::{EntityId, NoHooks, Sim};
use serde_json::{Value, json};

/// The route: (tick, the act starting it).
fn route() -> Vec<(u64, Value)> {
    let start =
        |intent: &str, params: Value| json!({"do": "start", "intent": intent, "params": params});
    vec![
        (0, start("trim_sail", json!({"keep": true}))),
        (
            0,
            start("come_to_heading", json!({"heading_deg": 45, "keep": true})),
        ),
        (
            1200,
            start("come_to_heading", json!({"heading_deg": 135, "keep": true})),
        ),
        (
            2400,
            start("come_to_heading", json!({"heading_deg": 0, "keep": true})),
        ),
        (
            3600,
            json!({"do": "start", "intent": "sail_to", "target": "Mark1", "params": {"trim": "manual"}}),
        ),
        (
            9000,
            json!({"do": "start", "intent": "sail_to", "target": "Mark2"}),
        ),
    ]
}

fn scene(seed: u64) -> (Sim, EntityId) {
    let (mut sim, boat) = world(0.0, 270.0, 6.0, seed);
    mark(&mut sim, "Mark1", [0.0, 0.0, -400.0]);
    mark(&mut sim, "Mark2", [-80.0, 0.0, -400.0]);
    (sim, boat)
}

fn controls(sim: &Sim, boat: EntityId) -> [f64; 3] {
    let e = pocket_sim::entity::require(sim.world(), boat).unwrap();
    let b = sim.world().get::<Boat>(e).unwrap();
    [b.rudder, b.sheet, b.hoist]
}

/// The tick's event records but the intents' own: (kind, tick, subject, data).
fn events(sim: &Sim) -> Vec<(String, u64, Option<u64>, String)> {
    sim.world()
        .resource::<pocket_sim::EventInbox>()
        .events()
        .iter()
        .filter(|e| !e.kind.as_str().starts_with("intent."))
        .map(|e| {
            (
                e.kind.as_str().to_owned(),
                e.tick.0,
                e.subject.map(EntityId::get),
                e.data.to_json().to_string(),
            )
        })
        .collect()
}

/// The sections the check compares, by name.
fn sections(sim: &Sim) -> Vec<(&'static str, Vec<u8>)> {
    const OWN: [&str; 4] = [
        "IntentTable",
        "ObserverEvents",
        "EventCounter",
        "EventInbox",
    ];
    ledger()
        .snapshot(sim.world())
        .unwrap()
        .sections
        .into_iter()
        .filter(|(n, _)| !OWN.contains(n))
        .collect()
}

fn check(ticks: u64, seed: u64) {
    // The run with intents: the controls the executors wrote, by tick.
    let (mut a, boat) = scene(seed);
    let route = route();
    let mut writes: Vec<(u64, [f64; 3])> = Vec::new();
    let mut digests = Vec::new();
    let mut last = controls(&a, boat);
    for t in 0..ticks {
        for (_, action) in route.iter().filter(|(at, _)| *at == t) {
            pocket_interface::action::act(a.world_mut(), &skipper(), &json!({"actions": [action]}))
                .unwrap_or_else(|p| panic!("{action}: {p:?}"));
        }
        a.step(&mut NoHooks).unwrap();
        let now = controls(&a, boat);
        if now != last {
            writes.push((t + 1, now));
            last = now;
        }
        digests.push((sections(&a), events(&a)));
    }
    let started = route.iter().filter(|(at, _)| *at < ticks).count() as u64;
    let table = a
        .world()
        .resource::<pocket_interface::action::IntentTable>();
    assert_eq!(
        table.next_id,
        1 + started,
        "every intent of the route started"
    );
    // The run with the same controls set directly, and no intent.
    let (mut b, _) = scene(seed);
    let mut next = 0;
    for t in 0..ticks {
        while next < writes.len() && writes[next].0 == t + 1 {
            let [rudder, sheet, hoist] = writes[next].1;
            pocket_interface::action::act(b.world_mut(), &skipper(),
                &json!({"actions": [{"do": "set", "controls": {"rudder": rudder, "sheet": sheet, "hoist": hoist}}]}))
                .unwrap();
            next += 1;
        }
        b.step(&mut NoHooks).unwrap();
        let (sa, ea) = &digests[usize::try_from(t).unwrap()];
        let sb = sections(&b);
        for ((name, x), (_, y)) in sa.iter().zip(&sb) {
            assert!(x == y, "tick {}: section {name} differs", t + 1);
        }
        assert_eq!(sa.len(), sb.len());
        assert_eq!(ea, &events(&b), "tick {}: the events differ", t + 1);
    }
    assert_eq!(
        b.world()
            .resource::<pocket_interface::action::IntentTable>()
            .next_id,
        1
    );
    println!("{ticks} ticks, {} control writes replayed", writes.len());
}

#[test]
fn intents_achieve_only_what_their_controls_would() {
    check(3600, 1);
}

#[test]
#[ignore = "18000 ticks twice with every section compared at every tick: run in release"]
fn intents_achieve_only_what_their_controls_would_over_five_minutes() {
    check(18000, 1);
}
