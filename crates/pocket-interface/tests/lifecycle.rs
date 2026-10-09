//! The intent lifecycle, channels and supersession (shared/contract/actions.md, The lifecycle,
//! Channels and supersession, Intent instances; Checks, "Supersession and lifecycle tests"): every
//! transition of the diagram, a deadline in the same tick as success, a control set on a holding
//! intent's channel, pulses that act one tick each, and the 32 finished intents kept.

mod common;

use common::fixture::*;
use pocket_interface::action::IntentStatus::*;
use pocket_interface::action::IntentTable;
use pocket_sim::PlainData;
use serde_json::json;

fn table(sim: &pocket_sim::Sim) -> &IntentTable {
    sim.world().resource::<IntentTable>()
}

#[test]
fn active_then_holding_then_cancelled() {
    let (mut sim, body) = world();
    let id = hold(&mut sim, json!({"ticks": 3, "keep": true}));
    assert_eq!(id, 1);
    assert_eq!(kinds(&sim), ["intent.started"]);
    assert_eq!(status(&sim, id), Active);
    step(&mut sim);
    step(&mut sim);
    assert_eq!(status(&sim, id), Active);
    assert_eq!(lever(&sim, body), Some(2.0));
    step(&mut sim);
    assert_eq!(status(&sim, id), Holding);
    assert_eq!(kinds(&sim), ["intent.reached"]);
    // Holding has no deadline: 10 s of timeout pass and it holds on.
    for _ in 0..700 {
        step(&mut sim);
    }
    assert_eq!(status(&sim, id), Holding);
    assert_eq!(table(&sim).by_id[&id].finished_tick, None);
    let done = act(
        &mut sim,
        json!({"actions": [{"do": "cancel", "intent_id": id}]}),
    )
    .unwrap();
    assert_eq!(
        done.outcomes[0],
        json!({"did": "cancel", "intent_id": 1, "status": "cancelled"})
    );
    assert_eq!(status(&sim, id), Cancelled);
    assert_eq!(
        kinds(&sim).last().map(String::as_str),
        Some("intent.cancelled")
    );
    // Cancelling a finished one answers its final status and changes nothing.
    let events = kinds(&sim).len();
    let again = act(
        &mut sim,
        json!({"actions": [{"do": "cancel", "intent_id": "#1"}]}),
    )
    .unwrap();
    assert_eq!(again.outcomes[0]["status"], json!("cancelled"));
    assert_eq!(kinds(&sim).len(), events);
}

#[test]
fn succeeds_without_keep_and_reports_progress() {
    let (mut sim, _) = world();
    let id = hold(&mut sim, json!({"ticks": 2}));
    step(&mut sim);
    let i = &table(&sim).by_id[&id];
    assert_eq!((i.status, i.started_tick.0), (Active, 1));
    assert_eq!(
        i.progress,
        vec![("count".to_owned(), PlainData::Number(1.0))]
    );
    step(&mut sim);
    let i = &table(&sim).by_id[&id];
    assert_eq!(
        (i.status, i.finished_tick.map(|t| t.0)),
        (Succeeded, Some(2))
    );
    assert_eq!(kinds(&sim), ["intent.succeeded"]);
    let e = &sim.world().resource::<pocket_sim::EventInbox>().events()[0];
    assert_eq!(e.data.get("intent_id"), Some(&PlainData::Number(1.0)));
    assert_eq!(e.data.get("count"), Some(&PlainData::Number(2.0)));
}

#[test]
fn a_deadline_fails_it_with_intent_timeout() {
    let (mut sim, _) = world();
    // 0.5 s at 60 Hz: the deadline is tick 1 + 30, checked before the executor runs.
    let id = hold(&mut sim, json!({"ticks": 100, "timeout_s": 0.5}));
    assert_eq!(table(&sim).by_id[&id].deadline_tick.map(|t| t.0), Some(31));
    for _ in 0..30 {
        step(&mut sim);
    }
    assert_eq!(status(&sim, id), Active);
    step(&mut sim);
    let i = &table(&sim).by_id[&id];
    assert_eq!(i.status, Failed);
    assert_eq!(i.failure.as_ref().unwrap().code, "intent.timeout");
    assert_eq!(i.progress[0].1, PlainData::Number(30.0), "it ran 30 times");
    let e = sim.world().resource::<pocket_sim::EventInbox>().events()[0].clone();
    assert_eq!(e.kind.as_str(), "intent.failed");
    assert_eq!(
        e.data.get("code"),
        Some(&PlainData::String("intent.timeout".into()))
    );
    let PlainData::String(m) = e.data.get("message").unwrap() else {
        panic!("no message")
    };
    assert!(m.contains("hold_for") && m.contains("0.5"), "{m}");
}

#[test]
fn a_deadline_in_the_same_tick_as_success_wins() {
    let (mut sim, _) = world();
    // The 31st run would come in tick 31, the deadline's: the deadline is checked first.
    let id = hold(&mut sim, json!({"ticks": 31, "timeout_s": 0.5}));
    for _ in 0..31 {
        step(&mut sim);
    }
    let i = &table(&sim).by_id[&id];
    assert_eq!((i.status, i.finished_tick.map(|t| t.0)), (Failed, Some(31)));
    assert_eq!(i.progress[0].1, PlainData::Number(30.0));
    // One tick less of goal: it succeeds the tick before the deadline.
    let (mut sim, _) = world();
    let id = hold(&mut sim, json!({"ticks": 30, "timeout_s": 0.5}));
    for _ in 0..31 {
        step(&mut sim);
    }
    assert_eq!(status(&sim, id), Succeeded);
}

#[test]
fn an_intent_on_the_same_channel_supersedes() {
    let (mut sim, _) = world();
    let a = hold(&mut sim, json!({"ticks": 100}));
    step(&mut sim);
    let done = act(
        &mut sim,
        json!({"actions": [{"do": "start", "intent": "hold_for", "params": {"ticks": 5}, "tag": "b"}]}),
    )
    .unwrap();
    let b = done.outcomes[0]["intent_id"].as_u64().unwrap();
    assert_eq!(done.outcomes[0]["superseded"], json!([a]));
    assert_eq!(done.outcomes[0]["tag"], json!("b"));
    assert_eq!(
        done.outcomes[0]["params"],
        json!({"keep": false, "refuse": false, "ticks": 5, "timeout_s": 10})
    );
    let i = &table(&sim).by_id[&a];
    assert_eq!((i.status, i.superseded_by), (Superseded, Some(b)));
    assert_eq!(
        kinds(&sim)[kinds(&sim).len() - 2..],
        ["intent.superseded", "intent.started"]
    );
}

#[test]
fn a_control_set_on_a_holding_intents_channel_supersedes_it() {
    let (mut sim, body) = world();
    let a = hold(&mut sim, json!({"ticks": 1, "keep": true}));
    step(&mut sim);
    assert_eq!(status(&sim, a), Holding);
    // A pulse on another channel leaves it alone.
    let done = act(
        &mut sim,
        json!({"actions": [{"do": "pulse", "control": "button"}]}),
    )
    .unwrap();
    assert_eq!(done.outcomes[0]["superseded"], json!([]));
    assert_eq!(status(&sim, a), Holding);
    // A hand on the lever disengages it.
    let done = act(
        &mut sim,
        json!({"actions": [{"do": "set", "controls": {"lever": 42}}]}),
    )
    .unwrap();
    assert_eq!(
        done.outcomes[0],
        json!({"did": "set", "controls": {"lever": 42}, "superseded": [a]})
    );
    let i = &table(&sim).by_id[&a];
    assert_eq!((i.status, i.superseded_by), (Superseded, None));
    step(&mut sim);
    assert_eq!(
        lever(&sim, body),
        Some(42.0),
        "no executor writes it any more"
    );
}

#[test]
fn two_pulses_act_in_two_ticks() {
    let (mut sim, body) = world();
    for _ in 0..2 {
        act(
            &mut sim,
            json!({"actions": [{"do": "pulse", "control": "button"}]}),
        )
        .unwrap();
    }
    let queued = |sim: &pocket_sim::Sim| {
        let e = pocket_sim::entity::require(sim.world(), body).unwrap();
        sim.world()
            .get::<pocket_interface::action::Controls>(e)
            .map_or(0, |c| c.pulses.get("button").map_or(0, |q| q.len()))
    };
    assert_eq!(queued(&sim), 2);
    step(&mut sim);
    assert_eq!(queued(&sim), 1);
    step(&mut sim);
    assert_eq!(queued(&sim), 0);
}

#[test]
fn the_32_most_recently_finished_are_kept() {
    let (mut sim, _) = world();
    for _ in 0..40 {
        hold(&mut sim, json!({"ticks": 1}));
        step(&mut sim);
    }
    let t = table(&sim);
    assert_eq!(t.next_id, 41);
    assert_eq!(t.by_id.len(), 32);
    assert_eq!(
        t.by_id.keys().next(),
        Some(&9),
        "the oldest eight went first"
    );
    assert!(t.by_id.values().all(|i| i.status == Succeeded));
}

#[test]
fn conflicts_inside_a_call_are_refused_whole() {
    let (mut sim, _) = world();
    let before = table(&sim).next_id;
    for actions in [
        json!([{"do": "set", "controls": {"lever": 1}},
               {"do": "start", "intent": "hold_for", "params": {"ticks": 2}}]),
        json!([{"do": "set", "controls": {"lever": 1}}, {"do": "set", "controls": {"lever": 2}}]),
        json!([{"do": "start", "intent": "hold_for", "params": {"ticks": 2}},
               {"do": "start", "intent": "hold_for", "params": {"ticks": 3}}]),
    ] {
        let p = act(&mut sim, json!({"actions": actions})).unwrap_err();
        assert_eq!(p.code, "action.conflict", "{p:?}");
    }
    assert_eq!(table(&sim).next_id, before);
    // Two controls on one channel in one call drive nothing twice: accepted.
    act(
        &mut sim,
        json!({"actions": [{"do": "set", "controls": {"mode": "fast", "lamp": true}}]}),
    )
    .unwrap();
}

#[test]
fn values_that_do_not_fit_are_refused_with_what_fits() {
    let (mut sim, _) = world();
    let cases = [
        (
            json!({"do": "set", "controls": {"lever": 101}}),
            "request.out_of_range",
        ),
        (
            json!({"do": "set", "controls": {"mode": "turbo"}}),
            "request.invalid_value",
        ),
        (
            json!({"do": "set", "controls": {"lamp": 1}}),
            "request.wrong_type",
        ),
        (
            json!({"do": "set", "controls": {"leverr": 1}}),
            "action.unknown_control",
        ),
        (
            json!({"do": "pulse", "control": "lever"}),
            "request.not_applicable",
        ),
        (
            json!({"do": "set", "controls": {"button": 1}}),
            "request.not_applicable",
        ),
        (
            json!({"do": "start", "intent": "hold"}),
            "action.unknown_intent",
        ),
        (
            json!({"do": "start", "intent": "hold_for", "params": {"ticks": 2, "refuse": true}}),
            "fixture.refused",
        ),
        (json!({"do": "cancel", "intent_id": 9}), "intent.unknown_id"),
        (json!({"do": "end_turn"}), "time.wrong_mode"),
    ];
    for (action, code) in cases {
        let p = act(&mut sim, json!({"actions": [action]})).unwrap_err();
        assert_eq!(p.code, code, "{action}: {p:?}");
        if code == "request.invalid_value" {
            assert!(p.detail.to_string_lossy().contains("fast"), "{p:?}");
        }
    }
    assert_eq!(table(&sim).next_id, 1);
}

trait Lossy {
    fn to_string_lossy(&self) -> String;
}

impl Lossy for serde_json::Map<String, serde_json::Value> {
    fn to_string_lossy(&self) -> String {
        serde_json::Value::Object(self.clone()).to_string()
    }
}
