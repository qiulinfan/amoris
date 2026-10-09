//! The text projection (shared/contract/projection.md, Text projection): one item a line, LF line
//! ends, ASCII except inside names and text facts. Each function writes one production of the
//! grammar, its line end included, so an answer is their concatenation and its size the sum of
//! theirs.

use super::round;
use super::{Header, Omitted};
use crate::perception::query::AffordanceStatus;
use crate::perception::state::{Named, Visibility};
use crate::perception::view::{EventView, Percept, Reading};

/// `REF := [NAME] "#" ENTITY_ID`.
pub fn reference(id: u64, name: Option<&str>) -> String {
    format!("{}#{id}", name.unwrap_or(""))
}

fn named(n: &Named) -> String {
    reference(n.id.get(), n.name.as_deref())
}

/// `header := "tick " TICK " t=" SECONDS "s seat=" SEAT " observer=" REF [" OMNISCIENT"] NL`;
/// the raw omniscient view, which has no observer, writes `seat=- observer=-`.
pub fn header(h: &Header) -> String {
    let observer = h.observer.as_ref().map_or_else(|| "-".to_owned(), named);
    format!(
        "tick {} t={}s seat={} observer={}{}\n",
        h.tick.0,
        round::fixed(h.t_s, 1),
        h.seat.as_deref().unwrap_or("-"),
        observer,
        if h.omniscient { " OMNISCIENT" } else { "" }
    )
}

/// `header-short := "tick " TICK [" OMNISCIENT"] NL`.
pub fn header_short(tick: u64, omniscient: bool) -> String {
    format!(
        "tick {tick}{}\n",
        if omniscient { " OMNISCIENT" } else { "" }
    )
}

fn pairs(out: &mut String, readings: &[Reading]) {
    for r in readings {
        out.push(' ');
        out.push_str(&r.name);
        out.push('=');
        out.push_str(&round::text(&r.value, r.format));
    }
}

/// `instruments := "self" (" " NAME "=" VALUE)* NL`.
pub fn instruments(readings: &[Reading]) -> String {
    let mut s = "self".to_owned();
    pairs(&mut s, readings);
    s.push('\n');
    s
}

/// `" brg " BBB " rng " RANGE "m"`.
fn bearing_range(b: f64, r: f64) -> String {
    format!(
        " brg {} rng {}m",
        round::bbb(b, 0),
        round::fixed(r, round::range_precision(r))
    )
}

/// `entity := VIS " " REF " " KIND " brg " BBB " rng " RANGE "m" [" age=" SECONDS "s"]
/// (" " NAME "=" VALUE)* [" can=" VERB ("," VERB)*] NL`.
pub fn entity(p: &Percept) -> String {
    let vis = match p.visibility {
        Visibility::Seen => "see",
        Visibility::Remembered => "mem",
        Visibility::Charted => "chart",
    };
    let mut s = format!(
        "{vis} {} {}{}",
        reference(p.id.get(), p.name.as_deref()),
        p.kind,
        bearing_range(p.bearing_deg, p.range_m)
    );
    if let Some(age) = p.age_s {
        s.push_str(&format!(" age={}s", round::fixed(age, 1)));
    }
    // The grammar has no position: `pos_m` is the JSON projection's (sailing.md's example).
    pairs(&mut s, &p.facts);
    if !p.can.is_empty() {
        s.push_str(" can=");
        s.push_str(&p.can.join(","));
    }
    s.push('\n');
    s
}

/// `event := "event " SEQ " tick=" TICK " " KIND [" " REF] [" brg " BBB " rng " RANGE "m"]
/// (" " NAME "=" VALUE)* [" cause=" SEQ] NL`.
pub fn event(e: &EventView) -> String {
    let mut s = format!("event {} tick={} {}", e.seq, e.tick.0, e.kind);
    if let Some(n) = &e.subject {
        s.push(' ');
        s.push_str(&named(n));
    }
    if let (Some(b), Some(r)) = (e.bearing_deg, e.range_m) {
        s.push_str(&bearing_range(b, r));
    }
    pairs(&mut s, &e.data);
    if let Some(c) = e.cause {
        s.push_str(&format!(" cause={c}"));
    }
    s.push('\n');
    s
}

/// `affordance := ("can " VERB | "cannot " VERB ":" (" " CODE " " MESSAGE)+) NL`.
pub fn affordance(a: &AffordanceStatus) -> String {
    if a.available {
        return format!("can {}\n", a.verb);
    }
    let mut s = format!("cannot {}:", a.verb);
    for p in &a.unmet {
        s.push_str(&format!(" {} {}", p.code, round::quote(&p.message)));
    }
    s.push('\n');
    s
}

/// A line another layer rendered, with its line end.
pub fn line(text: &str) -> String {
    format!("{text}\n")
}

/// `omitted := "omitted" (" " KIND "=" COUNT)* [" events=" COUNT] [" lost=" COUNT]
/// " (" HINT ("; " HINT)* ")" NL`, written only when something was left out. The hints name the
/// calls that would show what was left out: `nearby {"kinds":[...]}` and `events {"since":N}`.
pub fn omitted(o: &Omitted) -> String {
    if !o.any() {
        return String::new();
    }
    let mut s = "omitted".to_owned();
    for (kind, n) in &o.entities {
        s.push_str(&format!(" {kind}={n}"));
    }
    if o.events > 0 {
        s.push_str(&format!(" events={}", o.events));
    }
    if o.lost > 0 {
        s.push_str(&format!(" lost={}", o.lost));
    }
    let mut hints = Vec::new();
    if !o.entities.is_empty() {
        let kinds: Vec<String> = o.entities.keys().map(|k| round::quote(k)).collect();
        hints.push(format!("nearby {{\"kinds\":[{}]}}", kinds.join(",")));
    }
    if o.events > 0 || o.lost > 0 {
        hints.push(format!("events {{\"since\":{}}}", o.cursor));
    }
    s.push_str(&format!(" ({})\n", hints.join("; ")));
    s
}

/// `omitted-events := "omitted" " events=" COUNT [" lost=" COUNT] " (" HINT ")" NL`, for the
/// events delta of act and time answers.
pub fn omitted_events(o: &Omitted) -> String {
    if o.events == 0 && o.lost == 0 {
        return String::new();
    }
    let mut s = format!("omitted events={}", o.events);
    if o.lost > 0 {
        s.push_str(&format!(" lost={}", o.lost));
    }
    s.push_str(&format!(" (events {{\"since\":{}}})\n", o.cursor));
    s
}
