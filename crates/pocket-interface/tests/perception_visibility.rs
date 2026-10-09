//! The visibility unit tests of shared/contract/perception.md (Checks): the range's edge to the
//! ulp, the field of view's edge, bearings on 0, -0 and just below 360, a zero horizontal offset,
//! a mast over a wall, forgetting what is seen gone and keeping what is not, expiry at exactly
//! `memory_s`, the capacity's eviction order (a visible set beyond it kept and sighted once), the
//! view inside a tick, fog, every event scope with its causes, and where a global event is told.

mod perception_support;

use std::sync::{Arc, Mutex};

use perception_support::*;
use pocket_interface::perception::geometry::bearing_deg;
use pocket_interface::perception::probe::PLAYER;
use pocket_interface::perception::{
    Caller, NearbyRequest, NoAffordances, PerceptionView, Role, Sense, Visibility, VisibilityScale,
    nearby,
};
use pocket_interface::projection::Projection;
use pocket_sim::{RunCondition, TickPhase};
use serde_json::json;

const WATCH: Caller<'static> = Caller {
    role: Role::Player,
    seat: Some("watch"),
};

fn near_text(sim: &pocket_sim::Sim) -> String {
    let req = NearbyRequest {
        projection: Some(Projection::Text),
        ..NearbyRequest::default()
    };
    nearby(sim.world(), &WATCH, &req, &[], &NoAffordances)
        .expect("an answer")
        .body
}

fn near_json(sim: &pocket_sim::Sim) -> serde_json::Value {
    let req = NearbyRequest {
        projection: Some(Projection::Json),
        ..NearbyRequest::default()
    };
    let body = nearby(sim.world(), &WATCH, &req, &[], &NoAffordances)
        .expect("an answer")
        .body;
    serde_json::from_str(&body).expect("JSON")
}

#[test]
fn the_range_ends_exactly_at_r_squared() {
    let mut sim = sim();
    let w = watcher(&mut sim, [0.0; 3], 0.0);
    let edge = thing(&mut sim, "Edge", [0.0, 0.0, -100.0], 100.0, 0.0);
    let beyond = f64::from_bits(100f64.to_bits() + 1);
    let past = thing(&mut sim, "Past", [3.0, 0.0, -beyond], 150.0, 0.0);
    let short = thing(&mut sim, "Short", [-3.0, 0.0, -60.0], 59.0, 0.0);
    step(&mut sim);
    let seen = remembered(&sim, w);
    assert!(seen.contains(&edge), "at exactly r the thing is seen");
    // `Past` is at sqrt(9 + beyond^2) > 100 from the eye with a sight of 100.
    assert!(!seen.contains(&past));
    // `detect_m` bounds the range below the sight's.
    assert!(!seen.contains(&short));
    // One ulp beyond the range, straight ahead.
    let mut sim = sim_fresh();
    let w = watcher(&mut sim, [0.0; 3], 0.0);
    let one = thing(&mut sim, "Ulp", [0.0, 0.0, -beyond], 100.0, 0.0);
    step(&mut sim);
    assert!(
        !remembered(&sim, w).contains(&one),
        "one ulp beyond r is out of range"
    );
}

fn sim_fresh() -> pocket_sim::Sim {
    sim()
}

#[test]
fn fog_scales_every_range() {
    let mut sim = sim();
    sim.world_mut().insert_resource(VisibilityScale(0.5));
    let w = watcher(&mut sim, [0.0; 3], 0.0);
    let near = thing(&mut sim, "Near", [0.0, 0.0, -50.0], 100.0, 0.0);
    let far = thing(&mut sim, "Far", [5.0, 0.0, -60.0], 100.0, 0.0);
    step(&mut sim);
    let seen = remembered(&sim, w);
    assert!(seen.contains(&near));
    assert!(!seen.contains(&far));
    sim.world_mut().insert_resource(VisibilityScale(1.0));
    step(&mut sim);
    assert!(remembered(&sim, w).contains(&far));
}

#[test]
fn the_field_of_view_ends_at_half_its_width() {
    // atan2(10, 10) lands on 45 degrees exactly, the edge of a 90-degree field.
    assert_eq!(bearing_deg(10.0, -10.0), 45.0);
    assert_eq!(bearing_deg(-10.0, -10.0), 315.0);
    let mut sim = sim();
    let w = watcher(&mut sim, [0.0; 3], 0.0);
    let right = thing(&mut sim, "Right", [10.0, 0.0, -10.0], 100.0, 0.0);
    let left = thing(&mut sim, "Left", [-10.0, 0.0, -10.0], 100.0, 0.0);
    let wide = thing(&mut sim, "Wide", [10.000_001, 0.0, -10.0], 100.0, 0.0);
    let behind = thing(&mut sim, "Behind", [0.0, 0.0, 10.0], 100.0, 0.0);
    step(&mut sim);
    let seen = remembered(&sim, w);
    assert!(seen.contains(&right) && seen.contains(&left));
    assert!(!seen.contains(&wide) && !seen.contains(&behind));
}

#[test]
fn bearings_on_zero_negative_zero_and_just_below_360() {
    let mut sim = sim();
    watcher(&mut sim, [0.0; 3], 0.0);
    let zero = thing(&mut sim, "Zero", [0.0, 0.0, -20.0], 100.0, 0.0);
    let neg = thing(&mut sim, "NegZero", [-0.0, 0.0, -30.0], 100.0, 0.0);
    let below = thing(&mut sim, "Below", [-1e-12, 0.0, -40.0], 100.0, 0.0);
    assert_eq!(bearing_deg(-0.0, -30.0).to_bits(), (-0.0f64).to_bits());
    assert!(bearing_deg(-1e-12, -40.0) > 359.999);
    step(&mut sim);
    let text = near_text(&sim);
    for (name, id, rng) in [
        ("Zero", zero, 20),
        ("NegZero", neg, 30),
        ("Below", below, 40),
    ] {
        let line = format!(
            "see {name}#{} thing brg 000 rng {rng}.0m z_m=-{rng}.0 x_m=0.00
",
            id.get()
        )
        .replace("z_m=-0.0", "z_m=0.0");
        assert!(
            text.contains(&line),
            "{line:?} in
{text}"
        );
    }
    let v = near_json(&sim);
    for e in v["entities"].as_array().expect("entities") {
        assert_eq!(e["bearing_deg"], json!(0.0), "{e}");
    }
    assert!(!text.contains("brg 360") && !text.contains("-0"), "{text}");
    // Straight overhead (the watcher remembers three things at most, so in a world of its own):
    // no horizontal offset, bearing 0 and range 0.
    let mut sim = sim_fresh();
    watcher(&mut sim, [0.0; 3], 0.0);
    let above = thing(&mut sim, "Above", [0.0, 5.0, 0.0], 100.0, 0.0);
    step(&mut sim);
    let line = format!(
        "see Above#{} thing brg 000 rng 0.0m z_m=0.0 x_m=0.00
",
        above.get()
    );
    assert!(near_text(&sim).contains(&line));
    assert_eq!(near_json(&sim)["entities"][0]["bearing_deg"], json!(0.0));
}

#[test]
fn a_mast_shows_over_a_wall_and_a_hull_does_not() {
    let mut sim = sim();
    let w = watcher(&mut sim, [0.0, 1.0, 0.0], 0.0);
    wall(&mut sim, [0.0, 0.5, -20.0], [5.0, 1.5, 0.5]);
    let mast = thing(&mut sim, "Mast", [0.0, 0.0, -40.0], 100.0, 5.0);
    let hull = thing(&mut sim, "Hull", [1.0, 0.0, -40.0], 100.0, 1.0);
    let clear = thing(&mut sim, "Clear", [20.0, 0.0, -40.0], 100.0, 0.0);
    step(&mut sim);
    let seen = remembered(&sim, w);
    assert!(seen.contains(&mast), "the top clears the wall");
    assert!(!seen.contains(&hull), "both rays meet the wall");
    assert!(seen.contains(&clear));
}

#[test]
fn what_is_seen_gone_is_forgotten_and_what_is_not_is_kept() {
    let mut sim = sim();
    let w = watcher(&mut sim, [0.0, 1.0, 0.0], 0.0);
    wall(&mut sim, [0.0, 0.5, -20.0], [5.0, 1.5, 0.5]);
    let rover = thing(&mut sim, "Rover", [20.0, 0.0, -40.0], 100.0, 1.0);
    step(&mut sim);
    assert!(remembered(&sim, w).contains(&rover));
    // It slips behind the wall: its old place is in clear sight and empty, so it is dropped.
    place(&mut sim, rover, [0.0, 0.0, -40.0]);
    step(&mut sim);
    assert!(!remembered(&sim, w).contains(&rover));
    // Another is seen, then the watcher turns away: its place is out of sight, so it is kept.
    let stay = thing(&mut sim, "Stay", [20.0, 0.0, -40.0], 100.0, 1.0);
    step(&mut sim);
    assert!(remembered(&sim, w).contains(&stay));
    turn(&mut sim, w, 180.0);
    place(&mut sim, stay, [0.0, 0.0, -40.0]);
    step(&mut sim);
    let m = memory(&sim, w);
    let entry = m.entries.get(&stay).expect("kept");
    assert_eq!(
        entry.pos_m,
        [20.0, 0.0, -40.0],
        "kept where it was last seen"
    );
    let text = near_text(&sim);
    assert!(text.contains("mem Stay#"), "{text}");
    assert!(text.contains("age=0.0s"), "{text}");
}

#[test]
fn memory_expires_after_exactly_memory_s() {
    let mut sim = sim();
    let w = watcher(&mut sim, [0.0; 3], 0.0);
    let t = thing(&mut sim, "Brief", [0.0, 0.0, -10.0], 100.0, 0.0);
    step(&mut sim);
    let seen_tick = memory(&sim, w).entries[&t].seen_tick;
    turn(&mut sim, w, 180.0);
    // memory_s 1 at 60 ticks a second: kept while T - seen_tick <= 60.
    steps(&mut sim, 60);
    assert_eq!(sim.clock().tick.0 - seen_tick.0, 60);
    assert!(remembered(&sim, w).contains(&t), "kept at exactly memory_s");
    let text = near_text(&sim);
    assert!(text.contains("age=1.0s"), "{text}");
    step(&mut sim);
    assert!(!remembered(&sim, w).contains(&t), "dropped one tick later");
}

#[test]
fn capacity_drops_the_oldest_then_the_lowest_id() {
    let mut sim = sim();
    let w = watcher(&mut sim, [0.0; 3], 0.0);
    let a = thing(&mut sim, "A", [-5.0, 0.0, -10.0], 100.0, 0.0);
    let b = thing(&mut sim, "B", [0.0, 0.0, -10.0], 100.0, 0.0);
    let c = thing(&mut sim, "C", [5.0, 0.0, -10.0], 100.0, 0.0);
    step(&mut sim);
    assert_eq!(remembered(&sim, w), vec![a, b, c]);
    turn(&mut sim, w, 180.0);
    let d = thing(&mut sim, "D", [0.0, 0.0, 10.0], 100.0, 0.0);
    step(&mut sim);
    assert_eq!(
        remembered(&sim, w),
        vec![b, c, d],
        "A, oldest with the lowest id, goes"
    );
    let e = thing(&mut sim, "E", [3.0, 0.0, 10.0], 100.0, 0.0);
    step(&mut sim);
    // D and E are seen now; of B and C (both last seen at tick 1) B goes first.
    assert_eq!(remembered(&sim, w), vec![c, d, e]);
}

#[test]
fn a_visible_set_larger_than_the_capacity_is_kept_and_sighted_once() {
    let mut sim = sim();
    let w = watcher(&mut sim, [0.0; 3], 0.0);
    let things: Vec<_> = (0..5)
        .map(|i| {
            let x = f64::from(i) * 2.0 - 4.0;
            thing(&mut sim, &format!("T{i}"), [x, 0.0, -20.0], 100.0, 0.0)
        })
        .collect();
    steps(&mut sim, 5);
    // The capacity (3) bounds what is remembered unseen: all five are seen, so all are kept, and
    // each is sighted once.
    assert_eq!(remembered(&sim, w), things);
    let sighted: Vec<_> = ring(&sim, w)
        .into_iter()
        .filter(|e| e.kind == "sighted")
        .map(|e| e.subject.map(|s| s.id))
        .collect();
    let once: Vec<_> = things.iter().copied().map(Some).collect();
    assert_eq!(sighted, once, "one sighting each");
    // Turned away, all five are remembered unseen: the oldest, then the lowest ids, go.
    turn(&mut sim, w, 180.0);
    step(&mut sim);
    assert_eq!(remembered(&sim, w), things[2..].to_vec());
}

#[test]
fn the_view_inside_a_tick_sees_what_the_last_update_saw() {
    type Log = Arc<Mutex<Vec<(u64, Option<(Visibility, Option<f64>)>)>>>;
    let log: Log = Arc::default();
    let mut sim = sim();
    let w = watcher(&mut sim, [0.0; 3], 0.0);
    let t = thing(&mut sim, "Near", [0.0, 0.0, -10.0], 100.0, 0.0);
    let l = Arc::clone(&log);
    // An executor or an NPC rule reads the view in `Control`, before this tick's update.
    sim.add_exclusive(
        "test.look",
        TickPhase::Control,
        RunCondition::Always,
        move |world, _| {
            let v = PerceptionView::for_seat(world, "watch").expect("a view");
            let p = v.percept(t).map(|p| (p.visibility, p.age_s));
            l.lock().expect("the log").push((v.tick().0, p));
        },
    )
    .expect("a free key");
    steps(&mut sim, 3);
    let at_boundary = PerceptionView::for_seat(sim.world(), "watch")
        .and_then(|v| v.percept(t))
        .map(|p| p.visibility);
    assert_eq!(at_boundary, Some(Visibility::Seen));
    turn(&mut sim, w, 180.0);
    steps(&mut sim, 2);
    let seen = Some((Visibility::Seen, None));
    assert_eq!(
        *log.lock().expect("the log"),
        vec![
            (1, None),
            (2, seen),
            (3, seen),
            // Seen at the end of tick 3; turned away at the boundary after it.
            (4, seen),
            (5, Some((Visibility::Remembered, Some(1.0 / 60.0)))),
        ]
    );
}

#[test]
fn a_global_event_tells_where_only_what_is_seen() {
    let mut sim = sim();
    let w = watcher(&mut sim, [0.0, 1.0, 0.0], 0.0);
    wall(&mut sim, [0.0, 0.5, -20.0], [5.0, 1.5, 0.5]);
    let seen = thing(&mut sim, "Seen", [20.0, 0.0, -40.0], 100.0, 0.0);
    let hidden = thing(&mut sim, "Hidden", [0.0, 0.0, -40.0], 100.0, 0.0);
    let behind = thing(&mut sim, "Behind", [0.0, 0.0, 30.0], 100.0, 0.0);
    step(&mut sim);
    for s in [Some(w), Some(seen), Some(hidden), Some(behind), None] {
        emit(&mut sim, "ev.global", s, None, json!({}));
    }
    // A place of its own (`at_m`) is not the seen subject's: it is not told.
    emit(
        &mut sim,
        "ev.global",
        Some(seen),
        None,
        at([20.0, 0.0, -40.0]),
    );
    step(&mut sim);
    let places: Vec<_> = ring(&sim, w)
        .into_iter()
        .filter(|e| e.kind == "ev.global")
        .map(|e| (e.subject.map(|s| s.id), e.bearing_deg, e.range_m))
        .collect();
    assert_eq!(
        places,
        vec![
            (Some(w), Some(0.0), Some(0.0)),
            (Some(seen), Some(26.6), Some(44.7)),
            (None, None, None),
            (None, None, None),
            (None, None, None),
            (Some(seen), None, None),
        ]
    );
}

#[test]
fn an_event_names_a_remembered_subject_as_it_was_seen() {
    let mut sim = sim();
    let w = watcher(&mut sim, [0.0; 3], 0.0);
    let t = thing(&mut sim, "Old", [0.0, 0.0, -10.0], 100.0, 0.0);
    step(&mut sim);
    turn(&mut sim, w, 180.0);
    step(&mut sim);
    // Renamed while out of sight: what the watcher remembers keeps the old name.
    let e = sim.boundary().entity(t).expect("live");
    sim.world_mut()
        .entity_mut(e)
        .insert(pocket_sim::Name::new("New").expect("a name"));
    emit(&mut sim, "ev.global", Some(t), None, json!({}));
    step(&mut sim);
    let ring = ring(&sim, w);
    let last = ring.last().expect("an event");
    assert_eq!(last.kind, "ev.global");
    let name = last.subject.as_ref().and_then(|s| s.name.as_deref());
    assert_eq!(name, Some("Old"));
    assert_eq!((last.bearing_deg, last.range_m), (None, None));
}

#[test]
fn detail_follows_attention() {
    let mut sim = sim();
    watcher(&mut sim, [0.0; 3], 0.0);
    let close = thing(&mut sim, "Close", [0.0, 0.0, -50.0], 100.0, 0.0);
    let distant = thing(&mut sim, "Distant", [1.0, 0.0, -60.0], 100.0, 0.0);
    step(&mut sim);
    let text = near_text(&sim);
    // Within 50 m: coarse and full facts; beyond: coarse only. The hidden fact never shows.
    let full = format!(
        "see Close#{} thing brg 000 rng 50.0m z_m=-50.0 x_m=0.00\n",
        close.get()
    );
    let coarse = format!(
        "see Distant#{} thing brg 001 rng 60.0m z_m=-60.0\n",
        distant.get()
    );
    assert!(text.contains(&full) && text.contains(&coarse), "{text}");
    assert!(!text.contains("secret_m"));
}

#[test]
fn event_scopes_and_causes() {
    let mut sim = sim();
    let w = watcher(&mut sim, [0.0, 1.0, 0.0], 0.0);
    wall(&mut sim, [0.0, 0.5, -20.0], [5.0, 1.5, 0.5]);
    let other = thing(&mut sim, "Other", [0.0, 0.0, 50.0], 100.0, 0.0);
    step(&mut sim);
    let boundary_tick = sim.clock().tick;
    emit(
        &mut sim,
        "ev.private",
        Some(w),
        None,
        json!({"n": 3, "hush": 9}),
    );
    emit(&mut sim, "ev.private", Some(other), None, json!({"n": 4}));
    emit(&mut sim, "ev.sight", None, None, at([0.0, 0.0, -50.0]));
    emit(&mut sim, "ev.sight", None, None, at([20.0, 0.0, -30.0]));
    emit(&mut sim, "ev.sight", None, None, at([0.0, 0.0, 30.0]));
    emit(&mut sim, "ev.sound", None, None, at([0.0, 1.0, 10.0]));
    emit(&mut sim, "ev.sound", None, None, at([0.0, 1.0, 10.001]));
    let hidden = emit(&mut sim, "ev.hidden", Some(w), None, json!({}));
    emit(&mut sim, "ev.unknown", Some(w), None, json!({}));
    emit(&mut sim, "ev.global", Some(other), Some(hidden), json!({}));
    let first = emit(&mut sim, "ev.global", None, None, json!({}));
    emit(&mut sim, "ev.global", None, Some(first), json!({}));
    step(&mut sim);
    let ring = ring(&sim, w);
    let got: Vec<(&str, Sense)> = ring.iter().map(|e| (e.kind.as_str(), e.sense)).collect();
    assert_eq!(
        got,
        vec![
            ("ev.private", Sense::Private),
            ("ev.sight", Sense::Sight),
            ("ev.sound", Sense::Sound),
            ("ev.global", Sense::Global),
            ("ev.global", Sense::Global),
            ("ev.global", Sense::Global),
        ],
        "{ring:#?}"
    );
    // The seqs are the observer's own, from 1 and contiguous.
    assert_eq!(
        ring.iter().map(|e| e.seq).collect::<Vec<_>>(),
        (1..=6).collect::<Vec<_>>()
    );
    // Only declared, unhidden data; the world event's tick (the boundary's).
    assert_eq!(ring[0].data.len(), 1);
    assert_eq!(ring[0].data[0].0, "n");
    assert!(ring.iter().all(|e| e.tick == boundary_tick));
    // A cause the watcher did not perceive is not shown; one it did is, by its own seq.
    assert_eq!(ring[3].cause, None);
    assert_eq!(ring[5].cause, Some(ring[4].seq));
    // A subject the watcher does not know is left out; its own body is known.
    assert_eq!(ring[3].subject, None);
    assert_eq!(ring[0].subject.as_ref().map(|s| s.id), Some(w));
    // Sight events carry their bearing and range from the body's origin, rounded when stored.
    assert_eq!(ring[1].bearing_deg, Some(33.7));
    assert_eq!(ring[1].range_m, Some(36.1));
}

#[test]
fn an_undeclared_kind_reaches_no_player_of_the_sailing_probe() {
    let (mut sim, ids) = pocket_interface::perception::probe::world(2);
    emit(
        &mut sim,
        "debug.anything",
        Some(ids.sloop),
        None,
        json!({"n": 1}),
    );
    emit(
        &mut sim,
        "secret.signal",
        Some(ids.sloop),
        None,
        json!({"n": 1}),
    );
    emit(&mut sim, "race.start", None, None, json!({"n": 7}));
    step(&mut sim);
    let k = kinds(&sim, ids.sloop);
    assert!(k.contains(&"race.start".to_owned()));
    assert!(
        !k.iter()
            .any(|x| x == "debug.anything" || x == "secret.signal"),
        "{k:?}"
    );
    let req = pocket_interface::perception::EventsRequest {
        since: 0,
        projection: Some(Projection::Text),
        ..Default::default()
    };
    let text = pocket_interface::perception::events(sim.world(), &PLAYER, &req)
        .expect("an answer")
        .body;
    assert!(
        !text.contains("debug.") && !text.contains("secret."),
        "{text}"
    );
}
