//! The player tools on the game thread (docs/spec/player.md; shared/contract/time.md, Threads and
//! the web): a player's stepped `wait` runs one tick per boundary while the queue is served, and
//! in real time with pause-on-decision the world holds at each decision point until the seat
//! answers it (`continue`, or `act` with `resume`), then runs on to the next one.
#![cfg(all(feature = "thread", feature = "transpile"))]
// The tests' own clock and producer threads stand outside every tick, as a presenter's do.
#![allow(clippy::disallowed_methods)]
mod common;

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use pocket_link::Source;
use pocket_runtime::thread::{Clock, GameHandle, GameThread, ThreadOptions};
use pocket_runtime::{Game, GameSetup, Project};
use serde_json::json;

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

#[test]
fn a_stepped_wait_runs_to_the_next_decision_while_reads_are_served() {
    let h = spawn();
    let mut player = h.client(Source::Player(0)).unwrap();
    let mut dev = h.developer();
    let act = json!({"actions": [{"do": "start", "intent": "sail_to", "target": "Mark1",
                                  "params": {"arrive_m": 8}}]});
    player.call("player.act", act).unwrap();
    // The start decision is answered by moving time on; the sightings of tick 1 are the next.
    let w = player.call("player.wait", json!({})).unwrap().into_json();
    assert_eq!(w["stopped"], json!("decision"), "{w:#}");
    assert_eq!(w["tick"], json!(1), "{w:#}");
    assert!(
        w.get("world_hash").is_none(),
        "a player sees no hash: {w:#}"
    );
    // A long wait sends its answer later; meanwhile a developer's read is answered.
    let (tx, rx) = std::sync::mpsc::channel();
    player
        .send(
            "player.wait",
            json!({"until": {"event": "mark.rounded"}}),
            None,
            move |r| {
                let _ = tx.send(r);
            },
        )
        .unwrap();
    let s = dev.call("status", json!({})).unwrap().into_json();
    assert!(s["tick"].as_u64().is_some(), "{s:#}");
    let w = rx
        .recv_timeout(Duration::from_secs(60))
        .unwrap()
        .unwrap()
        .into_json();
    assert_eq!(w["stopped"], json!("until"), "{w:#}");
    assert_eq!(w["until"]["event"]["kind"], json!("mark.rounded"), "{w:#}");
    assert!(w["tick"].as_u64().unwrap() > 1000, "{w:#}");
    drop((player, dev));
    h.shutdown(2000).unwrap();
}

#[test]
fn real_time_pauses_on_each_decision_until_the_seat_answers() {
    let h = spawn();
    let mut player = h.client(Source::Player(0)).unwrap();
    let mut dev = h.developer();
    // A player cannot choose the pacing.
    let e = player
        .call("player.pacing", json!({"pacing": {"pacing": "stepped"}}))
        .unwrap_err();
    assert_eq!(e.code, "permission.denied", "{e:#?}");
    dev.call(
        "player.pacing",
        json!({"pacing": {"pacing": "real_time", "speed": 20, "pause_on_decision": true}}),
    )
    .unwrap();
    // The episode's start is a decision point: the world holds at tick 0 until it is answered.
    std::thread::sleep(Duration::from_millis(150));
    let s = player
        .call("player.session", json!({}))
        .unwrap()
        .into_json();
    assert_eq!(s["tick"], json!(0), "{s:#}");
    assert_eq!(s["status"]["paused_for"], json!(["skipper"]), "{s:#}");
    let w = player.call("player.wait", json!({})).unwrap().into_json();
    assert_eq!(w["stopped"], json!("decision"), "pending already: {w:#}");
    assert_eq!(w["ran"], json!(0), "{w:#}");
    // Acting with resume answers it; the world runs to the sightings of tick 1 and holds again.
    let act = json!({"actions": [{"do": "start", "intent": "sail_to", "target": "Mark1"}],
                     "resume": true});
    player.call("player.act", act).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let held = player
        .call("player.session", json!({}))
        .unwrap()
        .into_json();
    let at = held["tick"].as_u64().unwrap();
    assert_eq!(at, 1, "held at the sightings: {held:#}");
    assert_eq!(held["decision"]["tick"], json!(1), "{held:#}");
    std::thread::sleep(Duration::from_millis(200));
    let still = player
        .call("player.session", json!({}))
        .unwrap()
        .into_json();
    assert_eq!(
        still["tick"],
        json!(at),
        "no tick while the decision is pending"
    );
    // continue answers it; the long poll answers at the next decision, ticks later.
    let c = player
        .call("player.continue", json!({}))
        .unwrap()
        .into_json();
    assert_eq!(c["stopped"], json!("answered"), "{c:#}");
    let w = player
        .call("player.wait", json!({"max_wall_ms": 20000}))
        .unwrap()
        .into_json();
    assert_eq!(w["stopped"], json!("decision"), "{w:#}");
    assert!(w["tick"].as_u64().unwrap() > at, "{w:#}");
    // A wait with a short wall limit while the world holds answers at the limit.
    let t0 = Instant::now();
    let w = player
        .call(
            "player.wait",
            json!({"until": {"event": "course.finished"}, "max_wall_ms": 100}),
        )
        .unwrap()
        .into_json();
    assert_eq!(w["stopped"], json!("wall_limit"), "{w:#}");
    assert!(t0.elapsed() < Duration::from_secs(5));
    drop((player, dev));
    h.shutdown(2000).unwrap();
}

/// The reference skipper plays the course through a player client of the game thread to its end;
/// then a player's wait is refused with `time.episode_over`, and a developer's `time.step` still
/// runs (the episode's end holds the players' time, not a developer's).
#[test]
fn the_skipper_finishes_on_the_thread_and_a_developer_still_steps() {
    let h = spawn();
    let mut player = h.client(Source::Player(0)).unwrap();
    let mut dev = h.developer();
    let mut skipper = pocket_runtime::skipper::Skipper::new();
    let mut done = false;
    for _ in 0..100 {
        let obs = player
            .call("player.observe", json!({"budget_tokens": 1500}))
            .unwrap()
            .into_json();
        if let Some(act) = skipper.decide(&obs) {
            player.call("player.act", act).unwrap();
        }
        let w = player.call("player.wait", json!({})).unwrap().into_json();
        if w["stopped"] == json!("done") {
            done = true;
            break;
        }
    }
    assert!(done, "the course was not finished");
    let e = player.call("player.wait", json!({})).unwrap_err();
    assert_eq!(e.code, "time.episode_over", "{e:#?}");
    let before = dev.call("status", json!({})).unwrap().into_json()["tick"].clone();
    let s = dev
        .call("time.step", json!({"ticks": 10}))
        .unwrap()
        .into_json();
    assert_eq!(
        s["tick"].as_u64().unwrap(),
        before.as_u64().unwrap() + 10,
        "{s:#}"
    );
    drop((player, dev));
    h.shutdown(2000).unwrap();
}
