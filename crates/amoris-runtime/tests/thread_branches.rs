//! Branches for an agent's lookahead on the game thread (docs/spec/player.md 7.1;
//! shared/contract/mcp.md 5.5): `player.fork` gives the caller a branch of main that only it
//! reaches, every other player command acts on it by `world`, and nothing it does reaches main; a
//! branch run from the same state with the same calls is main's run, hash for hash; the bounds
//! (branches per source, branch ticks while main stays at one tick) are refused up front.
#![cfg(all(feature = "thread", feature = "transpile"))]
#![allow(clippy::disallowed_methods)]
mod common;

use std::sync::{Arc, OnceLock};
use std::time::Instant;

use amoris_link::{GameClient, Source};
use amoris_runtime::branches::{PER_DECISION, PER_SOURCE};
use amoris_runtime::thread::{Clock, GameHandle, GameThread, ThreadOptions};
use amoris_runtime::{Game, GameSetup, Project};
use serde_json::{Value, json};

fn course() -> Arc<GameSetup> {
    static SETUP: OnceLock<Arc<GameSetup>> = OnceLock::new();
    SETUP
        .get_or_init(|| {
            let p = Project::load(&common::repo().join("samples/sailing-course"))
                .unwrap_or_else(|e| panic!("{e:#?}"));
            Arc::new(p.setup(false).unwrap_or_else(|e| panic!("{e:#?}")))
        })
        .clone()
}

fn clock() -> Clock {
    let start = Instant::now();
    Arc::new(move || start.elapsed().as_secs_f64() * 1000.0)
}

fn spawn() -> GameHandle {
    GameThread::spawn(|| Game::new(course(), 1), ThreadOptions::new(clock()))
        .unwrap_or_else(|e| panic!("{e:#?}"))
}

fn ok(c: &mut GameClient, name: &str, params: Value) -> Value {
    c.call(name, params)
        .unwrap_or_else(|e| panic!("{name}: {e:#?}"))
        .into_json()
}

fn refused(c: &mut GameClient, name: &str, params: Value) -> amoris_contract::Problem {
    match c.call(name, params) {
        Ok(v) => panic!("{name} was answered: {:#}", v.into_json()),
        Err(p) => p,
    }
}

/// Main's tick and hash as a developer reads them.
fn main_state(dev: &mut GameClient) -> (Value, Value) {
    let s = ok(dev, "status", json!({}));
    (s["tick"].clone(), s["world_hash"].clone())
}

fn sail(player: &mut GameClient, world: Option<&str>) {
    let mut act = json!({"actions": [{"do": "start", "intent": "sail_to", "target": "Mark1",
                                      "params": {"arrive_m": 8}}]});
    if let Some(w) = world {
        act["world"] = json!(w);
    }
    ok(player, "player.act", act);
}

#[test]
fn a_branch_looks_ahead_as_the_seat_and_never_moves_main() {
    let h = spawn();
    let mut player = h.client(Source::Player(0)).unwrap();
    let mut dev = h.developer();
    sail(&mut player, None);
    let w = ok(&mut player, "player.wait", json!({}));
    assert_eq!(w["tick"], json!(1), "{w:#}");
    let before = main_state(&mut dev);

    let f = ok(&mut player, "player.fork", json!({"label": "mark first"}));
    assert_eq!(f["world"], json!("b1"), "{f:#}");
    assert_eq!(f["parent"], json!("main"), "{f:#}");
    assert_eq!(f["tick"], json!(1), "{f:#}");
    assert!(
        f.get("world_hash").is_none(),
        "a player sees no hash: {f:#}"
    );

    // The branch is played through the seat's own tools, by world.
    let o = ok(
        &mut player,
        "player.observe",
        json!({"world": "b1", "projection": "text"}),
    );
    let text = o.as_str().unwrap_or_else(|| panic!("{o:#}"));
    assert!(text.starts_with("world b1\n"), "{text}");
    sail(&mut player, Some("b1"));
    let w = ok(
        &mut player,
        "player.wait",
        json!({"world": "b1", "ticks": 300}),
    );
    assert_eq!(w["world"], json!("b1"), "{w:#}");
    assert!(w["tick"].as_u64().unwrap() > 1, "{w:#}");
    assert!(w.get("world_hash").is_none(), "{w:#}");
    // Main did not move, and its hash is what it was.
    assert_eq!(main_state(&mut dev), before);

    // Only the source that made a branch reaches it.
    let e = refused(
        &mut dev,
        "player.observe",
        json!({"world": "b1", "seat": "skipper"}),
    );
    assert_eq!(e.code, "session.unknown_world", "{e:#?}");
    assert_eq!(e.detail["worlds"], json!(["main"]), "{e:#?}");
    let e = refused(&mut player, "player.discard", json!({"branch": "main"}));
    assert_eq!(e.code, "session.cannot_discard_main", "{e:#?}");

    let l = ok(&mut player, "player.branches", json!({}));
    assert_eq!(l["branches"][0]["world"], json!("b1"), "{l:#}");
    assert_eq!(l["branches"][0]["label"], json!("mark first"), "{l:#}");
    assert_eq!(l["stats"]["forks"], json!(1), "{l:#}");
    assert_eq!(l["stats"]["branch_acts"], json!(1), "{l:#}");
    let d = ok(&mut player, "player.discard", json!({"branch": "b1"}));
    assert_eq!(d["discarded"], json!("b1"), "{d:#}");
    let e = refused(&mut player, "player.wait", json!({"world": "b1"}));
    assert_eq!(e.code, "session.unknown_world", "{e:#?}");
    drop((player, dev));
    h.shutdown(2000).unwrap();
}

#[test]
fn a_branch_runs_what_main_would_run_hash_for_hash() {
    let h = spawn();
    let mut player = h.client(Source::Player(0)).unwrap();
    let mut dev = h.developer();
    sail(&mut player, None);
    let (_, hash) = main_state(&mut dev);
    assert!(hash.is_string());
    let f = ok(&mut dev, "player.fork", json!({}));
    assert_eq!(f["world_hash"], hash, "a fork is its parent's state: {f:#}");
    // A branch of the branch, too.
    let g = ok(&mut dev, "player.fork", json!({"world": f["world"]}));
    assert_eq!(g["parent"], f["world"], "{g:#}");
    assert_eq!(g["world_hash"], hash, "{g:#}");
    let world = g["world"].clone();
    let run = json!({"seat": "skipper", "ticks": 240, "until": {"event": "mark.rounded"}});
    let mut on_branch = run.clone();
    on_branch["world"] = world.clone();
    let b = ok(&mut dev, "player.wait", on_branch);
    let m = ok(&mut dev, "player.wait", run);
    assert_eq!(b["world"], world, "{b:#}");
    assert!(b["tick"].as_u64().unwrap() > 1, "{b:#}");
    assert!(m["world_hash"].is_string(), "{m:#}");
    assert_ne!(m["world_hash"], hash, "main moved: {m:#}");
    assert_eq!(b["tick"], m["tick"], "{b:#}\n{m:#}");
    assert_eq!(b["world_hash"], m["world_hash"], "{b:#}\n{m:#}");
    assert!(
        m.get("world").is_none(),
        "main's answer is unchanged: {m:#}"
    );
    drop((player, dev));
    h.shutdown(2000).unwrap();
}

#[test]
fn the_bounds_are_refused_up_front() {
    let h = spawn();
    let mut player = h.client(Source::Player(0)).unwrap();
    sail(&mut player, None);
    for n in 1..=PER_SOURCE {
        let f = ok(&mut player, "player.fork", json!({}));
        assert_eq!(f["world"], json!(format!("b{n}")), "{f:#}");
    }
    let e = refused(&mut player, "player.fork", json!({}));
    assert_eq!(e.code, "session.limit_reached", "{e:#?}");
    assert_eq!(e.detail["limit"], json!(PER_SOURCE), "{e:#?}");

    // More branch ticks than are left for main's tick: refused before anything runs.
    let e = refused(
        &mut player,
        "player.wait",
        json!({"world": "b1", "ticks": PER_DECISION + 1}),
    );
    assert_eq!(e.code, "session.limit_reached", "{e:#?}");
    assert_eq!(e.detail["what"], json!("branch_ticks"), "{e:#?}");
    let l = ok(&mut player, "player.branches", json!({}));
    assert_eq!(
        l["left"]["branch_ticks_per_decision"],
        json!(PER_DECISION),
        "{l:#}"
    );
    // A wait without ticks runs at most what is left; the ticks of every branch count.
    let mut worlds = ["b1", "b2", "b3", "b4"].iter().cycle();
    let mut spent = 0;
    for _ in 0..200 {
        let w = worlds.next().unwrap();
        let before = ok(&mut player, "player.branches", json!({}));
        let left = before["left"]["branch_ticks_per_decision"]
            .as_u64()
            .unwrap();
        if left == 0 {
            break;
        }
        let a = ok(&mut player, "player.wait", json!({"world": w}));
        assert_eq!(a["world"], json!(w), "{a:#}");
        spent = PER_DECISION
            - ok(&mut player, "player.branches", json!({}))["left"]["branch_ticks_per_decision"]
                .as_u64()
                .unwrap();
    }
    assert_eq!(spent, PER_DECISION);
    let e = refused(&mut player, "player.wait", json!({"world": "b2"}));
    assert_eq!(e.code, "session.limit_reached", "{e:#?}");
    // Main moves on: the count starts again.
    ok(&mut player, "player.wait", json!({"ticks": 1}));
    let l = ok(&mut player, "player.branches", json!({}));
    assert_eq!(
        l["left"]["branch_ticks_per_decision"],
        json!(PER_DECISION),
        "{l:#}"
    );
    ok(
        &mut player,
        "player.wait",
        json!({"world": "b2", "ticks": 10}),
    );
    drop(player);
    h.shutdown(2000).unwrap();
}

#[test]
fn no_branch_tick_holds_main_running_in_real_time() {
    let h = spawn();
    let mut player = h.client(Source::Player(0)).unwrap();
    let mut dev = h.developer();
    ok(
        &mut dev,
        "player.pacing",
        json!({"pacing": {"pacing": "real_time", "speed": 1}}),
    );
    let f = ok(&mut player, "player.fork", json!({}));
    let e = refused(
        &mut player,
        "player.wait",
        json!({"world": f["world"], "ticks": 10}),
    );
    assert_eq!(e.code, "time.wrong_mode", "{e:#?}");
    // A branch's reads are answered all the same, and it never runs real time itself.
    ok(&mut player, "player.session", json!({"world": f["world"]}));
    let e = refused(
        &mut dev,
        "player.pacing",
        json!({"world": "b1", "pacing": {"pacing": "stepped"}}),
    );
    assert_eq!(
        e.code, "session.unknown_world",
        "the developer holds none: {e:#?}"
    );
    drop((player, dev));
    h.shutdown(2000).unwrap();
}

/// The branches of a source that has no client any more end at the next fork.
#[test]
fn a_gone_sources_branches_end() {
    let h = spawn();
    let mut a = h.developer();
    ok(&mut a, "player.fork", json!({}));
    ok(&mut a, "player.fork", json!({}));
    drop(a);
    let mut b = h.developer();
    let f = ok(&mut b, "player.fork", json!({}));
    assert_eq!(f["world"], json!("b3"), "ids are never reused: {f:#}");
    let l = ok(&mut b, "player.branches", json!({}));
    assert_eq!(l["branches"].as_array().map(Vec::len), Some(1), "{l:#}");
    drop(b);
    h.shutdown(2000).unwrap();
}
