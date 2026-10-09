//! The time requests (shared/contract/time.md, Checks): `until` stops after the first tick it holds
//! and is refused on what the seat cannot know, a `step` passes over decision points and keeps the
//! last one pending, a wall limit stops at a consistent boundary, a requested decision a player
//! could not perceive reaches only a developer, turns run only when decided, and a player's
//! `time.status` shows its own clock and decision only.

mod common;

use bevy_ecs::prelude::{Query, Res, ResMut};
use common::fixture;
use common::ledger_hash;
use common::play::*;
use pocket_interface::action::IntentTable;
use pocket_interface::time::control::Controller;
use pocket_interface::time::decide::DecisionDef;
use pocket_interface::time::play::{DecisionFilter, PlayPacing, ThinkingClock};
use pocket_interface::time::session;
use pocket_interface::time::turns::{Resolve, TimeRules, TurnOrder, TurnStructure};
use pocket_sim::{DecisionRequest, EntityId, RunCondition, Sim, SimClock, TickOutput, TickPhase};
use serde_json::{Value, json};

fn zero() -> impl FnMut() -> f64 {
    || 0.0
}

fn act(sim: &mut Sim, ctl: &mut Controller, req: Value) -> u64 {
    let done = pocket_interface::action::act(sim.world_mut(), &skipper(), &req)
        .unwrap_or_else(|p| panic!("{req}: {p:?}"));
    session::acted(ctl, "skipper", done.applied_at, false, 0.0);
    done.outcomes[0]["intent_id"].as_u64().unwrap_or(0)
}

fn step(sim: &mut Sim, ctl: &mut Controller, req: Value) -> Value {
    session::step(sim, ctl, &skipper(), &req, &mut zero())
        .unwrap_or_else(|p| panic!("{req}: {p:?}"))
}

fn tick(sim: &Sim) -> u64 {
    sim.world().resource::<SimClock>().tick.0
}

#[test]
fn until_decision_stops_after_the_tick_it_arose() {
    let (mut sim, _) = world(0.0, 270.0, 6.0, 1);
    let mut ctl = stepped();
    let id = act(
        &mut sim,
        &mut ctl,
        json!({"actions": [{"do": "start", "intent": "trim_sail"}]}),
    );
    let a = step(
        &mut sim,
        &mut ctl,
        json!({"ticks": 3000, "until": "decision"}),
    );
    assert_eq!(a["stopped"], json!("decision"), "{a}");
    let finished = sim.world().resource::<IntentTable>().by_id[&id]
        .finished_tick
        .unwrap();
    assert_eq!(
        a["tick"],
        json!(finished.0),
        "the tick trim_sail succeeded in"
    );
    assert_eq!(a["ran"], json!(finished.0));
    assert_eq!(a["passed_decisions"], json!(0));
    let reasons = &a["decision"]["reasons"];
    assert!(
        reasons
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["kind"] == json!("intent.succeeded")),
        "{a}"
    );
    assert_eq!(a["until"]["condition"], json!("decision"));
    // The events delta carries what the seat perceived, from its push cursor.
    let kinds: Vec<&str> = a["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["kind"].as_str())
        .collect();
    assert!(
        kinds.contains(&"intent.started") && kinds.contains(&"intent.succeeded"),
        "{kinds:?}"
    );
    // The decision is pending until answered; `continue` answers it.
    let c = session::continue_(&sim, &mut ctl, &skipper(), &json!({}), &mut zero()).unwrap();
    assert_eq!(
        (c["stopped"].clone(), c["ran"].clone()),
        (json!("answered"), json!(0))
    );
    assert!(c.get("decision").is_none(), "{c}");
    let again = session::continue_(&sim, &mut ctl, &skipper(), &json!({}), &mut zero()).unwrap();
    assert_eq!(again["warnings"][0]["code"], json!("time.no_decision"));
}

#[test]
fn until_intent_event_fact_any_and_changes() {
    // {"intent": n}: holding counts as finished for until.
    let (mut sim, _) = world(90.0, 270.0, 6.0, 1);
    let mut ctl = stepped();
    let id = act(
        &mut sim,
        &mut ctl,
        json!({"actions": [{"do": "start", "intent": "come_to_heading",
        "params": {"heading_deg": 0, "keep": true}}]}),
    );
    let a = step(
        &mut sim,
        &mut ctl,
        json!({"ticks": 3000, "until": {"intent": id}}),
    );
    assert_eq!(a["stopped"], json!("until"), "{a}");
    let t = a["tick"].as_u64().unwrap();
    let i = sim.world().resource::<IntentTable>().by_id[&id].clone();
    assert_eq!(i.status, pocket_interface::action::IntentStatus::Holding);
    let reached = sim
        .world()
        .resource::<pocket_sim::EventInbox>()
        .events()
        .iter()
        .any(|e| e.kind.as_str() == "intent.reached" && e.tick.0 == t);
    assert!(reached, "it became holding in tick {t}");

    // {"fact": ..}: the first tick the perceived speed, at its precision, is above 2.
    let trim =
        json!({"actions": [{"do": "start", "intent": "trim_sail", "params": {"keep": true}}]});
    let speeds = |n: u64| {
        let (mut twin, boat) = world(0.0, 270.0, 6.0, 1);
        act(&mut twin, &mut stepped(), trim.clone());
        let mut out = Vec::new();
        for _ in 0..n {
            twin.step(&mut pocket_sim::NoHooks).unwrap();
            let e = pocket_sim::entity::require(twin.world(), boat).unwrap();
            let s = twin.world().get::<pocket_physics::Boat>(e).unwrap().speed;
            out.push((s * 10.0).round() / 10.0);
        }
        out
    };
    let (mut sim, _) = world(0.0, 270.0, 6.0, 1);
    let mut ctl = stepped();
    act(&mut sim, &mut ctl, trim.clone());
    let a = step(
        &mut sim,
        &mut ctl,
        json!({"ticks": 3000,
        "until": {"fact": {"name": "speed_mps", "op": "above", "value": 2}}}),
    );
    assert_eq!(a["stopped"], json!("until"), "{a}");
    let t = a["tick"].as_u64().unwrap();
    let s = speeds(t);
    assert!(
        s[s.len() - 1] > 2.0 && s[..s.len() - 1].iter().all(|x| *x <= 2.0),
        "{:?}",
        &s[s.len() - 3..]
    );
    assert_eq!(a["until"]["value"].as_f64(), Some(s[s.len() - 1]));

    // {"event": kind} and {"any": [...]}, which says which held.
    let (mut sim, _) = world(0.0, 270.0, 6.0, 1);
    let mut ctl = stepped();
    act(
        &mut sim,
        &mut ctl,
        json!({"actions": [{"do": "start", "intent": "trim_sail", "params": {"sheet": 0.4}}]}),
    );
    let a = step(
        &mut sim,
        &mut ctl,
        json!({"ticks": 3000, "until": {"any": [
        {"event": "crate.taken"}, {"event": "intent.succeeded"}, {"fact": {"name": "speed_mps", "op": "above", "value": 9}}]}}),
    );
    assert_eq!(a["stopped"], json!("until"), "{a}");
    assert_eq!(
        a["until"]["condition"],
        json!({"event": "intent.succeeded"})
    );
    assert_eq!(a["until"]["event"]["kind"], json!("intent.succeeded"));

    // "changes": the point of sail changes when the boat bears away.
    act(
        &mut sim,
        &mut ctl,
        json!({"actions": [{"do": "start", "intent": "come_to_heading", "params": {"heading_deg": 90}}]}),
    );
    let a = step(
        &mut sim,
        &mut ctl,
        json!({"ticks": 3000,
        "until": {"fact": {"name": "point_of_sail", "op": "changes"}}}),
    );
    assert_eq!(a["stopped"], json!("until"), "{a}");
    assert_ne!(a["until"]["value"], json!("beam_reach"), "{a}");
}

#[test]
fn until_is_refused_on_what_the_seat_cannot_know() {
    let (mut sim, _) = world(0.0, 270.0, 6.0, 1);
    island(&mut sim, "Isle", [0.0, 0.0, -300.0], 40.0);
    let mut ctl = stepped();
    step(&mut sim, &mut ctl, json!({"ticks": 2}));
    let cases = [
        (
            json!({"fact": {"entity": "Isle", "name": "restitution", "op": "above", "value": 0}}),
            "perception.not_perceivable",
        ),
        (
            json!({"fact": {"name": "secret_deg", "op": "above", "value": 0}}),
            "perception.not_perceivable",
        ),
        (
            json!({"fact": {"entity": "Nowhere", "name": "radius_m", "op": "above", "value": 0}}),
            "perception.unknown_entity",
        ),
        (
            json!({"fact": {"name": "point_of_sail", "op": "above", "value": 3}}),
            "request.invalid_value",
        ),
        (
            json!({"fact": {"name": "trim", "op": "equals", "value": 3}}),
            "request.wrong_type",
        ),
        (json!({"intent": 99}), "intent.unknown_id"),
    ];
    let before = (tick(&sim), ledger_hash(&sim));
    for (u, code) in cases {
        let p = session::step(
            &mut sim,
            &mut ctl,
            &skipper(),
            &json!({"ticks": 10, "until": u}),
            &mut zero(),
        )
        .unwrap_err();
        assert_eq!(p.code, code, "{u}: {p:?}");
    }
    let p = session::step(
        &mut sim,
        &mut ctl,
        &skipper(),
        &json!({"ticks": 1, "omniscient": true}),
        &mut zero(),
    )
    .unwrap_err();
    assert_eq!(p.code, "perception.omniscient_forbidden");
    assert_eq!(
        (tick(&sim), ledger_hash(&sim)),
        before,
        "refused requests ran nothing"
    );
    // An `owner` fact is shown to its owner only: of another boat the seat sees it is as hidden,
    // and waiting on it would run to the limit for nothing.
    let mut defs = pocket_interface::perception::probe::defs();
    let boat = defs
        .decl
        .kinds
        .iter_mut()
        .find(|k| k.kind == "boat")
        .unwrap();
    boat.facts
        .iter_mut()
        .find(|f| f.name == "speed_mps")
        .unwrap()
        .exposure = pocket_interface::perception::Exposure::Owner;
    let (mut sim, _) = world_in(fresh_with(defs, 1), 0.0, 270.0, 6.0, true);
    other_boat(&mut sim, "Rival", [0.0, 0.0, -100.0], 0.0);
    let mut ctl = stepped();
    step(&mut sim, &mut ctl, json!({"ticks": 2}));
    let before = (tick(&sim), ledger_hash(&sim));
    let u = json!({"fact": {"entity": "Rival", "name": "speed_mps", "op": "above", "value": 0}});
    let p = session::step(
        &mut sim,
        &mut ctl,
        &skipper(),
        &json!({"ticks": 10, "until": u}),
        &mut zero(),
    )
    .unwrap_err();
    assert_eq!(p.code, "perception.not_perceivable", "{p:?}");
    assert_eq!((tick(&sim), ledger_hash(&sim)), before);
    // Its own instrument of the same name is the seat's to wait on.
    let a = step(
        &mut sim,
        &mut ctl,
        json!({"ticks": 600, "until": {"fact": {"name": "speed_mps", "op": "above", "value": 0}}}),
    );
    assert_eq!(a["stopped"], json!("until"), "{a}");
}

#[test]
fn a_developers_omniscient_until_reads_the_whole_world_and_a_players_never_does() {
    let (mut sim, _) = world(0.0, 270.0, 6.0, 1);
    island(&mut sim, "Isle", [0.0, 0.0, -300.0], 40.0);
    // Far beyond the 120 m a crate can be seen at: the skipper does not know it.
    crate_at(&mut sim, "FarCrate", [0.0, 0.0, 1500.0]);
    let mut ctl = stepped();
    step(&mut sim, &mut ctl, json!({"ticks": 2}));
    let dev = pocket_interface::action::Caller::Developer;
    let hidden = json!({"fact": {"entity": "Isle", "name": "restitution", "op": "at_least",
                                 "value": 0}});
    let unseen = json!({"fact": {"entity": "FarCrate", "name": "alongside", "op": "equals",
                                 "value": false}});
    let run = |sim: &mut Sim,
               ctl: &mut Controller,
               who: &pocket_interface::action::Caller,
               u: &Value,
               omniscient: bool| {
        session::step(
            sim,
            ctl,
            who,
            &json!({"ticks": 10, "until": u, "omniscient": omniscient}),
            &mut zero(),
        )
    };
    // Through the seat's perception, a developer is refused as the player is.
    let p = run(&mut sim, &mut ctl, &dev, &hidden, false).unwrap_err();
    assert_eq!(p.code, "perception.not_perceivable", "{p:?}");
    let p = run(&mut sim, &mut ctl, &dev, &unseen, false).unwrap_err();
    assert_eq!(p.code, "perception.unknown_entity", "{p:?}");
    // With `omniscient`, the developer's conditions read the whole world: each holds after the
    // first tick, and the answer is marked.
    for u in [&hidden, &unseen] {
        let a = run(&mut sim, &mut ctl, &dev, u, true).unwrap_or_else(|p| panic!("{p:?}"));
        assert_eq!(
            (
                a["stopped"].clone(),
                a["ran"].clone(),
                a["omniscient"].clone()
            ),
            (json!("until"), json!(1), json!(true)),
            "{a}"
        );
    }
    // A player's omniscient step is refused before it runs or reads anything, whatever it waits
    // on, and its own answers are never marked.
    let before = (tick(&sim), ledger_hash(&sim));
    for u in [&hidden, &unseen] {
        let p = run(&mut sim, &mut ctl, &skipper(), u, true).unwrap_err();
        assert_eq!(p.code, "perception.omniscient_forbidden", "{p:?}");
    }
    assert_eq!((tick(&sim), ledger_hash(&sim)), before);
    let a = step(&mut sim, &mut ctl, json!({"ticks": 1}));
    assert_eq!(a["omniscient"], json!(false), "{a}");
}

#[test]
fn a_step_passes_over_decision_points_and_keeps_the_last_pending() {
    let (mut sim, _) = world(0.0, 270.0, 6.0, 1);
    let filter = DecisionFilter {
        events: Vec::new(),
        idle_s: None,
        every_s: Some(1.0),
    };
    let mut ctl = Controller::new(PlayPacing::Stepped, filter, Vec::new());
    // The episode's start is a decision point; the step answers it by moving time on.
    let a = step(&mut sim, &mut ctl, json!({"ticks": 200}));
    assert_eq!(a["stopped"], json!("ticks"));
    assert_eq!(a["passed_decisions"], json!(3), "{a}");
    assert_eq!(a["decision"]["id"], json!("180.skipper"));
    assert_eq!(
        a["decision"]["reasons"],
        json!([{"reason": "interval", "every_s": 1.0}])
    );
}

#[test]
fn a_wall_limit_stops_at_a_consistent_boundary() {
    let (mut sim, _) = world(0.0, 270.0, 6.0, 1);
    let mut ctl = stepped();
    // A clock that advances 1 ms each time it is read: the limit falls inside the run.
    let mut ms = 0.0;
    let mut clock = move || {
        ms += 1.0;
        ms
    };
    let a = session::step(
        &mut sim,
        &mut ctl,
        &skipper(),
        &json!({"ticks": 36000, "max_wall_ms": 50}),
        &mut clock,
    )
    .unwrap();
    assert_eq!(a["stopped"], json!("wall_limit"), "{a}");
    let t = a["tick"].as_u64().unwrap();
    assert!(t > 0 && t < 36000);
    let (mut twin, _) = world(0.0, 270.0, 6.0, 1);
    for _ in 0..t {
        twin.step(&mut pocket_sim::NoHooks).unwrap();
    }
    assert_eq!(
        ledger_hash(&sim),
        ledger_hash(&twin),
        "the world at tick {t}"
    );
}

/// A rule that sees the whole world: at tick 30 it asks the skipper to decide, because of a boat
/// the skipper cannot see.
fn rule(
    clock: Res<SimClock>,
    mut out: ResMut<TickOutput>,
    q: Query<(&EntityId, &pocket_interface::perception::Observer)>,
) {
    if clock.tick.0 == 30 {
        for (id, _) in &q {
            out.decide(DecisionRequest {
                observer: *id,
                reason: "boat_near".into(),
                event: None,
            });
        }
    }
}

#[test]
fn a_requested_decision_from_hidden_knowledge_reaches_only_a_developer() {
    let declared = vec![DecisionDef {
        reason: "boat_near".into(),
        doc: "A boat closes in.".into(),
        from_instruments: false,
    }];
    let filter = DecisionFilter::default();
    let run = |all: bool| {
        let (mut sim, _) = world(0.0, 270.0, 6.0, 1);
        sim.add_system("test.rule", TickPhase::Update, RunCondition::Always, rule)
            .unwrap();
        other_boat(&mut sim, "Ketch", [0.0, 0.0, 3000.0], 0.0);
        let mut ctl = Controller::new(PlayPacing::Stepped, filter.clone(), declared.clone());
        ctl.all_requests = all;
        let caller = if all {
            pocket_interface::action::Caller::Developer
        } else {
            skipper()
        };
        session::step(
            &mut sim,
            &mut ctl,
            &caller,
            &json!({"ticks": 60, "until": "decision", "seat": "skipper"}),
            &mut zero(),
        )
        .unwrap()
    };
    let player = run(false);
    assert_eq!(player["stopped"], json!("ticks"), "{player}");
    let developer = run(true);
    assert_eq!(developer["stopped"], json!("decision"), "{developer}");
    assert_eq!(developer["tick"], json!(30));
    assert_eq!(
        developer["decision"]["reasons"],
        json!([{"reason": "requested", "name": "boat_near", "event": null}])
    );
}

fn turns(resolve: Resolve) -> TimeRules {
    TimeRules {
        structure: TurnStructure::Turns {
            order: TurnOrder::RoundRobin {
                seats: vec!["hand".into(), "foot".into()],
            },
            resolve,
        },
        done: None,
        quiet: Some(|w| w.resource::<SimClock>().tick.0 % 7 == 0),
        result: Vec::new(),
        doc: String::new(),
    }
}

#[test]
fn turns_run_only_once_decided_and_resolve_as_declared() {
    let (mut sim, _) = fixture::with_rules(turns(Resolve::Ticks { ticks: 5 }), true);
    let mut ctl = Controller::new(
        PlayPacing::RealTime {
            speed: 1.0,
            pause_on_decision: false,
            clock: None,
        },
        DecisionFilter::default(),
        Vec::new(),
    );
    let mut model = pocket_interface::TimeModel::new(
        pocket_sim::TickRate::DEFAULT,
        pocket_interface::Pacing::RealTime { speed: 1.0 },
    );
    // The deciding phase: no tick over 1000 ms of real time.
    for ms in 0..1000 {
        let p = ctl.pace(sim.world(), &mut model, f64::from(ms));
        assert_eq!(p, pocket_interface::Pace::WaitForCommand, "at {ms} ms");
    }
    // Not foot's turn: refused, nothing applied.
    for a in [
        json!({"do": "end_turn"}),
        json!({"do": "set", "controls": {"lever": 3}}),
    ] {
        let p = fixture::act_as(&mut sim, "foot", json!({"actions": [a]})).unwrap_err();
        assert_eq!(p.code, "time.not_your_turn");
    }
    let done = fixture::act_as(
        &mut sim,
        "hand",
        json!({"actions": [{"do": "set", "controls": {"lever": 2}}, {"do": "end_turn"}]}),
    )
    .unwrap();
    assert_eq!(done.outcomes[1], json!({"did": "end_turn", "turn": 1}));
    // Resolution runs exactly 5 ticks, then foot decides.
    let mut ran = 0;
    while !pocket_interface::time::turns::deciding(sim.world()) {
        sim.step(&mut pocket_sim::NoHooks).unwrap();
        ran += 1;
    }
    assert_eq!(ran, 5);
    let t = sim
        .world()
        .resource::<pocket_interface::time::turns::TurnState>()
        .clone();
    assert_eq!(
        (t.turn, t.to_move.into_iter().collect::<Vec<_>>()),
        (2, vec!["foot".to_owned()])
    );
    // Until quiet, at most max_ticks: quiet holds at tick 7.
    let (mut sim, _) = fixture::with_rules(turns(Resolve::UntilQuiet { max_ticks: 10 }), true);
    fixture::act_as(&mut sim, "hand", json!({"actions": [{"do": "end_turn"}]})).unwrap();
    let mut ran = 0;
    while !pocket_interface::time::turns::deciding(sim.world()) {
        sim.step(&mut pocket_sim::NoHooks).unwrap();
        ran += 1;
    }
    assert_eq!(ran, 7);
    // A continuous game has no turns to end.
    let (mut sim, _) = fixture::world();
    let p = fixture::act(&mut sim, json!({"actions": [{"do": "end_turn"}]})).unwrap_err();
    assert_eq!(p.code, "time.wrong_mode");
}

#[test]
fn a_turn_based_game_replays_exactly_from_its_actions() {
    let script = [
        (
            "hand",
            json!([{"do": "set", "controls": {"lever": 4}}, {"do": "end_turn"}]),
        ),
        (
            "foot",
            json!([{"do": "start", "intent": "hold_for", "params": {"ticks": 3}}, {"do": "end_turn"}]),
        ),
        (
            "hand",
            json!([{"do": "pulse", "control": "button"}, {"do": "end_turn"}]),
        ),
        (
            "foot",
            json!([{"do": "set", "controls": {"mode": "fast"}}, {"do": "end_turn"}]),
        ),
    ];
    let play = || {
        let (mut sim, _) = fixture::with_rules(turns(Resolve::Ticks { ticks: 4 }), true);
        let mut hashes = Vec::new();
        for (seat, actions) in &script {
            fixture::act_as(&mut sim, seat, json!({"actions": actions})).unwrap();
            while !pocket_interface::time::turns::deciding(sim.world()) {
                sim.step(&mut pocket_sim::NoHooks).unwrap();
                hashes.push(ledger_hash(&sim));
            }
        }
        hashes
    };
    let a = play();
    assert_eq!(a.len(), 16);
    assert_eq!(a, play());
}

#[test]
fn a_players_time_status_shows_its_own_clock_and_decision_only() {
    let (mut sim, _) = fixture::with_rules(TimeRules::default(), true);
    let clock = ThinkingClock {
        initial_s: 5.0,
        increment_s: 1.0,
        max_s: 10.0,
    };
    let mut ctl = Controller::new(
        PlayPacing::RealTime {
            speed: 1.0,
            pause_on_decision: true,
            clock: Some(clock),
        },
        DecisionFilter::default(),
        Vec::new(),
    );
    ctl.attach(sim.world());
    let model = pocket_interface::TimeModel::new(
        pocket_sim::TickRate::DEFAULT,
        pocket_interface::Pacing::RealTime { speed: 1.0 },
    );
    let all = ctl.status(sim.world(), &model, None, 0.0);
    assert_eq!(all["decisions"].as_array().unwrap().len(), 2, "{all}");
    assert_eq!(all["clocks"].as_array().unwrap().len(), 2);
    let mine = ctl.status(sim.world(), &model, Some("hand"), 0.0);
    assert_eq!(mine["decisions"].as_array().unwrap().len(), 1, "{mine}");
    assert_eq!(mine["decisions"][0]["seat"], json!("hand"));
    assert_eq!(
        mine["clocks"],
        json!([{"seat": "hand", "clock_s": 5.0, "running": false}])
    );
    // Others appear by name only, in paused_for.
    assert_eq!(mine["paused_for"], json!(["foot", "hand"]));
    let _ = &mut sim;
}
