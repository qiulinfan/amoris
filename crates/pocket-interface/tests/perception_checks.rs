//! shared/contract/perception.md, Checks, on the sailing probe (`perception::probe`):
//! `contract.perception.read_only`, `contract.perception.deterministic` (a second run, both
//! branches of a fork, a restore), `contract.perception.budget` (observe, nearby, events and
//! describe in text and JSON, and the push delta's byte limit), the omniscient view's marks and
//! refusals, the `omniscient_player` binding, the tensor projection's fixed shapes, and the push
//! delta's cursor.

use pocket_contract::Problem;
use pocket_interface::perception::probe::{self, Ids, PLAYER};
use pocket_interface::perception::{
    Answer, Caller, DescribeRequest, EntityRef, EventsRequest, NearbyRequest, NoAffordances,
    ObserveRequest, Role, SeatParts, Sector, bind_omniscient_player, delta, describe, events,
    nearby, observe,
};
use pocket_interface::projection::Projection;
use pocket_physics::probe::fork;
use pocket_sim::{EventKind, NewEvent, NoHooks, PlainData, Sim};
use serde_json::{Value, json};

const DEVELOPER: Caller<'static> = Caller {
    role: Role::Developer,
    seat: None,
};

fn step(sim: &mut Sim) {
    sim.step(&mut NoHooks).expect("a tick");
}

fn body(r: Result<pocket_interface::perception::Answer, pocket_contract::Problem>) -> String {
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

/// Every query kind for the skipper and the omniscient view, in each projection: what the
/// read-only and determinism checks issue.
fn every_query(sim: &Sim, ids: &Ids) -> Vec<String> {
    let w = sim.world();
    let parts = SeatParts::default();
    let aff = NoAffordances;
    let mut out = probe::answers(sim);
    let ahead = NearbyRequest {
        within_m: Some(500.0),
        sector: Some(Sector {
            from_deg: -30.0,
            to_deg: 30.0,
            relative: true,
        }),
        projection: Some(Projection::Text),
        ..NearbyRequest::default()
    };
    out.push(body(nearby(w, &PLAYER, &ahead, &[], &aff)));
    for name in [
        "Mark1", "Isle", "Crate1", "Ketch", "Islet2", "Crate2", "Sea",
    ] {
        let req = DescribeRequest {
            seat: None,
            entity: EntityRef::Text(name.to_owned()),
            budget_tokens: None,
            projection: Some(Projection::Text),
            omniscient: None,
        };
        out.push(body(describe(w, &PLAYER, &req, &aff)));
    }
    let tensor = ObserveRequest {
        projection: Some(Projection::Tensor),
        ..ObserveRequest::default()
    };
    out.push(body(observe(w, &PLAYER, &tensor, &parts, &aff)));
    for projection in [Projection::Text, Projection::Json] {
        let omni = ObserveRequest {
            projection: Some(projection),
            omniscient: Some(true),
            since: Some(0),
            budget_tokens: Some(4000),
            ..ObserveRequest::default()
        };
        out.push(body(observe(w, &DEVELOPER, &omni, &parts, &aff)));
        let ev = EventsRequest {
            since: 0,
            omniscient: Some(true),
            projection: Some(projection),
            ..EventsRequest::default()
        };
        out.push(body(events(w, &DEVELOPER, &ev)));
        let d = DescribeRequest {
            seat: Some("skipper".to_owned()),
            entity: EntityRef::Id(ids.hidden_crate.get()),
            budget_tokens: None,
            projection: Some(projection),
            omniscient: Some(true),
        };
        out.push(body(describe(w, &DEVELOPER, &d, &aff)));
    }
    out
}

/// The probe's world hash at this boundary (pocket-physics' ledger with perception's types).
fn hash(sim: &Sim) -> u64 {
    probe::ledger()
        .snapshot(sim.world())
        .expect("a snapshot")
        .hash()
}

#[test]
fn queries_are_read_only() {
    const TICKS: u64 = 3600;
    let run = |query: bool| -> Vec<u64> {
        let mut sim = probe::fresh();
        let ids = probe::scene(&mut sim);
        let mut hashes = Vec::new();
        for t in 1..=TICKS {
            if t % 97 == 0 {
                let e = NewEvent::new(EventKind::new("race.start").expect("a kind"))
                    .data(PlainData::from_json(&json!({"n": t})));
                sim.boundary().emit(e);
            }
            step(&mut sim);
            if query {
                let n = every_query(&sim, &ids).len();
                assert!(n > 20);
            }
            hashes.push(hash(&sim));
        }
        hashes
    };
    let quiet = run(false);
    let asked = run(true);
    let first = quiet.iter().zip(&asked).position(|(a, b)| a != b);
    assert_eq!(
        first,
        None,
        "the world hash differs from tick {:?}",
        first.map(|i| i + 1)
    );
}

#[test]
fn answers_are_deterministic_across_runs_forks_and_restores() {
    let (sim, ids) = probe::world(20);
    let want = every_query(&sim, &ids);
    let (again, _) = probe::world(20);
    assert_eq!(every_query(&again, &ids), want, "a second run");
    let ledger = probe::ledger();
    let mut branch = fork(&ledger, &sim, probe::fresh).expect("a fork");
    assert_eq!(
        every_query(&branch, &ids),
        want,
        "the fork, before either acts"
    );
    assert_eq!(
        every_query(&sim, &ids),
        want,
        "the original, after the fork"
    );
    let snap = ledger.snapshot(sim.world()).expect("a snapshot");
    let mut restored = probe::fresh();
    ledger.restore(&mut restored, &snap).expect("a restore");
    assert_eq!(
        every_query(&restored, &ids),
        want,
        "a restore of the tick's snapshot"
    );
    // And they go on alike.
    let mut sim = sim;
    for _ in 0..40 {
        step(&mut sim);
        step(&mut branch);
        step(&mut restored);
    }
    let later = every_query(&sim, &ids);
    assert_eq!(every_query(&branch, &ids), later);
    assert_eq!(every_query(&restored, &ids), later);
}

/// The percept and event lines of a text answer.
fn lines<'a>(text: &'a str, prefixes: &[&str]) -> Vec<&'a str> {
    text.lines()
        .filter(|l| prefixes.iter().any(|p| l.starts_with(p)))
        .collect()
}

/// The probe after 120 ticks with a `race.start` every 4 ticks, every other one about the sloop
/// (so `describe` of the sloop has more events than it shows).
fn busy() -> Sim {
    let mut sim = probe::fresh();
    let ids = probe::scene(&mut sim);
    for t in 0..120u64 {
        if t % 4 == 0 {
            let mut e = NewEvent::new(EventKind::new("race.start").expect("a kind"))
                .data(PlainData::from_json(&json!({"n": t})));
            if t % 8 == 0 {
                e = e.subject(ids.sloop);
            }
            sim.boundary().emit(e);
        }
        step(&mut sim);
    }
    sim
}

type Ask<'a> = dyn Fn(u32) -> Result<Answer, Problem> + 'a;

/// The smallest budget that answers; the one below it is refused and names it as `min_tokens`
/// (perception.md, Token budgets: the smallest budget that would fit).
fn least_budget(what: &str, ask: &Ask<'_>) -> u32 {
    let min = (1..=2000)
        .find(|b| ask(*b).is_ok())
        .unwrap_or_else(|| panic!("{what}: no budget answers"));
    if min > 1 {
        let refused = ask(min - 1).expect_err("below the smallest");
        assert_eq!(refused.code, "perception.budget_too_small", "{what}");
        assert_eq!(refused.detail["min_tokens"], json!(min), "{what}");
    }
    min
}

/// Budgets from the smallest that answers to 2000 in steps of 10: each answer within its budget,
/// its items (`pick`) a prefix of the whole answer's (with `newest`, a suffix: `describe` keeps
/// the newest events), never fewer at a larger budget; a text answer is given again, byte for
/// byte, at its own size in tokens. The number of items in the whole answer.
fn sweep(
    what: &str,
    projection: Projection,
    ask: &Ask<'_>,
    pick: &dyn Fn(&str) -> Vec<String>,
    newest: bool,
) -> usize {
    let min = least_budget(what, ask);
    let all = pick(&ask(16000).expect("everything").body);
    let mut last = 0;
    let mut b = min;
    while b <= 2000 {
        let a = ask(b).unwrap_or_else(|p| panic!("{what} at {b}: {}", p.message));
        let bytes = usize::try_from(b).expect("small") * 4;
        assert!(a.body.len() <= bytes, "{what} at {b}: {}", a.body);
        let got = pick(&a.body);
        let want = if newest {
            &all[all.len() - got.len()..]
        } else {
            &all[..got.len()]
        };
        assert_eq!(got[..], want[..], "{what} at {b}");
        assert!(got.len() >= last, "{what} at {b} chose fewer");
        last = got.len();
        if projection == Projection::Text {
            let again = ask(a.tokens).map(|x| x.body);
            assert_eq!(
                again.as_deref().ok(),
                Some(a.body.as_str()),
                "{what}: the answer at {b} is not given at its own {} tokens",
                a.tokens
            );
        }
        b += 10;
    }
    all.len()
}

#[test]
fn nearby_events_and_describe_honour_their_budgets() {
    let sim = busy();
    let w = sim.world();
    let aff = NoAffordances;
    for projection in [Projection::Text, Projection::Json] {
        let near = |b: u32| {
            let req = NearbyRequest {
                budget_tokens: Some(b),
                projection: Some(projection),
                ..NearbyRequest::default()
            };
            nearby(w, &PLAYER, &req, &[], &aff)
        };
        let percepts = |s: &str| percepts_in(s, projection);
        let n = sweep("nearby", projection, &near, &percepts, false);
        assert!(n > 4, "{n} percepts");
        let ev = |b: u32| {
            let req = EventsRequest {
                since: 0,
                budget_tokens: Some(b),
                projection: Some(projection),
                ..EventsRequest::default()
            };
            events(w, &PLAYER, &req)
        };
        let seqs = |s: &str| events_in(s, projection);
        let n = sweep("events", projection, &ev, &seqs, false);
        assert!(n > 20, "{n} events");
        let about = |b: u32| {
            let req = DescribeRequest {
                seat: None,
                entity: EntityRef::Text("Sloop".to_owned()),
                budget_tokens: Some(b),
                projection: Some(projection),
                omniscient: None,
            };
            describe(w, &PLAYER, &req, &aff)
        };
        let n = sweep("describe", projection, &about, &seqs, true);
        assert_eq!(n, 5, "describe shows the last five");
    }
}

#[test]
fn the_push_delta_honours_its_limit() {
    let sim = busy();
    let w = sim.world();
    for projection in [Projection::Text, Projection::Json] {
        let seqs = |d: &pocket_interface::perception::Delta| -> Vec<u64> {
            d.events
                .iter()
                .map(|e| {
                    let v: Value = serde_json::from_str(e).expect("JSON");
                    v["seq"].as_u64().expect("a seq")
                })
                .collect()
        };
        let all = seqs(&delta(w, "skipper", 0, projection, usize::MAX).expect("a delta"));
        assert!(all.len() > 20);
        let mut last = 0;
        let first = if projection == Projection::Json { 2 } else { 0 };
        for limit in (first..120).chain((120..=8000).step_by(40)) {
            let d = delta(w, "skipper", 0, projection, limit).expect("a delta");
            let size = match projection {
                Projection::Json => format!("[{}]", d.events.join(",")).len(),
                _ => d.text.len(),
            };
            assert!(size <= limit, "{limit}: {size} bytes: {}", d.text);
            let got = seqs(&d);
            assert_eq!(got[..], all[..got.len()], "{limit}");
            assert!(got.len() >= last, "{limit} chose fewer");
            assert_eq!(d.cursor, got.last().copied().unwrap_or(0), "{limit}");
            if projection == Projection::Text {
                let lines = d.text.lines().filter(|l| l.starts_with("event ")).count();
                assert_eq!(lines, got.len(), "{limit}: {}", d.text);
            }
            last = got.len();
        }
    }
}

#[test]
fn one_short_item_answers_below_the_omitted_lines_size() {
    // A single short event: its line is shorter than the omitted line that would replace it, so
    // the smallest answer is the whole one, and the refusal below it names that answer's size.
    let (mut sim, _) = probe::world(5);
    let e = NewEvent::new(EventKind::new("race.start").expect("a kind"))
        .data(PlainData::from_json(&json!({"n": 1})));
    sim.boundary().emit(e);
    step(&mut sim);
    let w = sim.world();
    let ask = |b: u32| {
        let req = EventsRequest {
            since: 0,
            kinds: Some(vec!["race.".to_owned()]),
            budget_tokens: Some(b),
            projection: Some(Projection::Text),
            ..EventsRequest::default()
        };
        events(w, &PLAYER, &req)
    };
    let min = least_budget("one event", &ask);
    let a = ask(min).expect("an answer");
    assert!(
        a.body.contains(" race.start ") && !a.body.contains("omitted"),
        "{}",
        a.body
    );
    assert_eq!(
        min, a.tokens,
        "the smallest budget is the answer's own size"
    );
    let empty = format!(
        "{}omitted events=1 (events {{\"since\":0}})\n",
        a.body
            .lines()
            .next()
            .map(|h| format!("{h}\n"))
            .unwrap_or_default()
    );
    assert!(empty.len() > a.body.len(), "{empty:?}");
}

#[test]
fn budgets_are_honoured_and_larger_ones_answer_supersets() {
    let sim = busy();
    let w = sim.world();
    let aff = NoAffordances;
    let parts = SeatParts::default();
    for projection in [Projection::Text, Projection::Json] {
        let ask = |b: u32| {
            let req = ObserveRequest {
                budget_tokens: Some(b),
                projection: Some(projection),
                since: Some(0),
                ..ObserveRequest::default()
            };
            observe(w, &PLAYER, &req, &parts, &aff)
        };
        let min = least_budget("observe", &ask);
        let full = ask(16000).expect("everything");
        let (all_p, all_e) = chosen(&full.body, projection);
        assert!(all_e.len() > 20 && all_p.len() > 4, "{}", full.body);
        let mut last: Option<(Vec<String>, Vec<String>)> = None;
        let mut b = min;
        while b <= 2000 {
            let a = ask(b).expect("fits");
            assert!(
                a.body.len() <= usize::try_from(b).expect("small") * 4,
                "{b}: {}",
                a.body
            );
            let (p, e) = chosen(&a.body, projection);
            // A prefix of each list, and of the merged sequence p1, e1, p2, e2, ...
            assert_eq!(p[..], all_p[..p.len()]);
            assert_eq!(e[..], all_e[..e.len()]);
            let k = p.len() + e.len();
            let want = split(all_p.len(), all_e.len(), k);
            assert_eq!(
                (p.len(), e.len()),
                want,
                "budget {b}: not a prefix of the merged sequence"
            );
            if let Some((lp, le)) = &last {
                assert!(
                    lp.len() <= p.len() && le.len() <= e.len(),
                    "budget {b} chose fewer"
                );
            }
            if projection == Projection::Text {
                assert_eq!(
                    ask(a.tokens).map(|x| x.body).ok().as_deref(),
                    Some(a.body.as_str()),
                    "the answer at {b} at its own size"
                );
            }
            last = Some((p, e));
            b += 10;
        }
    }
}

/// How many percepts and events the first `k` of the merged sequence p1, e1, p2, e2, ... hold,
/// with `n` percepts and `m` events in all.
fn split(n: usize, m: usize, k: usize) -> (usize, usize) {
    let (mut i, mut j) = (0, 0);
    while i + j < k {
        if i < n && (i <= j || j >= m) {
            i += 1;
        } else {
            j += 1;
        }
    }
    (i, j)
}

/// The percepts and events an observation chose, by their identifying text.
fn chosen(body: &str, projection: Projection) -> (Vec<String>, Vec<String>) {
    (percepts_in(body, projection), events_in(body, projection))
}

/// A list member's values in a JSON answer, or the second word of the lines with these prefixes
/// in a text one.
fn items_in(
    body: &str,
    projection: Projection,
    list: &str,
    field: &str,
    prefixes: &[&str],
) -> Vec<String> {
    match projection {
        Projection::Json => {
            let v: Value = serde_json::from_str(body).expect("JSON");
            v[list]
                .as_array()
                .expect("a list")
                .iter()
                .map(|x| x[field].to_string())
                .collect()
        }
        _ => lines(body, prefixes)
            .iter()
            .map(|l| l.split(' ').nth(1).unwrap_or("").to_owned())
            .collect(),
    }
}

/// The percepts an answer chose (its `entities`), by id or reference.
fn percepts_in(body: &str, projection: Projection) -> Vec<String> {
    items_in(
        body,
        projection,
        "entities",
        "id",
        &["see ", "mem ", "chart "],
    )
}

/// The events an answer chose, by seq.
fn events_in(body: &str, projection: Projection) -> Vec<String> {
    items_in(body, projection, "events", "seq", &["event "])
}

#[test]
fn the_omniscient_view_is_marked_and_never_a_players() {
    let (mut sim, ids) = probe::world(30);
    let aff = NoAffordances;
    let parts = SeatParts::default();
    let w = sim.world();
    let omni = ObserveRequest {
        omniscient: Some(true),
        projection: Some(Projection::Text),
        ..ObserveRequest::default()
    };
    let e = observe(w, &PLAYER, &omni, &parts, &aff).expect_err("a player is refused");
    assert_eq!(e.code, "perception.omniscient_forbidden");
    let other = ObserveRequest {
        seat: Some("lookout".to_owned()),
        ..ObserveRequest::default()
    };
    assert_eq!(
        observe(w, &PLAYER, &other, &parts, &aff)
            .expect_err("not yours")
            .code,
        "seat.not_yours"
    );
    let none = ObserveRequest::default();
    assert_eq!(
        observe(w, &DEVELOPER, &none, &parts, &aff)
            .expect_err("a seat or omniscient")
            .code,
        "request.missing_field"
    );
    // A developer's omniscient view: marked, every entity (perceivable or not), positions.
    let text = observe(w, &DEVELOPER, &omni, &parts, &aff)
        .expect("an answer")
        .body;
    let head = text.lines().next().expect("a header");
    assert!(
        head.ends_with(" OMNISCIENT") && head.contains("seat=- observer=-"),
        "{head}"
    );
    assert!(
        text.contains("Crate2#") && text.contains("Sea#") && text.contains(" entity "),
        "{text}"
    );
    let json_req = ObserveRequest {
        projection: Some(Projection::Json),
        budget_tokens: Some(16000),
        ..omni.clone()
    };
    let v: Value = serde_json::from_str(
        &observe(w, &DEVELOPER, &json_req, &parts, &aff)
            .expect("json")
            .body,
    )
    .expect("JSON");
    assert_eq!(v["omniscient"], json!(true));
    for p in v["entities"].as_array().expect("entities") {
        assert!(p["pos_m"].is_object(), "every percept has pos_m: {p}");
    }
    // The player's own answers say omniscient false and never show what is hidden.
    let mine = ObserveRequest {
        projection: Some(Projection::Json),
        since: Some(0),
        ..ObserveRequest::default()
    };
    let a = observe(w, &PLAYER, &mine, &parts, &aff)
        .expect("an answer")
        .body;
    assert!(
        a.contains("\"omniscient\":false") && !a.contains("Crate2") && !a.contains("restitution"),
        "{a}"
    );
    // A developer naming the seat gets the player's answer.
    let dev = ObserveRequest {
        seat: Some("skipper".to_owned()),
        ..mine.clone()
    };
    assert_eq!(
        observe(w, &DEVELOPER, &dev, &parts, &aff)
            .expect("an answer")
            .body,
        a
    );

    // Bound to omniscient_player: every perceivable entity in full with hidden facts and
    // positions, marked; non-perceivable entities left out.
    assert!(bind_omniscient_player(sim.world_mut(), "skipper", true));
    step(&mut sim);
    let w = sim.world();
    let text_req = ObserveRequest {
        projection: Some(Projection::Text),
        budget_tokens: Some(4000),
        ..ObserveRequest::default()
    };
    let t = observe(w, &PLAYER, &text_req, &parts, &aff)
        .expect("an answer")
        .body;
    assert!(
        t.lines().next().expect("a header").ends_with(" OMNISCIENT"),
        "{t}"
    );
    assert!(
        t.contains("see Crate2#") && t.contains("see Crate3#") && t.contains("restitution="),
        "{t}"
    );
    assert!(
        !t.contains("Sea#") && !t.contains("chart ") && !t.contains("mem "),
        "{t}"
    );
    // Unbound: what it knew while omniscient is gone.
    assert!(bind_omniscient_player(sim.world_mut(), "skipper", false));
    step(&mut sim);
    let t = observe(sim.world(), &PLAYER, &text_req, &parts, &aff)
        .expect("an answer")
        .body;
    assert!(
        !t.contains("OMNISCIENT") && !t.contains("Crate2") && !t.contains("restitution"),
        "{t}"
    );
    let _ = ids;
}

#[test]
fn tensors_have_the_profiles_fixed_shapes() {
    let aff = NoAffordances;
    let parts = SeatParts::default();
    let req = ObserveRequest {
        projection: Some(Projection::Tensor),
        ..ObserveRequest::default()
    };
    let mut shapes = Vec::new();
    for ticks in [1, 200] {
        let (sim, _) = probe::world(ticks);
        let a = observe(sim.world(), &PLAYER, &req, &parts, &aff).expect("tensors");
        let v: Value = serde_json::from_str(&a.body).expect("JSON");
        assert_eq!(v["omniscient"], json!(false));
        let t = &v["tensors"];
        for (name, shape) in [
            ("self", json!([12])),
            ("crates", json!([4, 5])),
            ("marks", json!([4, 6])),
            ("rays", json!([8, 4])),
        ] {
            assert_eq!(t[name]["shape"], shape, "{name}");
            let n: u64 = shape
                .as_array()
                .expect("dims")
                .iter()
                .map(|d| d.as_u64().expect("a dim"))
                .product();
            assert_eq!(
                t[name]["data"].as_array().expect("data").len() as u64,
                n,
                "{name}"
            );
        }
        // A seen crate is a present row; rows past the last percept are zero.
        assert_eq!(t["crates"]["data"][0], json!(1.0));
        assert_eq!(t["crates"]["data"][15], json!(0.0));
        shapes.push(a.body.len());
    }
    let budget = ObserveRequest {
        budget_tokens: Some(100),
        ..req
    };
    let (sim, _) = probe::world(1);
    let e = observe(sim.world(), &PLAYER, &budget, &parts, &aff).expect_err("not applicable");
    assert_eq!(e.code, "request.not_applicable");
}

#[test]
fn the_push_delta_continues_where_it_left_off() {
    let mut sim = probe::fresh();
    probe::scene(&mut sim);
    for t in 0..40u64 {
        if t % 2 == 0 {
            let e = NewEvent::new(EventKind::new("race.start").expect("a kind"))
                .data(PlainData::from_json(&json!({"n": t})));
            sim.boundary().emit(e);
        }
        step(&mut sim);
    }
    let w = sim.world();
    let first = delta(w, "skipper", 0, Projection::Text, 200).expect("a delta");
    assert!(
        first
            .text
            .ends_with(&format!("(events {{\"since\":{}}})\n", first.cursor)),
        "{}",
        first.text
    );
    assert!(first.text.len() <= 200, "{}", first.text);
    let mut seen: Vec<u64> = Vec::new();
    let mut cursor = 0;
    loop {
        let d = delta(w, "skipper", cursor, Projection::Json, 400).expect("a delta");
        for e in &d.events {
            let v: Value = serde_json::from_str(e).expect("JSON");
            seen.push(v["seq"].as_u64().expect("a seq"));
        }
        if d.cursor == cursor {
            break;
        }
        cursor = d.cursor;
    }
    let all: Vec<u64> = (1..=seen.len() as u64).collect();
    assert_eq!(seen, all, "every event once, in order");
    let ev = EventsRequest {
        since: cursor + 1,
        ..EventsRequest::default()
    };
    assert_eq!(
        events(w, &PLAYER, &ev).expect_err("ahead").code,
        "perception.cursor_ahead"
    );
}

#[test]
fn the_persisted_types_trace_as_pocket_persist_traces_them() {
    use pocket_interface::perception::{
        Observer, ObserverEvents, ObserverMemory, Occluder, Perceivable, VisibilityScale,
    };
    use pocket_sim::Persisted;
    use pocket_sim::persisted::{record_samples, tracer_config};
    use serde_reflection::{Samples, Tracer};
    type Trace = fn(&mut Tracer, &Samples) -> serde_reflection::Result<()>;
    let traces: [(&str, Trace); 6] = [
        ("Observer", <Observer as Persisted>::trace),
        ("Perceivable", <Perceivable as Persisted>::trace),
        ("Occluder", <Occluder as Persisted>::trace),
        ("VisibilityScale", <VisibilityScale as Persisted>::trace),
        ("ObserverMemory", <ObserverMemory as Persisted>::trace),
        ("ObserverEvents", <ObserverEvents as Persisted>::trace),
    ];
    for (name, trace) in traces {
        let mut t = Tracer::new(tracer_config());
        let mut s = Samples::new();
        record_samples(&mut t, &mut s).expect("the simulation's samples");
        trace(&mut t, &s).unwrap_or_else(|e| panic!("{name}: {e}"));
        t.registry().unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}

#[test]
fn the_view_answers_as_the_queries_do_before_projection() {
    let (sim, ids) = probe::world(60);
    let w = sim.world();
    let view =
        pocket_interface::perception::PerceptionView::for_seat(w, "skipper").expect("a view");
    for filter in [
        json!({}),
        json!({"kinds": ["island", "mark"]}),
        json!({"within_m": 300, "visibility": ["seen"]}),
        json!({"sector": {"from_deg": -90, "to_deg": 90, "relative": true}, "limit": 2}),
    ] {
        let mut req: NearbyRequest = serde_json::from_value(filter.clone()).expect("a request");
        let unprojected: Vec<u64> = view.nearby(&req).iter().map(|p| p.id.get()).collect();
        req.projection = Some(Projection::Json);
        req.budget_tokens = Some(16000);
        let a = nearby(w, &PLAYER, &req, &[], &NoAffordances).expect("an answer");
        let v: Value = serde_json::from_str(&a.body).expect("JSON");
        let projected: Vec<u64> = v["entities"]
            .as_array()
            .expect("entities")
            .iter()
            .map(|p| p["id"].as_u64().expect("an id"))
            .collect();
        assert_eq!(unprojected, projected, "{filter}");
    }
    assert!(view.percept(ids.hidden_crate).is_none());
    assert_eq!(
        view.instrument("wind_from_deg"),
        Some(pocket_interface::perception::FactValue::Number(270.0))
    );
    assert_eq!(
        view.events_since(0).count(),
        3,
        "two sightings at tick 1, Mark1 at tick 14"
    );
}
