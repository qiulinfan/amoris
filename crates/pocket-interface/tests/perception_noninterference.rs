//! `contract.perception.noninterference` (shared/contract/perception.md, Checks), perception's
//! part: from the sailing probe at a tick, a mutation the skipper cannot perceive is applied (a
//! hidden fact changed, an entity outside every range moved within that region, a crate behind an
//! island respawned elsewhere behind it, a hidden-scope event, an event of an undeclared kind, a
//! charted mark it has not seen moved), then a fixed scripted player session (every perception
//! query in each projection, refused calls among them, with steps between and a fixed script of
//! events about the entities the mutations touch) runs on the mutated world and on the unmutated
//! control. Every answer must be byte-identical. 256 cases drawn from a
//! seeded PCG32, each case a random set of mutations with random values; a failure names its seed.
//!
//! The session tools beyond perception (`act`, `step` with `until`, `intents`, `replay`) and the
//! mutation classes beyond it (a requested decision, another seat's clock) are the action, time
//! and MCP layers' and are checked with them (docs/spec/perception-slice2.md, choice 7).

use pocket_contract::{CheckOptions, decode};
use pocket_interface::perception::probe::{self, Ids, PLAYER};
use pocket_interface::perception::{
    DescribeRequest, EntityRef, EventsRequest, NearbyRequest, NoAffordances, ObserveRequest,
    SeatParts, Sector, Visibility, describe, events, nearby, observe,
};
use pocket_interface::projection::Projection;
use pocket_physics::probe::fork;
use pocket_physics::{Collider, Transform, sailing};
use pocket_sim::{EventKind, Name, NewEvent, NoHooks, Pcg32, PlainData, Sim};
use serde_json::{Value, json};

/// The tick the mutations are applied at.
const AT: u64 = 30;
/// The session: rounds of every query, with this many ticks between them.
const ROUNDS: usize = 4;
const GAP: u64 = 15;
const CASES: u64 = 256;

fn answer(r: Result<pocket_interface::perception::Answer, pocket_contract::Problem>) -> String {
    match r {
        Ok(a) => a.body,
        Err(p) => format!(
            "refused {} {} {}",
            p.code,
            p.message,
            Value::Object(p.detail)
        ),
    }
}

/// One round of the scripted player session: every query kind in each projection.
fn calls(sim: &Sim, ids: &Ids) -> Vec<String> {
    let w = sim.world();
    let parts = SeatParts::default();
    let aff = NoAffordances;
    let mut out = probe::answers(sim);
    for projection in [Projection::Text, Projection::Json, Projection::Tensor] {
        let req = ObserveRequest {
            projection: Some(projection),
            since: (projection != Projection::Tensor).then_some(2),
            ..ObserveRequest::default()
        };
        out.push(answer(observe(w, &PLAYER, &req, &parts, &aff)));
    }
    let nearbys = [
        json!({"within_m": 500, "sector": {"from_deg": -30, "to_deg": 30, "relative": true}}),
        json!({"kinds": ["crate"], "projection": "json"}),
        json!({"visibility": ["remembered", "charted"], "budget_tokens": 60}),
        json!({"sector": {"from_deg": 300, "to_deg": 60}, "limit": 2}),
        json!({"kinds": ["kraken"]}),
    ];
    for n in nearbys {
        let req: NearbyRequest = serde_json::from_value(n).expect("a request");
        out.push(answer(nearby(w, &PLAYER, &req, &[], &aff)));
    }
    let mut refs: Vec<EntityRef> = [
        "Mark1", "Mark2", "Isle", "Islet1", "Islet2", "Crate1", "Crate2", "Crate3", "Ketch",
        "Sloop", "Sea", "Breeze", "Crate",
    ]
    .iter()
    .map(|n| EntityRef::Text((*n).to_owned()))
    .collect();
    refs.extend(
        [
            ids.hidden_crate,
            ids.far_crate,
            ids.far_mark,
            ids.near_crate,
        ]
        .iter()
        .map(|id| EntityRef::Id(id.get())),
    );
    refs.push(EntityRef::Text(format!(
        "Crate2#{}",
        ids.hidden_crate.get()
    )));
    for (i, r) in refs.into_iter().enumerate() {
        let projection = if i % 2 == 0 {
            Projection::Text
        } else {
            Projection::Json
        };
        let req = DescribeRequest {
            seat: None,
            entity: r,
            budget_tokens: None,
            projection: Some(projection),
            omniscient: None,
        };
        out.push(answer(describe(w, &PLAYER, &req, &aff)));
    }
    for (since, kinds) in [
        (0, None),
        (1, Some(vec!["race.".to_owned(), "sighted".to_owned()])),
        (3, Some(vec!["secret.signal".to_owned()])),
    ] {
        let req = EventsRequest {
            since,
            kinds,
            projection: Some(Projection::Json),
            ..EventsRequest::default()
        };
        out.push(answer(events(w, &PLAYER, &req)));
    }
    // Refused calls: the omniscient view, another seat, a misspelt field, an unknown kind.
    let omni = ObserveRequest {
        omniscient: Some(true),
        ..ObserveRequest::default()
    };
    out.push(answer(observe(w, &PLAYER, &omni, &parts, &aff)));
    let other = NearbyRequest {
        seat: Some("lookout".to_owned()),
        ..NearbyRequest::default()
    };
    out.push(answer(nearby(w, &PLAYER, &other, &[], &aff)));
    let misspelt =
        decode::<ObserveRequest>(&json!({"budget_token": 100}), &CheckOptions::new("observe"));
    out.push(match misspelt {
        Ok(_) => "accepted".to_owned(),
        Err(p) => format!("refused {} {}", p.code, p.message),
    });
    out
}

/// The events the session's world emits before each round's ticks, the same in the control and
/// the mutated runs, about what the skipper cannot see: a global and a sound event about the far
/// crate, a sight event about the crate behind the Isle, a global event about the charted mark it
/// has not seen, and a global event with an `at_m` member. Their scopes admit the first, the
/// fourth and the fifth; what the skipper perceives of them must not follow the mutations.
fn script(sim: &mut Sim, ids: &Ids, round: usize) {
    let n = u64::try_from(round).expect("small");
    let at = json!({"n": n, "at_m": {"x": -2900.0, "y": 0.0, "z": -2900.0}});
    let events = [
        ("race.start", Some(ids.far_crate), json!({"n": n})),
        ("horn.blast", Some(ids.far_crate), json!({"n": n})),
        (
            "crate.taken",
            Some(ids.hidden_crate),
            json!({"taken": n, "left": 9}),
        ),
        ("race.start", Some(ids.far_mark), json!({"n": n})),
        ("race.start", None, at),
    ];
    for (kind, subject, data) in events {
        let mut e =
            NewEvent::new(EventKind::new(kind).expect("a kind")).data(PlainData::from_json(&data));
        if let Some(s) = subject {
            e = e.subject(s);
        }
        sim.boundary().emit(e);
    }
}

/// The whole session from `sim`: a round, then the event script and `GAP` ticks, `ROUNDS` times.
fn session(mut sim: Sim, ids: &Ids) -> Vec<String> {
    let mut out = Vec::new();
    for round in 0..ROUNDS {
        if round > 0 {
            script(&mut sim, ids, round);
            for _ in 0..GAP {
                sim.step(&mut NoHooks).expect("a tick");
            }
        }
        out.extend(calls(&sim, ids));
    }
    out
}

/// What a case changed, for its failure message.
#[derive(Debug, Default)]
struct Applied(Vec<String>);

/// Applies a random non-empty set of the mutation classes, with random values.
fn mutate(sim: &mut Sim, ids: &Ids, rng: &mut Pcg32) -> Applied {
    let mut done = Applied::default();
    let mut mask = 0u32;
    while mask == 0 {
        mask = rng.below(64).expect("a draw");
    }
    let r = |rng: &mut Pcg32, lo: f64, hi: f64| rng.range(lo, hi).expect("a draw");
    if mask & 1 != 0 {
        // A hidden fact: an island's restitution.
        let islands = [ids.isle, ids.islet, ids.far_islet];
        let which = islands[rng.pick(3).expect("a draw")];
        let v = r(rng, 0.0, 1.0);
        let e = sim.boundary().entity(which).expect("live");
        sim.world_mut()
            .get_mut::<Collider>(e)
            .expect("a collider")
            .restitution = v;
        done.0.push(format!("restitution of {which:?} = {v}"));
    }
    if mask & 2 != 0 {
        // Outside every range, moved within that region.
        let to = [r(rng, -3500.0, -2600.0), 0.0, r(rng, -3500.0, -2600.0)];
        let e = sim.boundary().entity(ids.far_crate).expect("live");
        sim.world_mut()
            .get_mut::<Transform>(e)
            .expect("a transform")
            .position = to;
        done.0.push(format!("far crate to {to:?}"));
    }
    if mask & 4 != 0 {
        // Behind the Isle, respawned elsewhere behind it.
        let to = [r(rng, -6.0, 6.0), 0.0, r(rng, -105.0, -92.0)];
        let mut b = sim.boundary();
        b.despawn(ids.hidden_crate).expect("despawns");
        let id = b
            .spawn((
                Name::new("Crate2").expect("a name"),
                sailing::crate_box(to, r(rng, 0.0, 90.0)),
            ))
            .expect("spawns");
        let e = b.entity(id).expect("live");
        b.world_mut()
            .entity_mut(e)
            .insert(pocket_interface::perception::Perceivable {
                kind: "crate".into(),
                detect_m: 120.0,
                height_m: 0.5,
                priority: 40,
                chart_m: None,
            });
        done.0
            .push(format!("hidden crate respawned as {id:?} at {to:?}"));
    }
    if mask & 8 != 0 {
        // A hidden-scope event, about the skipper's own boat, where it is.
        let n = rng.below(1000).expect("a draw");
        let e = NewEvent::new(EventKind::new("secret.signal").expect("a kind"))
            .subject(ids.sloop)
            .data(PlainData::from_json(
                &json!({"n": n, "at_m": {"x": 1.0, "y": 0.0, "z": 1.0}}),
            ));
        sim.boundary().emit(e);
        done.0.push(format!("secret.signal n={n}"));
    }
    if mask & 16 != 0 {
        // An event of an undeclared kind.
        let n = rng.below(1000).expect("a draw");
        let e = NewEvent::new(EventKind::new("debug.leak").expect("a kind"))
            .subject(ids.sloop)
            .data(PlainData::from_json(&json!({"n": n})));
        sim.boundary().emit(e);
        done.0.push(format!("debug.leak n={n}"));
    }
    if mask & 32 != 0 {
        // A charted mark the skipper has not seen, moved (its chart position stays).
        let to = [r(rng, 2300.0, 2700.0), 0.0, r(rng, -2700.0, -2300.0)];
        let e = sim.boundary().entity(ids.far_mark).expect("live");
        sim.world_mut()
            .get_mut::<Transform>(e)
            .expect("a transform")
            .position = to;
        done.0.push(format!("far mark to {to:?}"));
    }
    done
}

#[test]
fn hidden_changes_change_no_player_answer() {
    let (base, ids) = probe::world(AT);
    let ledger = probe::ledger();
    // The preconditions the mutation classes rest on.
    {
        let v = pocket_interface::perception::PerceptionView::for_seat(base.world(), "skipper")
            .expect("the skipper's view");
        assert!(
            v.percept(ids.hidden_crate).is_none(),
            "the crate behind the Isle is unseen"
        );
        assert!(
            v.percept(ids.far_crate).is_none(),
            "the far crate is unknown"
        );
        let mark = v.percept(ids.far_mark).expect("charted");
        assert_eq!(mark.visibility, Visibility::Charted);
        assert_eq!(
            v.percept(ids.isle).expect("seen").visibility,
            Visibility::Seen
        );
    }
    let control = session(fork(&ledger, &base, probe::fresh).expect("a fork"), &ids);
    assert!(control.len() > 100);
    // The script's global event about the unseen charted mark reaches the skipper: named, as the
    // chart names it, and without a bearing or a range.
    let mark = format!(
        "\"subject\":{{\"id\":{},\"name\":\"Mark2\"}},\"sense\":\"global\",\"data\"",
        ids.far_mark.get()
    );
    assert!(
        control.iter().any(|s| s.contains(&mark)),
        "no global event about Mark2 in the session"
    );
    // A refusal of an unknown name echoes the caller's own words; nothing else names a secret.
    for s in control
        .iter()
        .filter(|s| !s.starts_with("refused perception.unknown_entity"))
    {
        for secret in [
            "Crate3",
            "restitution",
            "secret.",
            "debug.",
            "Breeze#",
            "Sea#",
        ] {
            assert!(!s.contains(secret), "a player answer shows {secret}: {s}");
        }
        assert!(
            !s.contains("see Crate2") && !s.contains("\"name\":\"Crate2\""),
            "{s}"
        );
    }
    let mut rng = Pcg32::new(20_261_003, 54);
    for case in 0..CASES {
        let seed = rng.next_u64();
        let mut case_rng = Pcg32::new(seed, case);
        let mut sim = fork(&ledger, &base, probe::fresh).expect("a fork");
        let applied = mutate(&mut sim, &ids, &mut case_rng);
        let got = session(sim, &ids);
        assert_eq!(got.len(), control.len());
        if let Some(i) = got.iter().zip(&control).position(|(a, b)| a != b) {
            panic!(
                "case {case} (seed {seed:#x}): {:?}\nanswer {i} differs\nmutated:\n{}\ncontrol:\n{}",
                applied.0, got[i], control[i]
            );
        }
    }
}

#[test]
fn a_change_the_skipper_can_perceive_does_change_its_answers() {
    // The negative control: the session does notice what perception grants. Each change alone.
    let (base, ids) = probe::world(AT);
    let ledger = probe::ledger();
    let control = session(fork(&ledger, &base, probe::fresh).expect("a fork"), &ids);
    type Change = fn(&mut Sim, &Ids);
    let changes: [(&str, Change); 3] = [
        ("the near crate moved", |sim, ids| {
            let e = sim.boundary().entity(ids.near_crate).expect("live");
            sim.world_mut()
                .get_mut::<Transform>(e)
                .expect("a transform")
                .position = [25.0, 0.0, 9.0];
        }),
        ("a global event", |sim, _| {
            let e = NewEvent::new(EventKind::new("race.start").expect("a kind"))
                .data(PlainData::from_json(&json!({"n": 1})));
            sim.boundary().emit(e);
        }),
        ("the hidden crate moved into sight", |sim, ids| {
            let e = sim.boundary().entity(ids.hidden_crate).expect("live");
            sim.world_mut()
                .get_mut::<Transform>(e)
                .expect("a transform")
                .position = [60.0, 0.0, -80.0];
        }),
    ];
    for (what, change) in changes {
        let mut sim = fork(&ledger, &base, probe::fresh).expect("a fork");
        change(&mut sim, &ids);
        assert_ne!(session(sim, &ids), control, "{what} went unnoticed");
    }
}

#[test]
fn a_sector_and_a_kind_filter_see_no_more() {
    // `nearby` with every filter combination returns a subset of the unfiltered answer's percepts.
    let (sim, _) = probe::world(AT);
    let all = nearby(
        sim.world(),
        &PLAYER,
        &NearbyRequest {
            projection: Some(Projection::Text),
            ..Default::default()
        },
        &[],
        &NoAffordances,
    )
    .expect("an answer")
    .body;
    let sector = NearbyRequest {
        sector: Some(Sector {
            from_deg: 0.0,
            to_deg: 180.0,
            relative: false,
        }),
        projection: Some(Projection::Text),
        ..NearbyRequest::default()
    };
    let some = nearby(sim.world(), &PLAYER, &sector, &[], &NoAffordances)
        .expect("an answer")
        .body;
    for line in some.lines().skip(1) {
        assert!(all.contains(line), "{line}");
    }
}
