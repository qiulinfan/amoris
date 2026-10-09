//! The players' session against what a developer does to the world (docs/spec/player.md 6;
//! shared/contract/time.md, Halts): a player's calls that are a developer's are refused on the
//! game thread too; a rule that throws halts the players' time until a hot update lands or a
//! developer resumes; a restore rebases every seat's decision state on the restored world, so
//! the next wait stops at the restored timeline's decision points and carries its events.
#![cfg(all(feature = "thread", feature = "transpile"))]
// The tests' own clock stands outside every tick, as a presenter's does.
#![allow(clippy::disallowed_methods)]
mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use pocket_link::Source;
use pocket_runtime::thread::{Clock, GameHandle, GameThread, ThreadOptions};
use pocket_runtime::{Command, Game, GameBuilder, GameSetup, Project};
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

fn spawn_with(
    make: impl FnOnce() -> Result<Game, pocket_contract::Problem> + Send + 'static,
) -> GameHandle {
    GameThread::spawn(make, ThreadOptions::new(clock())).unwrap_or_else(|e| panic!("{e:#?}"))
}

/// The line the planted failure adds at the start of the `rounding` rule.
const PLANTED: &str = "if (ctx.tick === 3) throw new Error(\"planted at tick 3\");";

/// A copy of the sailing course under the target directory whose `rounding` rule throws at tick
/// 3.
fn course_that_throws(name: &str) -> PathBuf {
    let from = common::repo().join("samples/sailing-course");
    let to = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&to);
    std::fs::create_dir_all(to.join("scripts")).unwrap();
    for f in [
        "project.toml",
        "scene.json",
        "perception.json",
        "tsconfig.json",
    ] {
        std::fs::copy(from.join(f), to.join(f)).unwrap();
    }
    for e in std::fs::read_dir(from.join("scripts")).unwrap() {
        let e = e.unwrap();
        std::fs::copy(e.path(), to.join("scripts").join(e.file_name())).unwrap();
    }
    let rules = to.join("scripts/rules.ts");
    let text = std::fs::read_to_string(&rules).unwrap();
    let at = "    run(ctx, { boats, courses, marks }) {\n";
    assert!(text.contains(at), "the rounding rule moved");
    std::fs::write(
        &rules,
        text.replacen(at, &format!("{at}        {PLANTED}\n"), 1),
    )
    .unwrap();
    to
}

/// The planted failure taken out again: the fix a hot update lands.
fn fix(dir: &Path) {
    let rules = dir.join("scripts/rules.ts");
    let text = std::fs::read_to_string(&rules).unwrap();
    std::fs::write(&rules, text.replacen(PLANTED, "", 1)).unwrap();
}

fn game_in(dir: &Path) -> Result<Game, pocket_contract::Problem> {
    let setup = Project::load(dir).and_then(|p| p.setup(false))?;
    GameBuilder::new(Arc::new(setup)).project(dir).build()
}

/// On the game thread a player's developer calls are refused too, the ones the loop answers itself
/// among them (time, Play, the kept snapshots, the status), and the world does not move.
#[test]
fn the_thread_refuses_a_players_developer_calls() {
    let h = spawn_with(|| Game::new(course(), 1));
    let mut player = h.client(Source::Player(0)).unwrap();
    let mut dev = h.developer();
    for (name, params) in [
        ("time.step", json!({"ticks": 5})),
        ("step", json!({"ticks": 5})),
        ("time.control", json!({"pause": false})),
        ("status", json!({})),
        ("snapshot", json!({})),
        ("snapshots.list", json!({})),
        ("snapshots.restore", json!({"tick": 0})),
        ("play.start", json!({})),
        ("world.get", json!({"entity": "Crate4"})),
        ("world.query", json!({"with": ["Perceivable"]})),
    ] {
        let e = player.call(name, params).unwrap_err();
        assert_eq!(e.code, "permission.denied", "{name}: {e:#?}");
    }
    let s = dev.call("status", json!({})).unwrap().into_json();
    assert_eq!(s["tick"], json!(0), "{s:#}");
    assert_eq!(s["mode"], json!("edit"), "{s:#}");
    drop((player, dev));
    h.shutdown(2000).unwrap();
}

/// time.md, Halts, on a game driven directly: a rule that throws stops a player's wait at the tick
/// it failed in with `stopped: "halted"` and the generic `time.halted`; later waits run nothing;
/// a developer's wait gets the script's own problem; the hot update that removes the throw ends
/// the halt, and time runs again.
#[test]
fn a_rule_that_throws_halts_the_players_until_a_hot_update() {
    common::big_stack(|| {
        let dir = course_that_throws("halt-course-direct");
        let mut g = game_in(&dir).unwrap_or_else(|e| panic!("{e:#?}"));
        let mut seq = 0;
        let mut call = |g: &mut Game, source: Source, name: &str, params: Value| {
            seq += 1;
            g.apply(&Command::new(source, seq, name, params))
                .unwrap_or_else(|e| panic!("{name}: {e:#?}"))
        };
        let p = Source::Player(0);
        let wait = json!({"ticks": 10, "until": {"event": "crate.taken"}});
        let w = call(&mut g, p, "player.wait", wait.clone());
        assert_eq!(w["stopped"], json!("halted"), "{w:#}");
        assert_eq!(w["tick"], json!(3), "{w:#}");
        assert_eq!(w["halted"]["code"], json!("time.halted"), "{w:#}");
        let s = call(&mut g, p, "player.session", json!({}));
        assert_eq!(s["status"]["halted"], json!(true), "{s:#}");
        let w = call(&mut g, p, "player.wait", wait.clone());
        assert_eq!(
            (w["stopped"].clone(), w["ran"].clone()),
            (json!("halted"), json!(0)),
            "{w:#}"
        );
        // A developer sees the failure itself: the rule, and the script's exception as its cause.
        let d = call(
            &mut g,
            Source::Developer(0),
            "player.wait",
            json!({"seat": "skipper", "ticks": 1}),
        );
        assert_eq!(d["halted"]["code"], json!("sim.system_failed"), "{d:#}");
        assert_eq!(
            d["halted"]["detail"]["cause"]["code"],
            json!("script.exception"),
            "{d:#}"
        );
        // The fix lands as a hot update; the halt ends and the wait runs its ticks.
        fix(&dir);
        let a = call(&mut g, Source::Developer(0), "scripts.apply", json!({}));
        assert_eq!(a["outcome"], json!("applied"), "{a:#}");
        let w = call(&mut g, p, "player.wait", wait);
        assert_eq!(w["stopped"], json!("ticks"), "{w:#}");
        assert_eq!(w["ran"], json!(10), "{w:#}");
        let s = call(&mut g, p, "player.session", json!({}));
        assert_eq!(s["status"]["halted"], json!(false), "{s:#}");
    });
}

/// time.md, Halts, on the game thread: the halt holds a player's stepped wait, and a developer's
/// `time.control {pause: false}` (the runtime's `resume`) ends it. A restore ends it too (mcp.md
/// 7.1): the restored world runs until the rule fails again, at the same tick.
#[test]
fn a_developers_resume_ends_the_halt_on_the_thread() {
    let dir = course_that_throws("halt-course-thread");
    let d = dir.clone();
    let h = spawn_with(move || game_in(&d));
    let mut player = h.client(Source::Player(0)).unwrap();
    let mut dev = h.developer();
    let wait = json!({"ticks": 10, "until": {"event": "crate.taken"}});
    let w = player
        .call("player.wait", wait.clone())
        .unwrap()
        .into_json();
    assert_eq!(w["stopped"], json!("halted"), "{w:#}");
    assert_eq!(w["tick"], json!(3), "{w:#}");
    let w = player
        .call("player.wait", wait.clone())
        .unwrap()
        .into_json();
    assert_eq!(w["ran"], json!(0), "still halted: {w:#}");
    dev.call("time.control", json!({"pause": false})).unwrap();
    // The rule still throws, but only at tick 3: time runs on.
    let w = player
        .call("player.wait", wait.clone())
        .unwrap()
        .into_json();
    assert_eq!(w["stopped"], json!("ticks"), "{w:#}");
    assert_eq!(w["tick"], json!(13), "{w:#}");
    let w = player
        .call("player.wait", wait.clone())
        .unwrap()
        .into_json();
    assert_eq!(w["tick"], json!(23), "{w:#}");
    // Back to tick 0: not halted, and the rule halts the world at tick 3 again.
    dev.call("snapshots.restore", json!({"tick": 0})).unwrap();
    let s = player
        .call("player.session", json!({}))
        .unwrap()
        .into_json();
    assert_eq!(s["status"]["halted"], json!(false), "{s:#}");
    let w = player.call("player.wait", wait).unwrap().into_json();
    assert_eq!(
        (w["stopped"].clone(), w["tick"].clone()),
        (json!("halted"), json!(3)),
        "{w:#}"
    );
    drop((player, dev));
    h.shutdown(2000).unwrap();
}

/// The kinds and ticks of a time answer's events delta.
fn events(w: &Value) -> Vec<(String, u64)> {
    w["events"]
        .as_array()
        .unwrap_or_else(|| panic!("no events: {w:#}"))
        .iter()
        .map(|e| {
            (
                e["kind"].as_str().unwrap_or("").to_owned(),
                e["tick"].as_u64().unwrap_or(u64::MAX),
            )
        })
        .collect()
}

/// A restore replaces the world under the players' session; every seat's decision state follows
/// it. Restored to tick 0, the seat has the episode's `start` point again and its next wait stops
/// at the sightings of tick 1, carrying them; restored to a later tick, the next wait carries only
/// events after it.
#[test]
fn a_restore_rebases_the_players_session() {
    let h = spawn_with(|| Game::new(course(), 1));
    let mut player = h.client(Source::Player(0)).unwrap();
    let mut dev = h.developer();
    let act = json!({"actions": [{"do": "start", "intent": "sail_to", "target": "Mark1",
                                  "params": {"arrive_m": 8}}]});
    player.call("player.act", act.clone()).unwrap();
    let first = player.call("player.wait", json!({})).unwrap().into_json();
    assert_eq!(
        (first["stopped"].clone(), first["tick"].clone()),
        (json!("decision"), json!(1)),
        "{first:#}"
    );
    let sightings = events(&first);
    assert!(sightings.iter().any(|(k, _)| k == "sighted"), "{first:#}");
    let far = player
        .call("player.wait", json!({"until": {"event": "mark.rounded"}}))
        .unwrap()
        .into_json();
    let far_tick = far["tick"].as_u64().unwrap();
    assert!(far_tick > 600, "{far:#}");
    let pushed = player
        .call("player.session", json!({}))
        .unwrap()
        .into_json();
    assert!(pushed["cursor"].as_u64().unwrap() > 3, "{pushed:#}");

    // Back to tick 0: the start point again, and the first wait as on a fresh world.
    let r = dev
        .call("snapshots.restore", json!({"tick": 0}))
        .unwrap()
        .into_json();
    assert_eq!(r["restored"], json!(0), "{r:#}");
    let s = player
        .call("player.session", json!({}))
        .unwrap()
        .into_json();
    assert_eq!(
        s["decision"]["reasons"][0]["reason"],
        json!("start"),
        "{s:#}"
    );
    player.call("player.act", act).unwrap();
    let again = player.call("player.wait", json!({})).unwrap().into_json();
    assert_eq!(
        (again["stopped"].clone(), again["tick"].clone()),
        (json!("decision"), json!(1)),
        "{again:#}"
    );
    assert_eq!(events(&again), sightings, "{again:#}");

    // Run past tick 600, then back to it: no decision pending, and the next wait carries only
    // events after the restored tick.
    let w = player
        .call("player.wait", json!({"until": {"event": "mark.rounded"}}))
        .unwrap()
        .into_json();
    assert!(w["tick"].as_u64().unwrap() > 600, "{w:#}");
    let r = dev
        .call("snapshots.restore", json!({"tick": 600}))
        .unwrap()
        .into_json();
    let at = r["restored"].as_u64().unwrap();
    assert!(at <= 600 && at > 0, "{r:#}");
    let s = player
        .call("player.session", json!({}))
        .unwrap()
        .into_json();
    assert!(s["decision"].is_null(), "{s:#}");
    let w = player
        .call("player.wait", json!({"until": {"event": "mark.rounded"}}))
        .unwrap()
        .into_json();
    assert_eq!(w["stopped"], json!("until"), "{w:#}");
    let after = events(&w);
    assert!(!after.is_empty(), "{w:#}");
    assert!(after.iter().all(|(_, t)| *t > at), "{at}: {w:#}");
    drop((player, dev));
    h.shutdown(2000).unwrap();
}
