//! Fork lookahead for the reference skipper (docs/spec/player.md 8.1; charter 2 and 6, "fork 前瞻的
//! 收益"; shared/contract/mcp.md 5.5, exact lookahead; shared/benchmark/tasks.md 6.1, the fork
//! condition's limits). A decision model outside the tick, like [`Skipper`], that looks ahead by
//! forking the world.
//!
//! At each decision point on main, [`Lookahead::decide`] builds up to [`Limits::branches`]
//! candidate plans from main's observation (the greedy skipper's own act, each crate in sight, the
//! next mark), forks main once per candidate, plays the candidate in its branch as the seat for an
//! equal share of [`Limits::ticks_per_decision`] ticks, scores the branch's last observation and
//! the warnings its acts were answered with (`value`, [`AGROUND_M`]) and returns the best
//! candidate's act for the caller to send on main.
//!
//! Perception stays honest by construction:
//!
//! - **Main is borrowed, never driven**: `decide` takes `&Game` and only forks it; the caller sends
//!   the chosen act on main. A branch is a separate `Game`, so nothing it does reaches main.
//! - **A branch is reached as the seat alone** ([`Seat`]): every command is a `player.*` command
//!   sent as `Source::Player(index)`. The runtime refuses a player every other command
//!   (`crate::player::permitted`) and the omniscient view, so the model reads, in a branch, exactly
//!   what the seat would observe in that future, through the seat's own profile.
//! - **Branches rank, they never add targets**: candidates come from main's observation alone (a
//!   crate main sees, the charted next mark); a branch's answers only score them. Something a
//!   branch revealed (a crate behind the Isle) never becomes a target on main, though a rollout may
//!   chase it inside the branch, as the seat would in that future.
//! - **Hidden state is simulated**: a branch runs the true world. That is the contract's "exact
//!   lookahead" (mcp.md 5.5), bounded by the limits so a branch cannot scout the course: at most
//!   600 branch ticks between two decisions answered on main and 4 live branches.

use std::collections::BTreeMap;

use amoris_contract::{Problem, detail};
use amoris_link::Source;
use amoris_sim::math;
use serde::Serialize;
use serde_json::{Value, json};

use crate::catalog::Command;
use crate::game::Game;
use crate::skipper::{CRATE_TRIES, Skipper, instrument, percepts, sailing_to};

/// The lookahead's limits: the benchmark's fork condition (shared/benchmark/tasks.md 6.1; mcp.md
/// 3.1, `Policy`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Limits {
    /// Live branches at once (`max_branches`): one per candidate.
    pub branches: usize,
    /// Branch ticks between two decision points answered on main (`branch_ticks_per_decision`),
    /// shared equally by the candidates.
    pub ticks_per_decision: u64,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            branches: 4,
            ticks_per_decision: 600,
        }
    }
}

/// A plan a branch tries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Plan {
    /// What the reference skipper would do, and its rules after.
    Greedy,
    /// Sail to this crate (in sight on main), take it aboard, then on to the next mark.
    Crate(u64),
    /// Sail to the next mark, taking aboard only a crate that comes alongside.
    Mark,
}

/// How far a crate in sight may be to be a candidate, metres (the course's crates are seen within
/// 120 m).
pub const LOOK_RANGE_M: f64 = 120.0;
/// The speed course time converts at, m/s: the reference skipper's mean speed along the course's
/// legs (301 m in 85.8 s, docs/bench/player.md).
pub const V_REF_MPS: f64 = 3.5;
/// What a crate aboard is worth, in ticks of course time (the trade-off the value and the bench's
/// score make; 1000 ticks at 60 Hz is 16.7 s, 58 m of course at [`V_REF_MPS`]).
pub const CRATE_TICKS: f64 = 1000.0;
/// What running aground costs, metres of course; a branch whose act was warned that its course
/// crosses land before its target (`sail.land_on_course`) is charged the same.
pub const AGROUND_M: f64 = 1000.0;
/// How much a plan must beat the greedy skipper's branch by to be chosen, metres of course: the
/// lookahead keeps to the greedy skipper's rules unless a branch shows a clear gain (branches of a
/// few seconds from a standing start differ by centimetres). Chosen between 5 and 15 m on the
/// variants 100 to 119, which the comparison does not use (docs/bench/player.md 5).
pub const SWITCH_M: f64 = 15.0;
/// A chase closing slower than this, m/s, is not credited with its crate.
const V_MIN_MPS: f64 = 0.3;
/// Nor one whose crate is further off than this, seconds at the closing speed measured.
const ETA_MAX_S: f64 = 120.0;
/// Seconds a closing speed must be measured over.
const MEASURE_MIN_S: f64 = 0.5;

/// A crate's worth in metres of course.
pub fn crate_m(rate_hz: f64) -> f64 {
    CRATE_TICKS / rate_hz * V_REF_MPS
}

/// The seat as the model reaches a world: `player.*` commands sent as `Source::Player(index)`,
/// nothing else (the runtime refuses a player any other command as well).
#[derive(Clone, Debug)]
pub struct Seat {
    index: u32,
    seq: u64,
}

impl Seat {
    pub fn new(index: u32) -> Seat {
        Seat { index, seq: 0 }
    }

    pub fn source(&self) -> Source {
        Source::Player(self.index)
    }

    /// Sends `name` (a `player.*` command) to `game` as the seat.
    pub fn call(&mut self, game: &mut Game, name: &str, params: Value) -> Result<Value, Problem> {
        if !name.starts_with("player.") {
            return Err(Problem::new(
                "permission.denied",
                "The lookahead reaches a world as its seat: player tools only.",
                detail([("command", json!(name))]),
            ));
        }
        self.seq += 1;
        game.apply(&Command::new(self.source(), self.seq, name, params))
    }
}

/// The observation the model reads at a stop: JSON at a budget that holds the course whole.
pub fn observe_request() -> Value {
    json!({"projection": "json", "budget_tokens": 1500})
}

/// A mark with a place on the chart.
#[derive(Clone, Debug)]
struct Charted {
    order: u64,
    name: String,
    at: (f64, f64),
}

/// A crate in sight.
#[derive(Clone, Debug)]
struct Sighted {
    id: u64,
    range: f64,
    at: Option<(f64, f64)>,
}

/// What the model reads of an observation.
#[derive(Clone, Debug, Default)]
struct View {
    t_s: f64,
    pos: Option<(f64, f64)>,
    next_mark: String,
    taken: f64,
    afloat: bool,
    marks: Vec<Charted>,
    crates: Vec<Sighted>,
    /// The live `sail_to`'s target id and name.
    target: Option<(u64, String)>,
}

fn place(v: &Value) -> Option<(f64, f64)> {
    Some((v["x"].as_f64()?, v["z"].as_f64()?))
}

fn fact<'a>(p: &'a Value, name: &str) -> &'a Value {
    p["facts"]
        .as_array()
        .and_then(|a| a.iter().find(|r| r["name"] == json!(name)))
        .map_or(&Value::Null, |r| &r["value"])
}

fn view(obs: &Value) -> View {
    let mut v = View {
        t_s: obs["t_s"].as_f64().unwrap_or(0.0),
        pos: place(instrument(obs, "pos_m")),
        next_mark: instrument(obs, "next_mark")
            .as_str()
            .unwrap_or("none")
            .to_owned(),
        taken: instrument(obs, "taken").as_f64().unwrap_or(0.0),
        afloat: instrument(obs, "afloat").as_bool().unwrap_or(true),
        target: sailing_to(obs),
        ..View::default()
    };
    for p in obs["entities"].as_array().into_iter().flatten() {
        let kind = p["kind"].as_str().unwrap_or_default();
        if kind == "mark"
            && let (Some(order), Some(at)) = (fact(p, "order").as_u64(), place(&p["pos_m"]))
        {
            let name = p["name"].as_str().unwrap_or_default().to_owned();
            v.marks.push(Charted { order, name, at });
        } else if kind == "crate" && p["visibility"] == json!("seen") {
            let id = p["id"].as_u64().unwrap_or(0);
            let range = p["range_m"].as_f64().unwrap_or(f64::MAX);
            v.crates.push(Sighted {
                id,
                range,
                at: place(&p["pos_m"]),
            });
        }
    }
    v.marks
        .sort_by(|a, b| a.order.cmp(&b.order).then(a.name.cmp(&b.name)));
    v
}

fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    math::hypot(a.0 - b.0, a.1 - b.1)
}

impl View {
    /// The next mark's place and the length of the course after it, from the chart.
    fn ahead(&self) -> Option<((f64, f64), f64)> {
        let i = self.marks.iter().position(|m| m.name == self.next_mark)?;
        let mut rest = 0.0;
        for w in self.marks[i..].windows(2) {
            rest += dist(w[0].at, w[1].at);
        }
        Some((self.marks[i].at, rest))
    }

    fn crate_at(&self, id: u64) -> Option<&Sighted> {
        self.crates.iter().find(|c| c.id == id)
    }
}

/// Where each crate was first seen in a branch: seconds and range.
type FirstSeen = BTreeMap<u64, (f64, f64)>;

fn note_seen(first: &mut FirstSeen, v: &View) {
    for c in &v.crates {
        first.entry(c.id).or_insert((v.t_s, c.range));
    }
}

/// A branch's value, in metres of course, from the seat's last observation in it: crates aboard
/// at [`crate_m`] each, less the charted course still to sail (the distance to the next mark and
/// the legs after it), less [`AGROUND_M`] when aground. A chase still under way is credited with
/// its crate when the closing speed measured in the branch would bring it alongside within
/// two minutes: the crate's worth less the time to reach it (at that speed, in metres at
/// [`V_REF_MPS`]) and the course from the crate on, if that beats sailing on. A finished course
/// adds the time left in the branch, in metres at [`V_REF_MPS`].
fn value(end: &View, first: &FirstSeen, done: bool, left_s: f64, crate_m: f64) -> f64 {
    let mut v = crate_m * end.taken;
    if done || end.next_mark == "none" {
        return v + V_REF_MPS * left_s;
    }
    let Some(pos) = end.pos else {
        return v - AGROUND_M;
    };
    let Some((next, rest)) = end.ahead() else {
        return v;
    };
    let sail_on = dist(pos, next) + rest;
    let mut best = -sail_on;
    if let Some((id, _)) = &end.target
        && let Some(Sighted {
            range,
            at: Some(at),
            ..
        }) = end.crate_at(*id)
        && let Some(&(t0, r0)) = first.get(id)
        && end.t_s - t0 >= MEASURE_MIN_S
    {
        let (range, at) = (*range, *at);
        let closing = (r0 - range) / (end.t_s - t0);
        if closing > V_MIN_MPS && range / closing <= ETA_MAX_S {
            let chase = crate_m - (V_REF_MPS * range / closing + dist(at, next) + rest);
            best = math::max(best, chase);
        }
    }
    v += best;
    if !end.afloat {
        v -= AGROUND_M;
    }
    v
}

/// A candidate's value with forks off ([`Lookahead::without_forks`]), in the units of [`value`],
/// from main's observation and the chart alone: crates aboard at `crate_m` each, then, for a
/// candidate heading for a crate in sight (`to` is `#id`), the crate's worth less the straight
/// course to it, on to the next mark and the legs after it; for any other, less the straight
/// course to the next mark and the legs after it. What the model knows without a branch: no
/// wind, no land on the way, no closing speed.
fn estimate(v: &View, to: Option<&str>, crate_m: f64) -> f64 {
    let base = crate_m * v.taken;
    let (Some(pos), Some((next, rest))) = (v.pos, v.ahead()) else {
        return base;
    };
    let chased = to
        .and_then(|t| t.strip_prefix('#'))
        .and_then(|id| id.parse::<u64>().ok())
        .and_then(|id| v.crate_at(id))
        .and_then(|c| c.at);
    base + match chased {
        Some(at) => crate_m - (dist(pos, at) + dist(at, next) + rest),
        None => -(dist(pos, next) + rest),
    }
}

/// The candidate a decision chooses, by index: the best value among the candidates whose first
/// act their branch did not refuse (main would refuse it too), if it beats the greedy
/// candidate's, the first, by `switch_m`; else the greedy candidate. Ties keep the earlier one.
fn choose(branches: &[Branch], switch_m: f64) -> usize {
    let mut best = 0;
    for (i, b) in branches.iter().enumerate().skip(1) {
        if b.refused.is_some() {
            continue;
        }
        let bar = if best == 0 { switch_m } else { 0.0 };
        if b.value > branches[best].value + bar {
            best = i;
        }
    }
    best
}

/// Whether an act's answer warns that the course it set crosses land before its target: a
/// `sail.land_on_course` warning nearer than a started `sail_to`'s distance (`sail_to` does not
/// steer round land, so the boat would run aground).
fn land_ahead(answer: &Value) -> bool {
    let to = answer["outcomes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| {
            o["progress"]
                .as_array()?
                .iter()
                .find(|r| r["name"] == json!("distance_m"))?["value"]
                .as_f64()
        })
        .fold(0.0, math::max);
    answer["warnings"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|w| {
            w["code"] == json!("sail.land_on_course")
                && w["detail"]["along_m"].as_f64().is_some_and(|a| a < to)
        })
}

/// A crate the seat can take aboard now.
fn take_act(obs: &Value) -> Option<Value> {
    percepts(obs)
        .iter()
        .find(|p| p.kind == "crate" && p.can.iter().any(|v| v == "take_aboard"))
        .map(|c| json!({"actions": [{"do": "use", "entity": c.id, "verb": "take_aboard"}]}))
}

fn sail_to_crate(id: u64) -> Value {
    json!({"actions": [{"do": "start", "intent": "sail_to", "target": id,
        "params": {"arrive_m": 2}, "tag": "crate"}]})
}

/// Sail to the next mark, unless already.
fn mark_act(obs: &Value) -> Option<Value> {
    let next = instrument(obs, "next_mark").as_str().unwrap_or("none");
    if next == "none" || sailing_to(obs).is_some_and(|t| t.1 == next) {
        return None;
    }
    Some(
        json!({"actions": [{"do": "start", "intent": "sail_to", "target": next,
        "params": {"arrive_m": 8}, "tag": "mark"}]}),
    )
}

/// What a plan's first act on a world in the state `obs` describes is, or `None` to sail on. A
/// crate plan sets out for its crate at most [`CRATE_TRIES`] times, as the greedy skipper does
/// (the starts counted in `greedy`, the model's memory of its own acts), then sails on to the
/// mark.
fn first_act(plan: &Plan, obs: &Value, greedy: &mut Skipper) -> Option<Value> {
    match plan {
        Plan::Greedy => greedy.decide(obs),
        Plan::Crate(id) => {
            if sailing_to(obs).map(|t| t.0) == Some(*id) {
                None
            } else if greedy.tries(*id) < CRATE_TRIES {
                greedy.started(*id);
                Some(sail_to_crate(*id))
            } else {
                mark_act(obs)
            }
        }
        Plan::Mark => mark_act(obs),
    }
}

/// A rollout's act at a decision point inside its branch.
fn rollout_act(plan: &Plan, obs: &Value, greedy: &mut Skipper) -> Option<Value> {
    match plan {
        Plan::Greedy => greedy.decide(obs),
        Plan::Crate(id) => {
            if let Some(take) = take_act(obs) {
                return Some(take);
            }
            let seen = percepts(obs)
                .iter()
                .any(|p| p.id == *id && p.visibility == "seen");
            if seen {
                first_act(plan, obs, greedy)
            } else {
                mark_act(obs)
            }
        }
        Plan::Mark => take_act(obs).or_else(|| mark_act(obs)),
    }
}

/// Where an act sends the boat, `#id` for a crate or the mark's name, or else where the seat's live
/// `sail_to` does: the target a candidate would leave the boat sailing to.
fn heading_for(act: Option<&Value>, obs: &Value) -> Option<String> {
    if let Some(a) = act.map(|a| &a["actions"][0])
        && a["do"] == json!("start")
        && a["intent"] == json!("sail_to")
    {
        return Some(match &a["target"] {
            Value::String(name) => name.clone(),
            t => format!("#{t}"),
        });
    }
    let (id, name) = sailing_to(obs)?;
    let mark = percepts(obs).iter().any(|p| p.id == id && p.kind == "mark");
    Some(if mark { name } else { format!("#{id}") })
}

/// One branch: the plan it tried and what came of it.
#[derive(Clone, Debug, Serialize)]
pub struct Branch {
    pub plan: Plan,
    /// The plan's first act in its branch (`None`: sail on); `refused` if the branch refused it.
    pub act: Option<Value>,
    pub refused: Option<String>,
    /// An act in the branch was warned that its course crosses land (counted as aground).
    pub land: bool,
    pub ticks: u64,
    pub acts: u32,
    pub value: f64,
    pub done: bool,
}

/// What one decision on main weighed and chose.
#[derive(Clone, Debug, Serialize)]
pub struct Decision {
    pub tick: u64,
    pub branches: Vec<Branch>,
    pub chosen: Plan,
    pub act: Option<Value>,
}

/// What the lookahead did over an episode.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Stats {
    /// Decisions answered on main.
    pub decisions: u32,
    /// Decisions that ran branches (two candidates or more).
    pub looked: u32,
    /// Branches opened: forks of main.
    pub forks: u32,
    /// Ticks simulated in branches, in all.
    pub branch_ticks: u64,
    /// The most branch ticks of one decision (at most `Limits::ticks_per_decision`).
    pub max_branch_ticks_per_decision: u64,
    /// Acts sent in branches.
    pub branch_acts: u32,
    /// Decisions whose chosen plan was not the greedy skipper's.
    pub overrides: u32,
}

/// The lookahead skipper.
pub struct Lookahead {
    limits: Limits,
    /// The greedy skipper on main: the first candidate and every rollout's rules.
    skipper: Skipper,
    seat: Seat,
    /// Branch games kept between decisions and forked into again (their script host instantiated
    /// once), unless `fresh`.
    pool: Vec<Game>,
    fresh: bool,
    /// Fork off: candidates valued by [`estimate`] instead of branches.
    no_forks: bool,
    switch_m: f64,
    pub stats: Stats,
}

impl Lookahead {
    /// The lookahead for the seat of index `seat`.
    pub fn new(limits: Limits, seat: u32) -> Lookahead {
        Lookahead {
            limits,
            skipper: Skipper::new(),
            seat: Seat::new(seat),
            pool: Vec::new(),
            fresh: false,
            no_forks: false,
            switch_m: SWITCH_M,
            stats: Stats::default(),
        }
    }

    /// The same model with forks off, the control that measures what forking adds: the same
    /// candidates, margin and rules, each candidate valued from main's observation and the chart
    /// alone ([`estimate`]) rather than played in a branch. It never forks.
    pub fn without_forks(mut self) -> Lookahead {
        self.no_forks = true;
        self
    }

    /// Another margin than [`SWITCH_M`] for leaving the greedy skipper's plan.
    pub fn switch_margin(mut self, metres: f64) -> Lookahead {
        self.switch_m = metres;
        self
    }

    /// Every branch a fresh fork (`Game::fork`, a new script host each time) rather than a kept
    /// branch forked into again (`Game::fork_into`).
    pub fn fresh_branches(mut self) -> Lookahead {
        self.fresh = true;
        self
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// The candidates for this observation on main, at most `limits.branches`, each leaving the
    /// boat sailing somewhere else: the greedy skipper's act first, then the next mark, then the
    /// crates in sight within [`LOOK_RANGE_M`], nearest first. (plan, first act).
    fn candidates(&self, obs: &Value) -> Vec<(Plan, Option<Value>)> {
        let g = self.skipper.clone().decide(obs);
        let mut heading = vec![heading_for(g.as_ref(), obs)];
        let mut out = vec![(Plan::Greedy, g)];
        let v = view(obs);
        let current = v.target.as_ref().map(|t| t.0);
        let mut crates: Vec<_> = v
            .crates
            .iter()
            .filter(|c| c.range <= LOOK_RANGE_M)
            .filter(|c| self.skipper.tries(c.id) < CRATE_TRIES || current == Some(c.id))
            .collect();
        crates.sort_by(|a, b| a.range.total_cmp(&b.range).then(a.id.cmp(&b.id)));
        let plans = std::iter::once(Plan::Mark).chain(crates.iter().map(|c| Plan::Crate(c.id)));
        for plan in plans {
            if out.len() >= self.limits.branches {
                break;
            }
            let act = first_act(&plan, obs, &mut self.skipper.clone());
            let to = heading_for(act.as_ref(), obs);
            if to.is_none() || heading.contains(&to) {
                continue;
            }
            heading.push(to);
            out.push((plan, act));
        }
        out
    }

    /// A branch of `main` ready to step: a kept game forked into again, or a fresh fork (when
    /// none is kept, or the kept one runs another bundle).
    fn branch(main: &Game, kept: Option<Game>) -> Result<Game, Problem> {
        if let Some(mut g) = kept
            && main.fork_into(&mut g).is_ok()
        {
            return Ok(g);
        }
        main.fork()
    }

    /// Plays `plan` in `branch` for `ticks` ticks as the seat; returns the branch's result.
    fn rollout(
        &mut self,
        branch: &mut Game,
        plan: &Plan,
        act: Option<&Value>,
        obs: &Value,
        ticks: u64,
        crate_m: f64,
    ) -> Result<Branch, Problem> {
        let mut seat = self.seat.clone();
        // The candidate's first act was decided on a clone of the skipper: decide it again on
        // this one, so its memory (the crates set out for) follows the branch.
        let mut greedy = self.skipper.clone();
        first_act(plan, obs, &mut greedy);
        let from = branch.tick().0;
        let mut first = FirstSeen::new();
        let start = view(obs);
        note_seen(&mut first, &start);
        let mut acts = 0;
        let mut refused = None;
        let mut land = false;
        if let Some(a) = act {
            match seat.call(branch, "player.act", a.clone()) {
                Ok(answer) => {
                    acts += 1;
                    land |= land_ahead(&answer);
                }
                Err(p) => refused = Some(p.code),
            }
        }
        let mut last = start;
        let mut done = false;
        loop {
            let used = branch.tick().0 - from;
            if used >= ticks {
                break;
            }
            let w = seat.call(
                branch,
                "player.wait",
                json!({"ticks": ticks - used, "observe": observe_request()}),
            )?;
            let o = &w["observation"];
            last = view(o);
            note_seen(&mut first, &last);
            match w["stopped"].as_str() {
                Some("done") => {
                    done = true;
                    break;
                }
                Some("halted") => break,
                _ => {}
            }
            if branch.tick().0 - from >= ticks {
                break;
            }
            if let Some(a) = rollout_act(plan, o, &mut greedy)
                && let Ok(answer) = seat.call(branch, "player.act", a)
            {
                acts += 1;
                land |= land_ahead(&answer);
            }
        }
        let ran = branch.tick().0 - from;
        let left_s = secs(ticks - ran.min(ticks), branch);
        let mut v = value(&last, &first, done, left_s, crate_m);
        if land {
            v -= AGROUND_M;
        }
        Ok(Branch {
            plan: plan.clone(),
            act: act.cloned(),
            refused,
            land,
            ticks: ran,
            acts,
            value: v,
            done,
        })
    }

    /// The act for this observation of the seat on `main` (at a decision point), or `None` to let
    /// time pass. Forks `main` once per candidate, never changes it; the caller sends the act.
    pub fn decide(&mut self, main: &Game, obs: &Value) -> Result<Option<Value>, Problem> {
        Ok(self.weigh(main, obs)?.act)
    }

    /// [`Lookahead::decide`] with what it weighed.
    pub fn weigh(&mut self, main: &Game, obs: &Value) -> Result<Decision, Problem> {
        self.stats.decisions += 1;
        let tick = main.tick().0;
        // A crate alongside is taken aboard: no branch can beat it.
        if let Some(take) = take_act(obs) {
            self.skipper.decide(obs);
            return Ok(Decision {
                tick,
                branches: Vec::new(),
                chosen: Plan::Greedy,
                act: Some(take),
            });
        }
        // Aground, every `sail_to` fails at once (`sail.aground`): no branch can tell the plans
        // apart, so the greedy skipper decides (a grounded boat meets a decision point every other
        // tick, and weighing each would spend 600 branch ticks to learn nothing).
        let aground = !instrument(obs, "afloat").as_bool().unwrap_or(true);
        let candidates = if aground {
            Vec::new()
        } else {
            self.candidates(obs)
        };
        if candidates.len() < 2 {
            let act = self.skipper.decide(obs);
            return Ok(Decision {
                tick,
                branches: Vec::new(),
                chosen: Plan::Greedy,
                act,
            });
        }
        self.stats.looked += 1;
        let crate_m = crate_m(rate_hz(main));
        if self.no_forks {
            // Fork off: each candidate valued from main's observation and the chart alone.
            let v = view(obs);
            let branches = candidates
                .iter()
                .map(|(plan, act)| Branch {
                    plan: plan.clone(),
                    act: act.clone(),
                    refused: None,
                    land: false,
                    ticks: 0,
                    acts: 0,
                    value: estimate(&v, heading_for(act.as_ref(), obs).as_deref(), crate_m),
                    done: false,
                })
                .collect();
            return Ok(self.chosen(obs, &candidates, branches, tick));
        }
        let each = self.limits.ticks_per_decision / u64::try_from(candidates.len()).unwrap_or(1);
        let mut branches = Vec::with_capacity(candidates.len());
        let mut spare = std::mem::take(&mut self.pool).into_iter();
        let mut kept = Vec::new();
        for (plan, act) in &candidates {
            let mut b = Lookahead::branch(main, if self.fresh { None } else { spare.next() })?;
            self.stats.forks += 1;
            let r = self.rollout(&mut b, plan, act.as_ref(), obs, each, crate_m)?;
            self.stats.branch_ticks += r.ticks;
            self.stats.branch_acts += r.acts;
            branches.push(r);
            if !self.fresh {
                kept.push(b);
            }
        }
        // The branch games are kept for the next decision, as many as the limit allows.
        kept.extend(spare);
        kept.truncate(self.limits.branches);
        self.pool = kept;
        let spent: u64 = branches.iter().map(|b| b.ticks).sum();
        self.stats.max_branch_ticks_per_decision =
            self.stats.max_branch_ticks_per_decision.max(spent);
        Ok(self.chosen(obs, &candidates, branches, tick))
    }

    /// The decision once every candidate has a value: [`choose`]'s candidate, its first act
    /// decided again on the skipper on main so that its memory follows main.
    fn chosen(
        &mut self,
        obs: &Value,
        candidates: &[(Plan, Option<Value>)],
        branches: Vec<Branch>,
        tick: u64,
    ) -> Decision {
        let best = choose(&branches, self.switch_m);
        let (plan, act) = candidates[best].clone();
        // The skipper's memory follows main: the chosen plan's first act decided on it.
        first_act(&plan, obs, &mut self.skipper);
        if plan != Plan::Greedy {
            self.stats.overrides += 1;
        }
        Decision {
            tick,
            branches,
            chosen: plan,
            act,
        }
    }
}

/// The game's tick rate, ticks per second.
fn rate_hz(g: &Game) -> f64 {
    f64::from(g.setup().rate.0)
}

/// `ticks` of `g`'s rate, in seconds.
fn secs(ticks: u64, g: &Game) -> f64 {
    amoris_sim::Tick(ticks).to_f64() / rate_hz(g)
}

/// A decision model of the course played through the player tools: the reference skipper, or the
/// lookahead around it.
pub enum Model {
    Greedy(Skipper),
    Lookahead(Box<Lookahead>),
}

/// An episode played through the player tools.
#[derive(Clone, Debug, Serialize)]
pub struct Episode {
    /// The course finished (the episode's `done`).
    pub finished: bool,
    /// Main's ticks from the episode's start to its end (the finish, or the tick limit).
    pub ticks: u64,
    /// Crates aboard at the end (the seat's `taken`).
    pub taken: f64,
    /// The next mark at the end: `none` once finished.
    pub next_mark: String,
    /// Decision points answered on main, and acts sent on main.
    pub decisions: u32,
    pub acts: u32,
    /// The lookahead's own counts.
    pub lookahead: Option<Stats>,
    /// Every act sent on main, with its tick.
    pub acts_log: Vec<(u64, Value)>,
    /// The lookahead's decisions that ran branches: what each weighed and chose.
    pub weighed: Vec<Decision>,
    /// Main's world hash at the end.
    pub world_hash: String,
    /// Wall seconds inside the model's decisions, when the caller gave a clock.
    pub decide_s: f64,
}

/// Plays `game` from its current tick as the seat of index `seat` with `model` until the episode
/// ends or `max_ticks` have run (every wait runs a tick at least, so as many decisions at most):
/// observe, decide, act, wait for the next decision point. `clock`
/// (a wall clock in seconds, outside every tick) times the model's decisions.
pub fn play(
    game: &mut Game,
    model: &mut Model,
    seat: u32,
    max_ticks: u64,
    clock: Option<fn() -> f64>,
) -> Result<Episode, Problem> {
    let mut s = Seat::new(seat);
    let start = game.tick().0;
    let now = || clock.map_or(0.0, |c| c());
    let mut ep = Episode {
        finished: false,
        ticks: 0,
        taken: 0.0,
        next_mark: String::new(),
        decisions: 0,
        acts: 0,
        lookahead: None,
        acts_log: Vec::new(),
        weighed: Vec::new(),
        world_hash: String::new(),
        decide_s: 0.0,
    };
    loop {
        let obs = s.call(game, "player.observe", observe_request())?;
        ep.decisions += 1;
        let t0 = now();
        let act = match model {
            Model::Greedy(k) => k.decide(&obs),
            Model::Lookahead(l) => {
                let d = l.weigh(game, &obs)?;
                let act = d.act.clone();
                if !d.branches.is_empty() {
                    ep.weighed.push(d);
                }
                act
            }
        };
        ep.decide_s += now() - t0;
        if let Some(a) = act {
            s.call(game, "player.act", a.clone())?;
            ep.acts += 1;
            ep.acts_log.push((game.tick().0, a));
        }
        let left = max_ticks.saturating_sub(game.tick().0 - start);
        if left == 0 {
            break;
        }
        let before = game.tick();
        let w = s.call(
            game,
            "player.wait",
            json!({"ticks": left.min(crate::player::WAIT_TICKS)}),
        )?;
        if game.tick() == before {
            break;
        }
        match w["stopped"].as_str() {
            Some("done") => {
                ep.finished = true;
                break;
            }
            Some("halted") => break,
            _ => {}
        }
    }
    let end = s.call(game, "player.observe", observe_request())?;
    ep.ticks = game.tick().0 - start;
    ep.taken = instrument(&end, "taken").as_f64().unwrap_or(0.0);
    ep.next_mark = instrument(&end, "next_mark")
        .as_str()
        .unwrap_or("none")
        .to_owned();
    if let Model::Lookahead(l) = model {
        ep.lookahead = Some(l.stats.clone());
    }
    ep.world_hash = game.world_hash()?.to_string();
    Ok(ep)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn branch(plan: Plan, value: f64, refused: bool) -> Branch {
        Branch {
            plan,
            act: None,
            refused: refused.then(|| "intent.unknown_target".to_owned()),
            land: false,
            ticks: 0,
            acts: 0,
            value,
            done: false,
        }
    }

    /// The choice keeps the greedy candidate unless another beats it by the margin, takes the
    /// best of those that do, and never a candidate whose first act its branch refused (main
    /// would refuse it too, and the episode would end on the refusal).
    #[test]
    fn the_choice_needs_the_margin_and_skips_refused_branches() {
        let g = || branch(Plan::Greedy, 0.0, false);
        assert_eq!(choose(&[g(), branch(Plan::Mark, 10.0, false)], 15.0), 0);
        assert_eq!(choose(&[g(), branch(Plan::Mark, 20.0, false)], 15.0), 1);
        let three = [
            g(),
            branch(Plan::Crate(9), 20.0, false),
            branch(Plan::Crate(10), 30.0, false),
        ];
        assert_eq!(choose(&three, 15.0), 2);
        let refused = [
            g(),
            branch(Plan::Crate(9), 20.0, false),
            branch(Plan::Crate(10), 300.0, true),
        ];
        assert_eq!(choose(&refused, 15.0), 1);
        assert_eq!(choose(&[g(), branch(Plan::Mark, 300.0, true)], 15.0), 0);
    }

    /// With forks off, a crate in sight is worth its value less the straight course through it,
    /// any other heading the straight course on; neither knows the wind or the land.
    #[test]
    fn the_estimate_reads_the_chart_alone() {
        let v = View {
            pos: Some((0.0, 0.0)),
            next_mark: "M1".to_owned(),
            taken: 1.0,
            marks: vec![
                Charted {
                    order: 1,
                    name: "M1".to_owned(),
                    at: (100.0, 0.0),
                },
                Charted {
                    order: 2,
                    name: "M2".to_owned(),
                    at: (100.0, 50.0),
                },
            ],
            crates: vec![Sighted {
                id: 9,
                range: 50.0,
                at: Some((30.0, 40.0)),
            }],
            ..View::default()
        };
        let on = estimate(&v, Some("M1"), 60.0);
        assert!((on - (60.0 - 150.0)).abs() < 1e-9, "{on}");
        assert!((estimate(&v, None, 60.0) - on).abs() < 1e-9);
        // To the crate 50 m, from it to M1 math::hypot(70, 40), then M1 to M2 50 m.
        let chase = estimate(&v, Some("#9"), 60.0);
        let want = 60.0 + 60.0 - (50.0 + math::hypot(70.0, 40.0) + 50.0);
        assert!((chase - want).abs() < 1e-9, "{chase} against {want}");
        // A crate not in sight is valued as sailing on.
        assert!((estimate(&v, Some("#12"), 60.0) - on).abs() < 1e-9);
    }
}
