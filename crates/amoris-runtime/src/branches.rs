//! Branches for an agent's lookahead on the game thread (docs/spec/player.md 7.1;
//! shared/contract/mcp.md 5.5): `player.fork` forks main (or one of the caller's branches) into a
//! detached game that only its source reaches, every other `player.*` command reaches it by
//! `world`, and `player.discard` drops it. A branch runs stepped, inline on the game thread
//! between main's boundaries; it records, publishes and presents nothing, and nothing it does
//! reaches main.
//!
//! The bounds are counted here, not trusted to the agent: at most [`PER_SOURCE`] live branches a
//! source and [`LIVE`] on the thread, and at most [`PER_DECISION`] branch ticks a source simulates
//! while main stays at one tick (the benchmark's fork condition, shared/benchmark/tasks.md 6.1),
//! a `wait` that asks for more refused up front.

use std::collections::BTreeMap;
use std::sync::Arc;

use amoris_contract::{Problem, detail};
use amoris_link::Source;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::catalog::Command;
use crate::game::Game;

/// Live branches a source holds at once (mcp.md 3.1, `Policy.max_branches`' default).
pub const PER_SOURCE: usize = 4;
/// Live branches on one game thread, of all sources (mcp.md 3.1, `MAX_BRANCHES`).
pub const LIVE: usize = 16;
/// Branch ticks a source simulates while main stays at one tick (mcp.md 3.1,
/// `branch_ticks_per_decision`, as the benchmark's fork condition sets it).
pub const PER_DECISION: u64 = 600;
/// The longest label kept.
const LABEL_MAX: usize = 64;

/// `player.fork`'s parameters.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForkParams {
    /// The world to fork: "main" (the default) or one of your branches.
    #[serde(default)]
    pub world: Option<String>,
    /// A name of your own for the branch, listed by player.branches.
    #[serde(default)]
    pub label: Option<String>,
}

/// `player.discard`'s parameters.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiscardParams {
    /// The branch to drop ("b1", ...); main cannot be discarded.
    pub branch: String,
}

/// One live branch.
struct Branch {
    game: Game,
    owner: Source,
    parent: String,
    forked_at: u64,
    label: Option<String>,
}

/// What a source has spent.
#[derive(Default)]
struct Spent {
    /// Main's tick the per-decision count belongs to.
    at: u64,
    /// Branch ticks simulated while main was at `at`.
    ticks: u64,
    forks: u64,
    branch_ticks: u64,
    branch_acts: u64,
}

/// Whether a source still has a live client: a branch whose source has none ends at the next
/// fork (the session that made it went away).
pub type Alive = Arc<dyn Fn(Source) -> bool + Send + Sync>;

/// The game thread's branches.
pub struct Branches {
    /// The last `n` of a `b<n>`; never reused within the thread's life.
    made: u64,
    live: BTreeMap<String, Branch>,
    spent: BTreeMap<Source, Spent>,
    alive: Alive,
}

fn unknown_world(path: &str, world: &str, worlds: Vec<String>) -> Problem {
    let mut all = vec!["main".to_owned()];
    all.extend(worlds);
    Problem::new(
        "session.unknown_world",
        format!(
            "There is no world '{world}' in this session; it has {}.",
            all.join(", ")
        ),
        detail([
            ("path", json!(path)),
            ("world", json!(world)),
            ("worlds", json!(all)),
        ]),
    )
}

fn limit_reached(what: &str, limit: u64, value: u64) -> Problem {
    Problem::new(
        "session.limit_reached",
        format!("This session already has {value} {what}; the limit is {limit}."),
        detail([
            ("limit", json!(limit)),
            ("value", json!(value)),
            ("what", json!(what)),
        ]),
    )
}

/// A branch's answer names its world: `world` in an object, a first line `world <id>` before a
/// text projection (mcp.md 4.1).
fn named(world: &str, v: Value) -> Value {
    match v {
        Value::Object(mut m) => {
            m.insert("world".into(), json!(world));
            Value::Object(m)
        }
        Value::String(text) => Value::String(format!("world {world}\n{text}")),
        other => other,
    }
}

impl Branches {
    pub fn new(alive: Alive) -> Branches {
        Branches {
            made: 0,
            live: BTreeMap::new(),
            spent: BTreeMap::new(),
            alive,
        }
    }

    /// The branches `source` holds, in order of creation.
    fn worlds_of(&self, source: Source) -> Vec<String> {
        let mut out: Vec<(u64, String)> = self
            .live
            .iter()
            .filter(|(_, b)| b.owner == source)
            .map(|(w, _)| (w[1..].parse().unwrap_or(0), w.clone()))
            .collect();
        out.sort();
        out.into_iter().map(|(_, w)| w).collect()
    }

    fn owned(&mut self, source: Source, path: &str, world: &str) -> Result<&mut Branch, Problem> {
        let mine = self.live.get(world).is_some_and(|b| b.owner == source);
        if !mine {
            return Err(unknown_world(path, world, self.worlds_of(source)));
        }
        self.live
            .get_mut(world)
            .ok_or_else(|| unknown_world(path, world, Vec::new()))
    }

    /// The per-decision count of `source`, started again when main has moved.
    fn spent(&mut self, source: Source, main_tick: u64) -> &mut Spent {
        let s = self.spent.entry(source).or_default();
        if s.at != main_tick {
            s.at = main_tick;
            s.ticks = 0;
        }
        s
    }

    /// `player.fork`: a new branch of main or of one of the caller's branches.
    pub fn fork(&mut self, main: &Game, cmd: &Command) -> Result<Value, Problem> {
        let p: ForkParams = crate::decode(&cmd.params, "player.fork")?;
        // Branches whose source went away end now (mcp.md 5.5: with their session).
        let alive = self.alive.clone();
        self.live.retain(|_, b| alive(b.owner));
        let parent = p.world.unwrap_or_else(|| "main".into());
        let held = self.worlds_of(cmd.source).len();
        if held >= PER_SOURCE {
            return Err(limit_reached("branches", PER_SOURCE as u64, held as u64));
        }
        if self.live.len() >= LIVE {
            return Err(limit_reached(
                "branches on this runtime",
                LIVE as u64,
                self.live.len() as u64,
            ));
        }
        let game = if parent == "main" {
            main.fork_branch()?
        } else {
            self.owned(cmd.source, "/world", &parent)?
                .game
                .fork_branch()?
        };
        self.made += 1;
        let world = format!("b{}", self.made);
        let tick = game.tick().0;
        let label = p
            .label
            .map(|l| l.trim().chars().take(LABEL_MAX).collect::<String>())
            .filter(|l| !l.is_empty());
        let mut out = json!({"tick": tick, "world": world, "parent": parent});
        if let Some(l) = &label {
            out["label"] = json!(l);
        }
        if !matches!(cmd.source, Source::Player(_)) {
            out["world_hash"] = json!(game.world_hash()?.to_string());
        }
        self.spent(cmd.source, main.tick().0).forks += 1;
        self.live.insert(
            world,
            Branch {
                game,
                owner: cmd.source,
                parent,
                forked_at: tick,
                label,
            },
        );
        Ok(out)
    }

    /// `player.discard`: drops one of the caller's branches (its own branches forked from it live
    /// on: each is a world of its own).
    pub fn discard(&mut self, main: &Game, cmd: &Command) -> Result<Value, Problem> {
        let p: DiscardParams = crate::decode(&cmd.params, "player.discard")?;
        if p.branch == "main" {
            return Err(Problem::new(
                "session.cannot_discard_main",
                "Main cannot be discarded; only branches can.",
                detail([]),
            ));
        }
        self.owned(cmd.source, "/branch", &p.branch)?;
        self.live.remove(&p.branch);
        Ok(json!({"tick": main.tick().0, "world": "main", "discarded": p.branch}))
    }

    /// `player.branches`: the caller's branches, its limits and what it has spent.
    pub fn list(&mut self, main: &Game, cmd: &Command) -> Result<Value, Problem> {
        crate::decode::<crate::catalog::NoParams>(&cmd.params, "player.branches")?;
        let main_tick = main.tick().0;
        let rows: Vec<Value> = self
            .worlds_of(cmd.source)
            .into_iter()
            .filter_map(|w| {
                let b = self.live.get(&w)?;
                let mut row = json!({"world": w, "parent": b.parent,
                                     "forked_at": b.forked_at, "tick": b.game.tick().0});
                if let Some(l) = &b.label {
                    row["label"] = json!(l);
                }
                Some(row)
            })
            .collect();
        let s = self.spent(cmd.source, main_tick);
        Ok(json!({
            "tick": main_tick,
            "world": "main",
            "branches": rows,
            "limits": {"branches": PER_SOURCE, "branch_ticks_per_decision": PER_DECISION},
            "left": {"branch_ticks_per_decision": PER_DECISION - s.ticks.min(PER_DECISION)},
            "stats": {"forks": s.forks, "branch_ticks": s.branch_ticks,
                      "branch_acts": s.branch_acts},
        }))
    }

    /// A `player.*` command for the branch `world`, which the caller holds. A `wait` runs at most
    /// what is left of the caller's ticks for main's tick (all that is left when it names no
    /// `ticks`; more is refused before anything runs); `may_run` is false while main runs in real
    /// time, when no branch tick may hold it.
    pub fn call(
        &mut self,
        main_tick: u64,
        world: &str,
        mut cmd: Command,
        may_run: bool,
    ) -> Result<Value, Problem> {
        let source = cmd.source;
        self.owned(source, "/world", world)?;
        match cmd.name.as_str() {
            "player.pacing" => {
                return Err(Problem::new(
                    "request.invalid_value",
                    "A branch always runs stepped; player.pacing sets main's pacing alone.",
                    detail([("path", json!("/world")), ("world", json!(world))]),
                ));
            }
            "player.continue" => {}
            "player.wait" => {
                if !may_run {
                    return Err(Problem::new(
                        "time.wrong_mode",
                        "Main runs in real time: a branch steps while main is stepped, paused or \
                         held for a seat's decision.",
                        detail([("world", json!(world)), ("pacing", json!("real_time"))]),
                    ));
                }
                let left = PER_DECISION - self.spent(source, main_tick).ticks.min(PER_DECISION);
                let asked = cmd.params.get("ticks").and_then(Value::as_u64);
                match asked {
                    Some(t) if t > left => {
                        let mut p =
                            limit_reached("branch_ticks", PER_DECISION, PER_DECISION - left);
                        p.detail.insert("asked".into(), json!(t));
                        p.detail.insert("left".into(), json!(left));
                        return Err(p);
                    }
                    Some(_) => {}
                    None if left == 0 => {
                        return Err(limit_reached("branch_ticks", PER_DECISION, PER_DECISION));
                    }
                    None => {
                        if let Value::Object(m) = &mut cmd.params {
                            m.insert("ticks".into(), json!(left));
                        } else if cmd.params.is_null() {
                            cmd.params =
                                Value::Object(Map::from_iter([("ticks".to_owned(), json!(left))]));
                        }
                    }
                }
            }
            _ => {}
        }
        let branch = self.owned(source, "/world", world)?;
        let before = branch.game.tick().0;
        let r = branch.game.apply(&cmd);
        let ran = branch.game.tick().0.saturating_sub(before);
        let s = self.spent(source, main_tick);
        s.ticks += ran;
        s.branch_ticks += ran;
        if cmd.name == "player.act" && r.is_ok() {
            s.branch_acts += 1;
        }
        r.map(|v| named(world, v))
    }
}
