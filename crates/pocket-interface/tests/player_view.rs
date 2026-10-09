//! The omniscient view never reaches a player seat through actions (shared/contract/actions.md,
//! Executors rule 1 and Affordances; README, Seats and callers): an entity the seat does not
//! perceive is refused exactly as one that does not exist, listings leave it out, a player cannot
//! act, list or read for another seat, and nothing a player is answered is marked omniscient.

mod common;

use common::ledger_hash;
use common::play::*;
use pocket_interface::action::{self, Caller};
use pocket_interface::perception::Observer;
use pocket_sim::Sim;
use serde_json::{Value, json};

fn refusal(r: Result<impl std::fmt::Debug, pocket_contract::Problem>) -> pocket_contract::Problem {
    r.expect_err("refused")
}

fn mentions(v: &Value, name: &str) -> bool {
    v.to_string().contains(name)
}

/// The Sloop (the skipper) with a crate near, a crate far beyond sight, and a rival boat seated at
/// `rival`, also beyond sight; perception has run.
fn scene() -> (Sim, u64, u64) {
    let (mut sim, _) = world(90.0, 270.0, 6.0, 1);
    crate_at(&mut sim, "NearCrate", [30.0, 0.0, 0.0]);
    let far = crate_at(&mut sim, "FarCrate", [0.0, 0.0, 1500.0]);
    let rival = other_boat(&mut sim, "Rival", [0.0, 0.0, -3000.0], 0.0);
    let e = sim.boundary().entity(rival).unwrap();
    sim.world_mut().entity_mut(e).insert(Observer {
        profile: "skipper".into(),
        seat: Some("rival".into()),
        omniscient: false,
        team: None,
    });
    for _ in 0..2 {
        sim.step(&mut pocket_sim::NoHooks).unwrap();
    }
    (sim, far.get(), rival.get())
}

#[test]
fn a_player_is_refused_what_it_does_not_perceive_as_what_does_not_exist() {
    let (mut sim, far, rival) = scene();
    let me = skipper();
    let before = ledger_hash(&sim);
    let refs = [
        json!("FarCrate"),
        json!(far),
        json!("Rival"),
        json!(rival),
        json!("Nowhere"),
        json!(999_999),
    ];
    for r in &refs {
        let calls = [
            json!({"actions": [{"do": "use", "entity": r, "verb": "take_aboard"}]}),
            json!({"actions": [{"do": "pulse", "control": "interact", "target": r}]}),
            json!({"actions": [{"do": "start", "intent": "sail_to", "target": r}]}),
        ];
        for call in calls {
            let p = refusal(action::act(sim.world_mut(), &me, &call));
            assert_eq!(p.code, "perception.unknown_entity", "{call}: {p:?}");
            let detail = serde_json::to_value(&p).unwrap();
            for hidden in ["FarCrate", "Rival"] {
                if r.as_str() != Some(hidden) {
                    assert!(
                        !mentions(&detail, hidden),
                        "{call} names {hidden}: {detail}"
                    );
                }
            }
            assert!(
                !mentions(&detail["detail"]["suggestions"], "FarCrate")
                    && !mentions(&detail["detail"]["suggestions"], "Rival"),
                "{call}: {detail}"
            );
        }
        let p = refusal(action::affordances(sim.world(), &me, &json!({"entity": r})));
        assert_eq!(p.code, "perception.unknown_entity", "{r}: {p:?}");
    }
    // The listings name what the seat perceives only.
    let a = action::affordances(sim.world(), &me, &json!({})).unwrap();
    assert!(mentions(&a, "NearCrate"), "{a}");
    assert!(!mentions(&a, "FarCrate") && !mentions(&a, "Rival"), "{a}");
    assert_eq!(a["omniscient"], json!(false));
    let i = action::intents(sim.world(), &me, &json!({})).unwrap();
    assert_eq!(i["omniscient"], json!(false));
    assert_eq!(ledger_hash(&sim), before, "nothing was applied");
}

#[test]
fn a_player_cannot_act_list_or_read_for_another_seat() {
    let (mut sim, _, _) = scene();
    let me = skipper();
    let before = ledger_hash(&sim);
    let act = json!({"seat": "rival", "actions": [{"do": "set", "controls": {"rudder": 0.5}}]});
    let p = refusal(action::act(sim.world_mut(), &me, &act));
    assert_eq!(p.code, "seat.not_yours", "{p:?}");
    let p = refusal(action::intents(sim.world(), &me, &json!({"seat": "rival"})));
    assert_eq!(p.code, "seat.not_yours", "{p:?}");
    let p = refusal(action::affordances(
        sim.world(),
        &me,
        &json!({"seat": "rival"}),
    ));
    assert_eq!(p.code, "seat.not_yours", "{p:?}");
    assert_eq!(ledger_hash(&sim), before, "nothing was applied");
    // A developer acts and lists for any seat it names.
    let dev = Caller::Developer;
    action::act(sim.world_mut(), &dev, &act).unwrap();
    action::affordances(sim.world(), &dev, &json!({"seat": "rival"})).unwrap();
}

/// `intents` names a target as the seat knows it, never from the world: a target the seat has
/// forgotten is answered the same before and after it is destroyed (the skipper's memory made a
/// second long so it forgets within the test).
#[test]
fn intents_answer_a_forgotten_target_alike_whether_it_exists_or_not() {
    let mut defs = pocket_interface::perception::probe::defs();
    let profile = defs
        .decl
        .observers
        .iter_mut()
        .find(|p| p.name == "skipper")
        .unwrap();
    profile.memory_s = 1.0;
    let (mut sim, _) = world_in(fresh_with(defs, 1), 90.0, 270.0, 6.0, true);
    let me = skipper();
    let id = crate_at(&mut sim, "Probe1", [0.0, 0.0, -20.0]);
    for _ in 0..2 {
        sim.step(&mut pocket_sim::NoHooks).unwrap();
    }
    let done = action::act(
        sim.world_mut(),
        &me,
        &json!({"actions": [{"do": "start", "intent": "sail_to", "target": "Probe1",
                             "params": {"trim": "manual"}}]}),
    )
    .unwrap();
    assert_eq!(
        done.outcomes[0]["target"],
        json!({"id": id.get(), "name": "Probe1"})
    );
    // Out of sight from now on: remembered, then forgotten a second later.
    let e = sim.boundary().entity(id).unwrap();
    sim.world_mut()
        .get_mut::<pocket_interface::perception::Perceivable>(e)
        .unwrap()
        .detect_m = 0.01;
    for _ in 0..120 {
        sim.step(&mut pocket_sim::NoHooks).unwrap();
    }
    let view = pocket_interface::perception::PerceptionView::for_seat(sim.world(), "skipper");
    assert!(view.unwrap().percept(id).is_none(), "forgotten");
    let before = action::intents(sim.world(), &me, &json!({})).unwrap();
    assert_eq!(before["intents"][0]["target"], json!({"id": id.get()}));
    assert!(!mentions(&before, "Probe1"), "{before}");
    sim.boundary().despawn(id).unwrap();
    let after = action::intents(sim.world(), &me, &json!({})).unwrap();
    assert_eq!(after, before);
}

/// `Source::Player(index)` names a seat the game declares, and only that one: a seat an observer
/// names without a declaration has no index (a developer may still act for it), so a body that
/// goes never hands a player another seat's identity.
#[test]
fn player_indices_name_declared_seats_only() {
    let (mut sim, _) = world(90.0, 270.0, 6.0, 1);
    let mut bodies = Vec::new();
    for (n, seat, z) in [("Alpha", "alpha", 300.0), ("Bravo", "bravo", 600.0)] {
        let id = other_boat(&mut sim, n, [0.0, 0.0, z], 0.0);
        let e = sim.boundary().entity(id).unwrap();
        sim.world_mut().entity_mut(e).insert(Observer {
            profile: "skipper".into(),
            seat: Some(seat.into()),
            omniscient: false,
            team: None,
        });
        bodies.push(id);
    }
    let by_index = |sim: &Sim| -> Vec<Option<String>> {
        (0..3)
            .map(|i| {
                action::state::seat_by_index(sim.world(), i)
                    .unwrap()
                    .map(|r| r.id)
            })
            .collect()
    };
    let rows: Vec<(String, Option<u32>)> = action::seats(sim.world())
        .unwrap()
        .into_iter()
        .map(|r| (r.id, r.index))
        .collect();
    assert_eq!(
        rows,
        [
            ("skipper".to_owned(), Some(0)),
            ("alpha".to_owned(), None),
            ("bravo".to_owned(), None)
        ]
    );
    assert_eq!(by_index(&sim), [Some("skipper".to_owned()), None, None]);
    sim.boundary().despawn(bodies[0]).unwrap();
    assert_eq!(by_index(&sim), [Some("skipper".to_owned()), None, None]);
    let act = json!({"seat": "bravo", "actions": [{"do": "set", "controls": {"rudder": 0.2}}]});
    action::act(sim.world_mut(), &Caller::Developer, &act).unwrap();
}
