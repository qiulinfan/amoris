//! The act request's checks (shared/contract/actions.md, Checks): `contract.actions.atomic` (a
//! valid action followed by an invalid one of each class of each validation phase is refused, and
//! the world, the intent counter and the events are as they were) and `contract.actions.refusal`
//! (every field and intent parameter misspelled one edit at a time is refused with
//! `request.unknown_field` whose suggestions name it, and nothing is applied).

mod common;

use common::*;
use pocket_contract::Problem;
use pocket_interface::action::{Caller, IntentTable};
use pocket_sim::Sim;
use serde_json::{Value, json};

/// A sailing world with a crate in reach, one out of reach, a mark, an island and a live intent.
fn scene() -> Sim {
    let (mut sim, _) = world(90.0, 270.0, 6.0, true, 1);
    crate_at(&mut sim, "Crate4", [2.0, 0.0, 0.0]);
    crate_at(&mut sim, "Crate7", [0.0, 0.0, 14.0]);
    mark(&mut sim, "Mark1", [0.0, 0.0, -300.0]);
    island(&mut sim, "Isle", [400.0, 0.0, 0.0], 60.0);
    act(
        &mut sim,
        json!({"actions": [{"do": "start", "intent": "trim_sail", "params": {"keep": true}}]}),
    )
    .unwrap();
    step(&mut sim);
    sim
}

/// What a refused call must leave as it was: the world hash, the counter, the events.
fn state(sim: &Sim) -> (u64, u64, usize) {
    (
        ledger_hash(sim),
        sim.world().resource::<IntentTable>().next_id,
        sim.world()
            .resource::<pocket_sim::EventInbox>()
            .events()
            .len(),
    )
}

fn codes_of(p: &Problem) -> Vec<String> {
    let mut out = vec![p.code.clone()];
    out.extend(p.also().into_iter().map(|a| a.code));
    out
}

#[test]
fn a_call_with_one_invalid_action_applies_nothing() {
    let valid = [
        json!({"do": "set", "controls": {"rudder": 0.1}}),
        json!({"do": "pulse", "control": "interact", "target": "Crate4"}),
        json!({"do": "start", "intent": "come_to_heading", "params": {"heading_deg": 45}}),
        json!({"do": "use", "entity": "Crate4", "verb": "take_aboard"}),
        json!({"do": "cancel", "intent_id": 1}),
        json!({"do": "start", "intent": "sail_to", "target": "Mark1", "params": {"trim": "manual"}}),
    ];
    // (the invalid action, the code it is refused with, its phase)
    let invalid = [
        (
            json!({"do": "set", "controls": {"sheet": 0.5}, "bogus": 1}),
            "request.unknown_field",
            1,
        ),
        (
            json!({"do": "start", "intent": "come_to_heading", "heading_deg": 45}),
            "request.misplaced_field",
            1,
        ),
        (
            json!({"do": "start", "intent": "come_to_heading", "params": {"heading_deg": 400}}),
            "request.out_of_range",
            1,
        ),
        (
            json!({"do": "set", "controls": {"ruder": 0.1}}),
            "action.unknown_control",
            2,
        ),
        (
            json!({"do": "start", "intent": "come_to_headin"}),
            "action.unknown_intent",
            2,
        ),
        (
            json!({"do": "use", "entity": "Crate4", "verb": "take"}),
            "action.unknown_verb",
            2,
        ),
        (
            json!({"do": "pulse", "control": "interact", "target": "Crate99"}),
            "perception.unknown_entity",
            2,
        ),
        (
            json!({"do": "set", "controls": {"hoist": 2}}),
            "request.out_of_range",
            2,
        ),
        (
            json!({"do": "pulse", "control": "interact", "target": "Mark1"}),
            "action.wrong_target_kind",
            2,
        ),
        (
            json!({"do": "pulse", "control": "interact", "target": "Crate7"}),
            "action.unavailable",
            3,
        ),
        (
            json!({"do": "cancel", "intent_id": 999}),
            "intent.unknown_id",
            3,
        ),
        (
            json!({"do": "start", "intent": "sail_to", "target": {"x": 400, "z": 10}}),
            "sail.target_on_land",
            3,
        ),
    ];
    let mut sim = scene();
    let before = state(&sim);
    let mut checked = 0;
    for v in &valid {
        let alone = json!({"actions": [v]});
        // Each valid action is valid alone (checked on a fork of the world, by applying it there).
        let mut fork = scene();
        act(&mut fork, alone).unwrap_or_else(|p| panic!("{v}: {p:?}"));
        for (bad, code, phase) in &invalid {
            let call = json!({"actions": [v, bad]});
            let p = act(&mut sim, call.clone()).expect_err(&call.to_string());
            let codes = codes_of(&p);
            // A phase reports every problem it found; a later phase never runs after it.
            assert!(
                codes.iter().any(|c| c == code),
                "{call}: expected {code} (phase {phase}), got {codes:?}"
            );
            assert_eq!(state(&sim), before, "{call} changed the world");
            checked += 1;
        }
    }
    assert_eq!(checked, valid.len() * invalid.len());
}

#[test]
fn a_later_phase_never_runs_after_an_earlier_one_found_a_problem() {
    let mut sim = scene();
    // A shape problem and a names problem: only the shape one is reported.
    let p = act(
        &mut sim,
        json!({"actions": [{"do": "set", "controls": {"ruder": 0.1}},
                           {"do": "set", "controls": {}, "bogus": true}]}),
    )
    .unwrap_err();
    assert_eq!(codes_of(&p), ["request.unknown_field"]);
    // Two names problems: both are reported.
    let p = act(
        &mut sim,
        json!({"actions": [{"do": "set", "controls": {"ruder": 0.1}},
                           {"do": "start", "intent": "trim"}]}),
    )
    .unwrap_err();
    assert_eq!(
        codes_of(&p),
        ["action.unknown_control", "action.unknown_intent"]
    );
    // `do` at the top level, without `actions`.
    let p = act(&mut sim, json!({"do": "set", "controls": {"rudder": 0}})).unwrap_err();
    assert_eq!(p.code, "request.misplaced_field");
    assert_eq!(p.detail["belongs_at"], json!("/actions/0"));
}

/// Up to `n` single-edit variants of `name`: the camelCase spelling and the name without its
/// unit suffix first, then letters dropped, doubled, swapped and changed (README, Names; errors.md,
/// Suggestions), in a fixed order.
fn variants(name: &str, others: &[&str], n: usize) -> Vec<String> {
    let c: Vec<char> = name.chars().collect();
    let mut out: Vec<String> = Vec::new();
    if name.contains('_') {
        let mut camel = String::new();
        let mut up = false;
        for ch in name.chars() {
            if ch == '_' {
                up = true;
            } else if up {
                camel.extend(ch.to_uppercase());
                up = false;
            } else {
                camel.push(ch);
            }
        }
        out.push(camel);
    }
    for suffix in ["_deg", "_mps", "_m", "_s"] {
        if let Some(stem) = name.strip_suffix(suffix) {
            out.push(stem.to_owned());
        }
    }
    for i in 0..c.len() {
        let mut d = c.clone();
        d.remove(i);
        out.push(d.into_iter().collect());
        let mut d = c.clone();
        d.insert(i, c[i]);
        out.push(d.into_iter().collect());
        if i + 1 < c.len() && c[i] != c[i + 1] {
            let mut d = c.clone();
            d.swap(i, i + 1);
            out.push(d.into_iter().collect());
        }
    }
    let mut k = 0usize;
    let letters: Vec<char> = ('a'..='z').collect();
    while out.len() < 4 * n && k < 26 * c.len() {
        // Every position, letters in a stride that visits all 26.
        let i = k % c.len();
        let l = letters[(k / c.len() * 7 + i) % 26];
        k += 1;
        if l != c[i] {
            let mut d = c.clone();
            d[i] = l;
            out.push(d.into_iter().collect());
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    out.retain(|v| {
        !v.is_empty() && v != name && !others.contains(&v.as_str()) && seen.insert(v.clone())
    });
    out.truncate(n);
    out
}

/// Renames the key `field` of the object at `path` (keys) in `req` to `to`.
fn renamed(req: &Value, path: &[&str], field: &str, to: &str) -> Value {
    let mut out = req.clone();
    let mut at = &mut out;
    for p in path {
        at = match p.parse::<usize>() {
            Ok(i) => &mut at[i],
            Err(_) => &mut at[*p],
        };
    }
    let obj = at.as_object_mut().expect("an object");
    let v = obj.remove(field).expect("the field");
    obj.insert(to.to_owned(), v);
    out
}

fn suggested(p: &Problem, field: &str) -> bool {
    std::iter::once(p.clone()).chain(p.also()).any(|p| {
        p.code == "request.unknown_field"
            && p.detail
                .get("suggestions")
                .and_then(Value::as_array)
                .is_some_and(|s| s.iter().any(|x| x == field))
    })
}

#[test]
fn every_field_misspelled_is_refused_with_a_suggestion_and_nothing_applied() {
    let mut sim = scene();
    let before = state(&sim);
    let player = Caller::Player {
        seat: "skipper".into(),
    };
    type Call = fn(&mut Sim, &Caller, &Value) -> Result<Vec<Problem>, Problem>;
    let act_call: Call =
        |sim, c, v| pocket_interface::action::act(sim.world_mut(), c, v).map(|d| d.warnings);
    fn warnings(answer: Value) -> Vec<Problem> {
        serde_json::from_value(answer["warnings"].clone()).unwrap_or_default()
    }
    let intents_call: Call =
        |sim, c, v| pocket_interface::action::intents(sim.world(), c, v).map(warnings);
    let aff_call: Call =
        |sim, c, v| pocket_interface::action::affordances(sim.world(), c, v).map(warnings);
    // (the call, a valid request using every field, the objects whose keys are fields)
    let cases: Vec<(Call, Value, Vec<Vec<&str>>)> = vec![
        (
            act_call,
            json!({"seat": "skipper", "budget_tokens": 200, "resume": false, "actions": [
                {"do": "start", "intent": "come_to_heading", "tag": "a",
                 "params": {"heading_deg": 45, "tolerance_deg": 5, "turn": "port", "settle_s": 2,
                            "keep": true, "timeout_s": 60}}]}),
            vec![vec![], vec!["actions", "0"], vec!["actions", "0", "params"]],
        ),
        (
            act_call,
            json!({"actions": [{"do": "start", "intent": "trim_sail",
                 "params": {"sheet": "best", "hoist": 1, "tolerance": 0.02, "keep": true,
                            "timeout_s": 20}}]}),
            vec![vec!["actions", "0", "params"]],
        ),
        (
            act_call,
            json!({"actions": [{"do": "start", "intent": "sail_to", "target": {"x": 0, "y": 0, "z": -200},
                 "params": {"arrive_m": 10, "trim": "auto", "timeout_s": 900}}]}),
            vec![
                vec!["actions", "0"],
                vec!["actions", "0", "params"],
                vec!["actions", "0", "target"],
            ],
        ),
        (
            act_call,
            json!({"actions": [{"do": "use", "entity": "Crate4", "verb": "take_aboard", "params": {},
                                "tag": "t"}]}),
            vec![vec!["actions", "0"]],
        ),
        (
            act_call,
            json!({"actions": [{"do": "set", "controls": {"rudder": 0.2}},
                               {"do": "pulse", "control": "interact", "target": "Crate4"}]}),
            vec![vec!["actions", "0"], vec!["actions", "1"]],
        ),
        (
            act_call,
            json!({"actions": [{"do": "cancel", "intent_id": 1}]}),
            vec![vec!["actions", "0"]],
        ),
        (
            intents_call,
            json!({"seat": "skipper", "ids": [1], "active_only": false, "budget_tokens": 100}),
            vec![vec![]],
        ),
        (
            aff_call,
            json!({"seat": "skipper", "entity": "Crate4", "kinds": ["crate"], "within_m": 50,
                   "available_only": false, "budget_tokens": 100}),
            vec![vec![]],
        ),
    ];
    let mut refused = 0;
    let mut aliases = 0;
    for (call, req, objects) in &cases {
        // The template itself is accepted (on a fork for an act, which applies it).
        let mut fork = scene();
        call(&mut fork, &player, req).unwrap_or_else(|p| panic!("{req}: {p:?}"));
        for path in objects {
            let mut at = req;
            for p in path {
                at = match p.parse::<usize>() {
                    Ok(i) => &at[i],
                    Err(_) => &at[*p],
                };
            }
            let fields: Vec<&str> = at.as_object().unwrap().keys().map(String::as_str).collect();
            for field in &fields {
                for v in variants(field, &fields, 64) {
                    let bad = renamed(req, path, field, &v);
                    match call(&mut sim, &player, &bad) {
                        Err(p) => {
                            assert!(
                                suggested(&p, field),
                                "{bad}: {field} misspelled {v} is refused without suggesting it:                                  {p:?}"
                            );
                            assert_eq!(state(&sim), before, "{bad} changed the world");
                            refused += 1;
                        }
                        Ok(w) => {
                            // A declared alias (a unit suffix left out): read as the field, said.
                            assert!(
                                w.iter().any(|w| w.code == "request.alias_used"
                                    && w.detail["field"] == json!(field)),
                                "{bad}: {field} misspelled {v} was accepted"
                            );
                            aliases += 1;
                            sim = scene();
                        }
                    }
                }
            }
        }
    }
    println!("{refused} misspellings refused, {aliases} read as declared aliases");
    assert!(refused > 2000, "{refused}");
}

/// The game's world (perception and the sailing catalog, `action::web`), the skipper's caller.
fn game() -> (Sim, Caller) {
    let sim = pocket_interface::action::web::world().unwrap();
    let skipper = Caller::Player {
        seat: "skipper".into(),
    };
    (sim, skipper)
}

fn sloop_rudder_bits(sim: &Sim) -> u64 {
    let w = sim.world();
    let index = w.resource::<pocket_sim::EntityIndex>();
    let (_, e) = index
        .iter()
        .find(|(_, e)| {
            w.get::<pocket_sim::Name>(*e)
                .is_some_and(|n| n.as_str() == "Sloop")
        })
        .unwrap();
    w.get::<pocket_physics::Boat>(e).unwrap().rudder.to_bits()
}

/// A `set` of `-0.0` latches what its canonical record says: the call applied again from the
/// record (as a replay applies `act.apply`) gives the same world, tick for tick (persistence.md
/// hashes an `f64` by its bits).
#[test]
fn a_recorded_set_of_negative_zero_replays_to_the_same_world() {
    let (mut live, skipper) = game();
    let (mut replayed, _) = game();
    let raw: Value =
        serde_json::from_str(r#"{"actions":[{"do":"set","controls":{"rudder":-0.0}}]}"#).unwrap();
    let done = pocket_interface::action::act(live.world_mut(), &skipper, &raw).unwrap();
    assert_eq!(
        done.canonical,
        json!({"seat": "skipper", "actions": [{"do": "set", "controls": {"rudder": 0}}]})
    );
    pocket_interface::action::act(replayed.world_mut(), &skipper, &done.canonical).unwrap();
    assert_eq!(sloop_rudder_bits(&live), 0, "positive zero latched");
    assert_eq!(sloop_rudder_bits(&live), sloop_rudder_bits(&replayed));
    for _ in 0..5 {
        assert_eq!(ledger_hash(&live), ledger_hash(&replayed));
        step(&mut live);
        step(&mut replayed);
    }
    assert_eq!(ledger_hash(&live), ledger_hash(&replayed));
}

/// A seat whose body has no `Boat` has no sailing control and no sailing intent: a call that names
/// them is refused in phase 2 with what the body does take, and nothing of it is applied, so the
/// next tick runs; an intent whose body loses its `Boat` fails, and the tick does not.
#[test]
fn a_body_without_a_boat_takes_no_sailing_control() {
    let (mut sim, skipper) = game();
    sim.boundary()
        .spawn((
            pocket_sim::Name::new("Deck").unwrap(),
            pocket_physics::Transform::at([50.0, 0.0, 50.0]),
            pocket_interface::perception::Observer {
                profile: "skipper".into(),
                seat: Some("deck".into()),
                omniscient: false,
                team: None,
            },
        ))
        .unwrap();
    step(&mut sim);
    let deck = Caller::Player {
        seat: "deck".into(),
    };
    let before = state(&sim);
    let p = pocket_interface::action::act(
        sim.world_mut(),
        &deck,
        &json!({"actions": [{"do": "start", "intent": "trim_sail", "params": {"hoist": 1}},
                            {"do": "set", "controls": {"rudder": 0.1}}]}),
    )
    .unwrap_err();
    assert_eq!(
        codes_of(&p),
        ["action.unknown_intent", "action.unknown_control"],
        "{p:?}"
    );
    assert_eq!(state(&sim), before, "nothing applied");
    let p = pocket_interface::action::act(
        sim.world_mut(),
        &deck,
        &json!({"actions": [{"do": "set", "controls": {"rudder": 0.1}}]}),
    )
    .unwrap_err();
    assert_eq!(p.code, "action.unknown_control");
    assert_eq!(p.detail["allowed"], json!(["interact"]), "{p:?}");
    assert_eq!(
        p.message,
        "Seat deck has no control 'rudder'; it takes interact."
    );
    assert_eq!(state(&sim), before, "nothing applied");
    sim.step(&mut pocket_sim::NoHooks).unwrap();
    assert!(sim.poisoned().is_none());
    // The skipper's trim, its Sloop's Boat removed at a boundary: the intent fails, the tick runs.
    let done = pocket_interface::action::act(
        sim.world_mut(),
        &skipper,
        &json!({"actions": [{"do": "start", "intent": "trim_sail", "params": {"keep": true}}]}),
    )
    .unwrap();
    let id = started(&done);
    step(&mut sim);
    let w = sim.world_mut();
    let sloop = w
        .resource::<pocket_sim::EntityIndex>()
        .iter()
        .find(|(_, e)| {
            w.get::<pocket_sim::Name>(*e)
                .is_some_and(|n| n.as_str() == "Sloop")
        })
        .map(|(_, e)| e)
        .unwrap();
    w.entity_mut(sloop).remove::<pocket_physics::Boat>();
    sim.step(&mut pocket_sim::NoHooks).unwrap();
    assert!(sim.poisoned().is_none());
    let i = intent(&sim, id);
    assert_eq!(
        (i.status, i.failure.map(|f| f.code)),
        (
            pocket_interface::action::IntentStatus::Failed,
            Some("internal.error".to_owned())
        )
    );
}

/// `intents` and `affordances` answer within `budget_tokens`: the longest prefix of the list
/// that fits, the rest counted in `omitted`, and `tokens`, the estimate of the answer without that
/// member; a budget that cannot hold the empty list is refused with its minimum.
#[test]
fn intents_and_affordances_answer_within_their_budget() {
    let mut sim = scene();
    for h in [10, 20, 30] {
        act(
            &mut sim,
            json!({"actions": [{"do": "start", "intent": "come_to_heading",
                                "params": {"heading_deg": h}}]}),
        )
        .unwrap();
    }
    let me = Caller::Player {
        seat: "skipper".into(),
    };
    let all = pocket_interface::action::intents(sim.world(), &me, &json!({})).unwrap();
    let n = all["intents"].as_array().unwrap().len();
    // The trim, two headings superseded and the live one.
    assert_eq!((n, all["omitted"].clone()), (4, json!(0)), "{all}");
    let tokens = |v: &Value| {
        let mut m = v.as_object().unwrap().clone();
        m.remove("tokens");
        Value::Object(m).to_string().len().div_ceil(4) as u64
    };
    assert_eq!(all["tokens"].as_u64(), Some(tokens(&all)));
    let mut last = n;
    for budget in [400usize, 200, 120, 60] {
        let a =
            pocket_interface::action::intents(sim.world(), &me, &json!({"budget_tokens": budget}))
                .unwrap();
        let kept = a["intents"].as_array().unwrap();
        assert_eq!(kept.len() as u64 + a["omitted"].as_u64().unwrap(), n as u64);
        assert!(kept.len() <= last, "a smaller budget never keeps more");
        assert_eq!(kept[..], all["intents"].as_array().unwrap()[..kept.len()]);
        assert_eq!(a["tokens"].as_u64(), Some(tokens(&a)));
        assert!(a.to_string().len() <= budget * 4, "{budget}: {a}");
        last = kept.len();
    }
    assert!(last < n, "the smallest budget left something out");
    let p = pocket_interface::action::intents(sim.world(), &me, &json!({"budget_tokens": 5}))
        .unwrap_err();
    assert_eq!(p.code, "perception.budget_too_small");
    let p = pocket_interface::action::intents(sim.world(), &me, &json!({"budget_tokens": 16001}))
        .unwrap_err();
    assert_eq!(p.code, "request.out_of_range");
    let a = pocket_interface::action::affordances(sim.world(), &me, &json!({})).unwrap();
    assert!(a.get("omitted").is_none(), "nothing left out: {a}");
    assert_eq!(a["tokens"].as_u64(), Some(tokens(&a)));
    let listed = a["affordances"].as_array().unwrap().len();
    assert!(listed >= 2, "{a}");
    let min = pocket_interface::action::affordances(sim.world(), &me, &json!({"budget_tokens": 1}))
        .unwrap_err();
    let least = min.detail["min_tokens"].as_u64().unwrap();
    let cut =
        pocket_interface::action::affordances(sim.world(), &me, &json!({"budget_tokens": least}))
            .unwrap();
    assert_eq!(cut["affordances"], json!([]));
    assert_eq!(cut["omitted"].as_u64(), Some(listed as u64));
}

/// A call that validated and then fails while applying (an engine bug: here a rudder whose write
/// fails though its body takes it) is undone whole: the intent it started first, its counter, its
/// events and the world are as they were, and the answer is `internal.error`.
#[test]
fn a_call_that_fails_while_applying_is_undone_whole() {
    let mut sim = scene();
    let mut c = sim
        .world()
        .resource::<pocket_interface::action::ActionCatalog>()
        .clone();
    let rudder = c.controls.iter_mut().find(|d| d.name == "rudder").unwrap();
    rudder.binding = pocket_interface::action::defs::ControlBinding::Engine {
        write: |_, _, _| {
            Err(pocket_contract::codes::internal_error(
                "test",
                "a write that fails",
            ))
        },
        read: |_, _| None,
        writable: |_, _| true,
    };
    sim.world_mut().insert_resource(c);
    let before = state(&sim);
    let table = sim.world().resource::<IntentTable>().clone();
    let p = act(
        &mut sim,
        json!({"actions": [{"do": "start", "intent": "trim_sail", "params": {"sheet": 0.5}},
                           {"do": "set", "controls": {"rudder": 0.1}}]}),
    )
    .unwrap_err();
    assert_eq!(p.code, "internal.error", "{p:?}");
    assert_eq!(
        state(&sim),
        before,
        "the started intent and its events are undone"
    );
    assert_eq!(*sim.world().resource::<IntentTable>(), table);
    step(&mut sim);
    assert!(sim.poisoned().is_none());
}
