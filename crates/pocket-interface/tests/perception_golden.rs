//! Projection golden cases (shared/contract/projection.md, Checks; sailing.md, The skipper's
//! perception): answers built by hand from the sailing showcase's declarations and written to the
//! exact bytes the contract shows. The shared conformance file (`projection.jsonl`) is not in
//! `shared/` yet, so the cases live here (docs/spec/perception-slice2.md, choice 6).

use pocket_contract::Problem;
use pocket_interface::perception::probe::SAILING;
use pocket_interface::perception::state::{Named, Sense, Visibility};
use pocket_interface::perception::{
    AffordanceStatus, Detail, EventView, FactValue, Observation, Percept, PerceptionDefs, Reading,
    load, write_describe, write_events, write_nearby, write_observation,
};
use pocket_interface::projection::round::Format;
use pocket_interface::projection::{Header, Projection, Rendered};
use pocket_sim::{EntityId, Tick};
use serde_json::json;

fn defs() -> PerceptionDefs {
    load(SAILING).expect("the sailing declarations load")
}

fn id(n: u64) -> EntityId {
    EntityId::new(n).expect("an id")
}

fn num(x: f64) -> FactValue {
    FactValue::Number(x)
}

fn text(s: &str) -> FactValue {
    FactValue::Text(s.to_owned())
}

/// An instrument reading at its declared format.
fn instrument(d: &PerceptionDefs, name: &str, value: FactValue) -> Reading {
    let def = d.instrument(name).expect("declared");
    Reading {
        name: name.to_owned(),
        value,
        format: Format::of(&def.unit, def.precision),
    }
}

/// A fact reading of `kind` at its declared format.
fn fact(d: &PerceptionDefs, kind: &str, name: &str, value: FactValue) -> Reading {
    let def = d
        .kind(kind)
        .and_then(|k| k.facts.iter().find(|f| f.name == name))
        .expect("declared");
    Reading {
        name: name.to_owned(),
        value,
        format: Format::of(&def.unit, def.precision),
    }
}

#[allow(clippy::too_many_arguments)]
fn percept(
    n: u64,
    name: &str,
    kind: &str,
    visibility: Visibility,
    detail: Detail,
    bearing: f64,
    range: f64,
    facts: Vec<Reading>,
) -> Percept {
    Percept {
        id: id(n),
        name: Some(name.to_owned()),
        kind: kind.to_owned(),
        visibility,
        detail,
        bearing_deg: bearing,
        range_m: range,
        pos_m: None,
        age_s: None,
        facts,
        can: Vec::new(),
        priority: 0,
    }
}

fn header(tick: u64, t_s: f64, omniscient: bool) -> Header {
    Header {
        tick: Tick(tick),
        t_s,
        seat: Some("skipper".to_owned()),
        observer: Some(Named {
            id: id(12),
            name: Some("Sloop".to_owned()),
        }),
        omniscient,
    }
}

/// sailing.md's example: the boat running east toward the first mark, an island to the north-east,
/// a crate passed astern, an islet on the chart, and the crate it took aboard five ticks ago.
fn example() -> Observation {
    let d = defs();
    let i = |n: &str, v: FactValue| instrument(&d, n, v);
    let instruments = vec![
        i("heading_deg", num(87.0)),
        i("course_deg", num(89.0)),
        i("speed_mps", num(3.4)),
        i("wind_from_deg", num(270.0)),
        i("wind_mps", num(6.1)),
        i("twa_deg", num(-177.0)),
        i("awa_deg", num(-176.0)),
        i("aws_mps", num(2.7)),
        i("point_of_sail", text("running")),
        i("tack", text("port")),
        i("hoist", num(1.0)),
        i("sheet", num(0.95)),
        i("trim", text("good")),
        i("drive", num(0.91)),
        i("rudder", num(0.02)),
        i("heel_deg", num(1.0)),
        i("pos_m", FactValue::Position([12.4, 0.0, -3.1])),
        i("afloat", FactValue::Bool(true)),
        i("next_mark", text("Mark1")),
        i("taken", num(3.0)),
        i("left", num(13.0)),
    ];
    let mut mark = percept(
        40,
        "Mark1",
        "mark",
        Visibility::Seen,
        Detail::Full,
        92.0,
        412.0,
        vec![
            fact(&d, "mark", "color", text("yellow")),
            fact(&d, "mark", "round_to", text("port")),
            fact(&d, "mark", "order", num(1.0)),
            fact(&d, "mark", "next", FactValue::Bool(true)),
        ],
    );
    mark.pos_m = Some([424.2, 0.0, 11.3]);
    let isle = percept(
        7,
        "Isle",
        "island",
        Visibility::Seen,
        Detail::Full,
        41.0,
        380.0,
        vec![
            fact(&d, "island", "radius_m", num(150.0)),
            fact(&d, "island", "shore_m", num(231.0)),
            fact(&d, "island", "shore_brg_deg", num(44.0)),
        ],
    );
    let mut crate4 = percept(
        21,
        "Crate4",
        "crate",
        Visibility::Remembered,
        Detail::Full,
        268.0,
        135.0,
        vec![],
    );
    crate4.age_s = Some(4.5);
    let islet = percept(
        9,
        "Islet2",
        "island",
        Visibility::Charted,
        Detail::Chart,
        175.0,
        1210.0,
        vec![fact(&d, "island", "radius_m", num(60.0))],
    );
    let taken = d.event("crate.taken").expect("declared");
    let data = taken
        .data
        .iter()
        .zip([3.0, 13.0])
        .map(|(f, v)| Reading {
            name: f.name.clone(),
            value: num(v),
            format: Format::of(&f.unit, f.precision),
        })
        .collect();
    let event = EventView {
        seq: 41,
        tick: Tick(1795),
        kind: "crate.taken".to_owned(),
        subject: Some(Named {
            id: id(12),
            name: Some("Sloop".to_owned()),
        }),
        sense: Sense::Sight,
        bearing_deg: Some(0.0),
        range_m: Some(0.0),
        data,
        cause: None,
    };
    Observation {
        header: header(1800, 30.0, false),
        instruments,
        intents: vec![Rendered {
            json: r#"{"id":3}"#.to_owned(),
            text: "intent #3 sail_to active target=Mark1#40 distance_m=412 bearing_deg=092 \
                   vmg_mps=3.4 eta_s=121 leg=direct tacks=0"
                .to_owned(),
        }],
        decision: None,
        percepts: vec![mark, isle, crate4, islet],
        events: vec![event],
        cursor: 40,
        lost: 0,
    }
}

const EXAMPLE: &str = "tick 1800 t=30.0s seat=skipper observer=Sloop#12
self heading_deg=087 course_deg=089 speed_mps=3.4 wind_from_deg=270 wind_mps=6.1 twa_deg=-177 awa_deg=-176 aws_mps=2.7 point_of_sail=running tack=port hoist=1.00 sheet=0.95 trim=good drive=0.91 rudder=0.02 heel_deg=1 pos_m=(12.4,0.0,-3.1) afloat=true next_mark=Mark1 taken=3 left=13
intent #3 sail_to active target=Mark1#40 distance_m=412 bearing_deg=092 vmg_mps=3.4 eta_s=121 leg=direct tacks=0
see Mark1#40 mark brg 092 rng 412m color=yellow round_to=port order=1 next=true
see Isle#7 island brg 041 rng 380m radius_m=150 shore_m=231 shore_brg_deg=044
mem Crate4#21 crate brg 268 rng 135m age=4.5s
chart Islet2#9 island brg 175 rng 1210m radius_m=60
event 41 tick=1795 crate.taken Sloop#12 brg 000 rng 0.0m taken=3 left=13
";

#[test]
fn the_skippers_example_observation() {
    let a = write_observation(&example(), Projection::Text, 400).expect("an answer");
    assert_eq!(a.body, EXAMPLE);
    assert_eq!(a.body.len(), 774, "sailing.md: 774 bytes");
    assert_eq!(a.tokens, 194, "sailing.md: 194 tokens by the estimate");
    assert_eq!(a.cursor, Some(41));
}

#[test]
fn the_example_at_185_tokens_keeps_the_first_three_candidates() {
    let a = write_observation(&example(), Projection::Text, 185).expect("an answer");
    let mut want: String = EXAMPLE.lines().take(5).map(|l| format!("{l}\n")).collect();
    want.push_str(
        &EXAMPLE
            .lines()
            .nth(7)
            .map(|l| format!("{l}\n"))
            .unwrap_or_default(),
    );
    want.push_str("omitted crate=1 island=1 (nearby {\"kinds\":[\"crate\",\"island\"]})\n");
    assert_eq!(a.body, want);
    assert_eq!(a.body.len(), 739, "sailing.md: 739 bytes");
    // One token less and Isle gives way too.
    let b = write_observation(&example(), Projection::Text, 184).expect("an answer");
    assert!(b.body.contains("omitted crate=1 island=2 ("), "{}", b.body);
    assert!(b.body.contains("event 41 "));
}

#[test]
fn the_examples_json() {
    let a = write_observation(&example(), Projection::Json, 1000).expect("an answer");
    let mark = r#"{"id":40,"name":"Mark1","kind":"mark","visibility":"seen","detail":"full","bearing_deg":92,"range_m":412,"pos_m":{"x":424.2,"y":0.0,"z":11.3},"facts":[{"name":"color","value":"yellow"},{"name":"round_to","value":"port"},{"name":"order","value":1},{"name":"next","value":true}],"can":[]}"#;
    assert!(a.body.contains(mark), "{}", a.body);
    let v: serde_json::Value = serde_json::from_str(&a.body).expect("JSON");
    let keys: Vec<&str> = v
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    // serde_json's map is ordered by key here; the bytes keep declaration order (checked below).
    assert_eq!(keys.len(), 12, "{keys:?}");
    let order = [
        "\"tick\"",
        "\"t_s\"",
        "\"seat\"",
        "\"observer\"",
        "\"omniscient\"",
        "\"instruments\"",
        "\"intents\"",
        "\"entities\"",
        "\"events\"",
        "\"cursor\"",
        "\"omitted\"",
        "\"tokens\"",
    ];
    let at: Vec<usize> = order
        .iter()
        .map(|k| a.body.find(k).expect("a member"))
        .collect();
    assert!(at.windows(2).all(|w| w[0] < w[1]), "{}", a.body);
    assert_eq!(v["omniscient"], json!(false));
    assert_eq!(
        v["instruments"][0],
        json!({"name": "heading_deg", "value": 87})
    );
    assert_eq!(v["instruments"][10], json!({"name": "hoist", "value": 1.0}));
    assert_eq!(v["events"][0]["bearing_deg"], json!(0.0));
    assert_eq!(v["events"][0]["range_m"], json!(0.0));
    assert_eq!(v["entities"][2]["age_s"], json!(4.5));
    assert_eq!(
        v["omitted"],
        json!({"entities": {}, "events": 0, "lost": 0})
    );
    // tokens is the estimate of the bytes without the tokens member.
    let without = a.body.len() - format!(",\"tokens\":{}", a.tokens).len();
    assert_eq!(a.tokens, u32::try_from(without.div_ceil(4)).expect("small"));
}

#[test]
fn rounding_quoting_and_bearings() {
    let d = defs();
    let mut p = percept(
        5,
        "Odd \"one\"",
        "boat",
        Visibility::Seen,
        Detail::Full,
        359.6,
        99.95,
        vec![
            fact(&d, "boat", "heading_deg", num(359.5)),
            fact(&d, "boat", "sail", text("set")),
            fact(&d, "boat", "speed_mps", num(-0.04)),
        ],
    );
    p.pos_m = Some([0.25, -0.0, 2.5]);
    let near = percept(
        6,
        "Near",
        "boat",
        Visibility::Seen,
        Detail::Coarse,
        0.0,
        0.0,
        vec![
            fact(&d, "boat", "heading_deg", num(0.5)),
            fact(&d, "boat", "sail", text("a b")),
        ],
    );
    let h = header(9, 0.15, false);
    let t = write_nearby(&h, &[p.clone(), near.clone()], 100, Projection::Text, 400).expect("text");
    // 359.6 rounds to 360, written 000; 99.95 is below 100 m: one decimal (ties to even on the
    // binary value, which is above the tie); -0.04 at one decimal is -0.0, written 0.0; 359.5 at
    // 0 is 360 -> 000; 0.5 at 0 ties to even, 000; a text value outside [A-Za-z0-9_.:+-] is a
    // JSON string; a name is written as it is.
    let want = concat!(
        "tick 9
",
        "see Odd \"one\"#5 boat brg 000 rng 100.0m heading_deg=000 sail=set speed_mps=0.0
",
        "see Near#6 boat brg 000 rng 0.0m heading_deg=000 sail=\"a b\"
",
    );
    assert_eq!(t.body, want);
    let j = write_nearby(&h, &[p, near], 100, Projection::Json, 400).expect("json");
    let v: serde_json::Value = serde_json::from_str(&j.body).expect("JSON");
    // In JSON a bearing below 100 m has one decimal: 359.6 stays 359.6.
    assert_eq!(v["entities"][0]["bearing_deg"], json!(359.6), "{}", j.body);
    assert_eq!(v["entities"][0]["range_m"], json!(100.0));
    assert_eq!(
        v["entities"][0]["pos_m"],
        json!({"x": 0.2, "y": 0.0, "z": 2.5})
    );
    assert_eq!(v["entities"][0]["facts"][2]["value"], json!(0.0));
    assert!(j.body.contains(r#""name":"Odd \"one\"""#), "{}", j.body);
}

#[test]
fn every_section_empty() {
    let o = Observation {
        header: header(0, 0.0, false),
        instruments: Vec::new(),
        intents: Vec::new(),
        decision: None,
        percepts: Vec::new(),
        events: Vec::new(),
        cursor: 0,
        lost: 0,
    };
    let t = write_observation(&o, Projection::Text, 13).expect("text");
    assert_eq!(
        t.body,
        "tick 0 t=0.0s seat=skipper observer=Sloop#12
self
"
    );
    let e: Problem = write_observation(&o, Projection::Text, 12).expect_err("too small");
    assert_eq!(e.code, "perception.budget_too_small");
    assert_eq!(e.detail["min_tokens"], json!(13), "{e:?}");
    let j = write_observation(&o, Projection::Json, 100).expect("json");
    assert_eq!(
        j.body,
        r#"{"tick":0,"t_s":0.0,"seat":"skipper","observer":{"id":12,"name":"Sloop"},"omniscient":false,"instruments":[],"intents":[],"entities":[],"events":[],"cursor":0,"omitted":{"entities":{},"events":0,"lost":0},"tokens":52}"#
    );
    // JSON reserves 16 bytes for its tokens member: 204 + 1 + 16 bytes need 56 tokens.
    let e: Problem = write_observation(&o, Projection::Json, 55).expect_err("too small");
    assert_eq!(e.detail["min_tokens"], json!(56), "{e:?}");
    assert!(write_observation(&o, Projection::Json, 56).is_ok());
}

#[test]
fn one_short_item_answers_where_its_omitted_line_would_not_fit() {
    // A short line is smaller than the omitted line, with its hint, that would replace it: the
    // whole answer is the smallest, and the refusal below it names that size
    // (docs/spec/perception-slice2.md 5).
    let h = Header {
        tick: Tick(1800),
        t_s: 30.0,
        seat: None,
        observer: None,
        omniscient: false,
    };
    let c = percept(
        5,
        "C",
        "crate",
        Visibility::Seen,
        Detail::Full,
        90.0,
        50.0,
        Vec::new(),
    );
    let a = write_nearby(&h, std::slice::from_ref(&c), 100, Projection::Text, 11)
        .expect("the whole answer");
    assert_eq!(a.body, "tick 1800\nsee C#5 crate brg 090 rng 50.0m\n");
    assert_eq!(a.tokens, 11);
    let e = write_nearby(&h, &[c], 100, Projection::Text, 10).expect_err("too small");
    assert_eq!(e.code, "perception.budget_too_small");
    assert_eq!(e.detail["min_tokens"], json!(11), "{e:?}");
    let ev = EventView {
        seq: 5,
        tick: Tick(50),
        kind: "sail.set".to_owned(),
        subject: None,
        sense: Sense::Global,
        bearing_deg: None,
        range_m: None,
        data: Vec::new(),
        cause: None,
    };
    let a = write_events(
        &h,
        std::slice::from_ref(&ev),
        0,
        0,
        200,
        Projection::Text,
        9,
    )
    .expect("whole");
    assert_eq!(a.body, "tick 1800\nevent 5 tick=50 sail.set\n");
    assert_eq!((a.tokens, a.cursor), (9, Some(5)));
    let e = write_events(&h, &[ev], 0, 0, 200, Projection::Text, 8).expect_err("too small");
    assert_eq!(e.detail["min_tokens"], json!(9), "{e:?}");
}

#[test]
fn events_with_lost_and_left_out() {
    let ev = |seq: u64| EventView {
        seq,
        tick: Tick(seq * 10),
        kind: "sail.set".to_owned(),
        subject: None,
        sense: Sense::Global,
        bearing_deg: None,
        range_m: None,
        data: Vec::new(),
        cause: (seq > 5).then_some(seq - 1),
    };
    let all: Vec<EventView> = (5..=9).map(ev).collect();
    let h = header(100, 1.0, false);
    let a = write_events(&h, &all, 2, 2, 3, Projection::Text, 100).expect("an answer");
    assert_eq!(
        a.body,
        "tick 100\n\
         event 5 tick=50 sail.set\n\
         event 6 tick=60 sail.set cause=5\n\
         event 7 tick=70 sail.set cause=6\n\
         omitted events=2 lost=2 (events {\"since\":7})\n"
    );
    assert_eq!(a.cursor, Some(7));
    let j = write_events(&h, &all, 2, 2, 200, Projection::Json, 1000).expect("json");
    let v: serde_json::Value = serde_json::from_str(&j.body).expect("JSON");
    assert_eq!(v["cursor"], json!(9));
    assert_eq!(
        v["omitted"],
        json!({"entities": {}, "events": 0, "lost": 2})
    );
}

#[test]
fn describe_with_two_unmet_requirements_and_the_omniscient_header() {
    let d = defs();
    let mut c = percept(
        21,
        "Crate4",
        "crate",
        Visibility::Seen,
        Detail::Full,
        268.0,
        14.0,
        vec![fact(&d, "crate", "alongside", FactValue::Bool(false))],
    );
    c.can = vec![];
    let unmet = vec![
        Problem::new(
            "action.not_seen",
            "Crate4 is not in sight.".to_owned(),
            serde_json::Map::new(),
        ),
        Problem::new(
            "action.out_of_reach",
            "Crate4 is 14.0 m away; within 3 m.".to_owned(),
            serde_json::Map::new(),
        ),
    ];
    let aff = [AffordanceStatus {
        verb: "take_aboard".to_owned(),
        available: false,
        unmet,
    }];
    let h = Header {
        tick: Tick(7),
        t_s: 0.1,
        seat: None,
        observer: None,
        omniscient: true,
    };
    let a = write_describe(&h, &c, &aff, &[], Projection::Text, 400).expect("an answer");
    assert_eq!(
        a.body,
        "tick 7 OMNISCIENT\n\
         see Crate4#21 crate brg 268 rng 14.0m alongside=false\n\
         cannot take_aboard: action.not_seen \"Crate4 is not in sight.\" action.out_of_reach \
         \"Crate4 is 14.0 m away; within 3 m.\"\n"
    );
    let full = pocket_interface::projection::text::header(&h);
    assert_eq!(full, "tick 7 t=0.1s seat=- observer=- OMNISCIENT\n");
    let o = Observation {
        header: header(7, 0.1, true),
        instruments: Vec::new(),
        intents: Vec::new(),
        decision: Some(Rendered {
            json: r#"{"id":"d1"}"#.to_owned(),
            text: "decision d1 event:4:sighted idle:10".to_owned(),
        }),
        percepts: Vec::new(),
        events: Vec::new(),
        cursor: 0,
        lost: 0,
    };
    let t = write_observation(&o, Projection::Text, 100).expect("text");
    assert_eq!(
        t.body,
        "tick 7 t=0.1s seat=skipper observer=Sloop#12 OMNISCIENT\nself\ndecision d1 event:4:sighted idle:10\n"
    );
    let j = write_observation(&o, Projection::Json, 100).expect("json");
    assert!(
        j.body.contains(
            r#""omniscient":true,"instruments":[],"intents":[],"decision":{"id":"d1"},"entities""#
        ),
        "{}",
        j.body
    );
}
