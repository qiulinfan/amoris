//! The players on the web (docs/spec/player.md 6; threads.md 7.3): the worker's loop feeds the
//! players' session as the game thread does. A player's stepped `wait` runs its ticks through the
//! loop, one per boundary, so the hash stream, the publications and the queue go on between them;
//! the decision points come after every tick, whatever ran it; and in real time with
//! pause-on-decision the world holds at each decision until the seat answers it, the long poll
//! answering when the decision comes or at its wall limit. A player sends no control of the loop.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use pocket_link::source_json;
use pocket_runtime::{Game, GameSetup, Project};
use pocket_web::Pacing;
use pocket_web::package::Package;
use pocket_web::worker::{Clock, Next, Out, WorkerCore};
use serde_json::{Value, json};

/// Runs `f` on a thread with the stack the script host needs.
fn on_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(pocket_runtime::GAME_STACK_BYTES)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap()
}

/// The sailing course, compiled once per test binary.
fn course() -> &'static GameSetup {
    static SETUP: OnceLock<GameSetup> = OnceLock::new();
    SETUP.get_or_init(|| {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../samples/sailing-course");
        let p = Project::load(&dir).unwrap_or_else(|e| panic!("{e:#?}"));
        p.setup(false).unwrap_or_else(|e| panic!("{e:#?}"))
    })
}

/// The course's game as the worker gets it: through the web package's bytes.
fn packaged() -> Game {
    let bytes = Package::from_setup(course(), 1).to_bytes();
    let setup = Package::from_bytes(&bytes).unwrap().setup().unwrap();
    Game::new(Arc::new(setup), 1).unwrap_or_else(|e| panic!("{e:#?}"))
}

/// A clock the test moves, in milliseconds (an `f64`'s bits).
#[derive(Clone)]
struct Time(Arc<AtomicU64>);

impl Time {
    fn set(&self, ms: f64) {
        self.0.store(ms.to_bits(), Ordering::SeqCst);
    }

    fn get(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::SeqCst))
    }

    fn clock(&self) -> Clock {
        let t = self.clone();
        Arc::new(move || t.get())
    }
}

fn time() -> Time {
    Time(Arc::new(AtomicU64::new(0)))
}

/// The page: replies by source and seq, the hash stream, and how many `snap`s came.
#[derive(Default)]
struct Page {
    replies: Vec<Value>,
    hashes: Vec<u64>,
    snaps: usize,
    seq: u64,
}

impl Page {
    /// Posts a command as `source`, with the next seq; returns the seq.
    fn send(&mut self, w: &mut WorkerCore, source: Value, name: &str, params: Value) -> u64 {
        self.seq += 1;
        w.push(
            &json!({"t": "cmd", "source": source, "seq": self.seq, "name": name,
                       "params": params.to_string()}),
        );
        self.seq
    }

    /// Takes the worker's messages, acknowledging every snapshot at once.
    fn take(&mut self, w: &mut WorkerCore) {
        for o in w.take_out() {
            match o {
                Out::Message(m) if m["t"] == "reply" => self.replies.push(m),
                Out::Message(_) => {}
                Out::Snap(meta, _) => {
                    self.snaps += 1;
                    for h in meta["hashes"].as_array().unwrap() {
                        self.hashes.push(h[0].as_u64().unwrap());
                    }
                    w.ack(meta["version"].as_u64().unwrap());
                }
            }
        }
    }

    /// The reply to `seq`, if it came.
    fn reply(&self, seq: u64) -> Option<&Value> {
        self.replies.iter().find(|r| r["seq"] == json!(seq))
    }

    /// The reply to `seq`'s answer; panics on an error or when none came.
    fn ok(&self, seq: u64) -> Value {
        let r = self
            .reply(seq)
            .unwrap_or_else(|| panic!("no reply to {seq}: {:?}", self.replies));
        assert!(r.get("error").is_none(), "{r:#}");
        r["ok"].clone()
    }

    /// Runs tasks until `seq` is answered (the clock moved by `step_ms` before each), at most
    /// `tasks` of them; returns how many ran.
    fn run_until(
        &mut self,
        w: &mut WorkerCore,
        t: &Time,
        step_ms: f64,
        seq: u64,
        tasks: usize,
    ) -> usize {
        for n in 0..tasks {
            if self.reply(seq).is_some() {
                return n;
            }
            t.set(t.get() + step_ms);
            w.run(8.0);
            self.take(w);
        }
        assert!(
            self.reply(seq).is_some(),
            "{seq} not answered in {tasks} tasks"
        );
        tasks
    }
}

const PLAYER: fn() -> Value = || source_json(pocket_link::Source::Player(0));
const DEV: fn() -> Value = || json!({"developer": 0});

fn sail_to_mark1() -> Value {
    json!({"actions": [{"do": "start", "intent": "sail_to", "target": "Mark1",
                        "params": {"arrive_m": 8}}]})
}

/// A stepped `wait` runs through the loop: its ticks are the worker's own, so every tick's hash is
/// streamed in order, snapshots are posted while it runs, and a developer's read sent meanwhile is
/// answered before the wait is.
#[test]
fn a_stepped_wait_runs_its_ticks_through_the_loop() {
    on_stack(|| {
        let t = time();
        // A clock that moves 1 ms a read, so the task budget ends every few ticks.
        let moving = {
            let t = t.clone();
            Arc::new(move || {
                t.set(t.get() + 1.0);
                t.get()
            }) as Clock
        };
        let mut w = WorkerCore::new(packaged(), Pacing::Stepped, moving).unwrap();
        let mut page = Page::default();
        page.take(&mut w);
        let act = page.send(&mut w, PLAYER(), "player.act", sail_to_mark1());
        let first = page.send(&mut w, PLAYER(), "player.wait", json!({}));
        for _ in 0..50 {
            w.run(8.0);
            page.take(&mut w);
        }
        page.ok(act);
        let first = page.ok(first);
        assert_eq!(
            (first["stopped"].clone(), first["tick"].clone()),
            (json!("decision"), json!(1)),
            "{first:#}"
        );
        let snaps = page.snaps;
        let long = page.send(
            &mut w,
            PLAYER(),
            "player.wait",
            json!({"until": {"event": "mark.rounded"}}),
        );
        w.run(8.0);
        page.take(&mut w);
        assert!(
            page.reply(long).is_none(),
            "the wait ran inline in one task"
        );
        let read = page.send(&mut w, DEV(), "status", json!({}));
        for _ in 0..100_000 {
            if page.reply(long).is_some() {
                break;
            }
            w.run(8.0);
            page.take(&mut w);
        }
        let done = page.ok(long);
        assert_eq!(done["stopped"], json!("until"), "{done:#}");
        let end = done["tick"].as_u64().unwrap();
        assert!(end > 1000, "{done:#}");
        let at = |seq| {
            page.replies
                .iter()
                .position(|r| r["seq"] == json!(seq))
                .unwrap()
        };
        assert!(
            at(read) < at(long),
            "the developer's read waited for the wait"
        );
        assert!(
            page.snaps > snaps + 10,
            "{} snapshots during the wait",
            page.snaps - snaps
        );
        // Every tick's hash, once and in order, through the end of the wait.
        let ticks: Vec<u64> = page.hashes.iter().copied().filter(|&t| t <= end).collect();
        assert_eq!(ticks, (0..=end).collect::<Vec<_>>());
    });
}

/// The players' decision points come after every tick the loop runs, a developer's `step`'s too:
/// the session sees the sightings of tick 1 as its pending decision.
#[test]
fn decision_points_follow_every_tick_the_loop_runs() {
    on_stack(|| {
        let t = time();
        let mut w = WorkerCore::new(packaged(), Pacing::Stepped, t.clock()).unwrap();
        let mut page = Page::default();
        page.take(&mut w);
        let s = page.send(&mut w, PLAYER(), "player.session", json!({}));
        page.run_until(&mut w, &t, 1.0, s, 10);
        let before = page.ok(s);
        let step = page.send(&mut w, DEV(), "step", json!({"ticks": 3}));
        page.run_until(&mut w, &t, 1.0, step, 100);
        assert_eq!(page.ok(step)["tick"], json!(3));
        let s = page.send(&mut w, PLAYER(), "player.session", json!({}));
        page.run_until(&mut w, &t, 1.0, s, 10);
        let after = page.ok(s);
        let points = |v: &Value| v["stats"]["decision_points"].as_u64().unwrap();
        assert!(points(&after) > points(&before), "{before:#}\n{after:#}");
        assert_eq!(after["decision"]["tick"], json!(1), "{after:#}");
    });
}

/// Real time with pause-on-decision on the web: the world holds at the episode's start until the
/// seat answers, runs to the sightings of tick 1 and holds again; `continue` runs it on, and the
/// long poll answers at the next decision or at its wall limit. A player chooses no pacing and
/// sends no control of the loop.
#[test]
fn real_time_pauses_on_each_decision_on_the_web() {
    on_stack(|| {
        let t = time();
        let mut w = WorkerCore::new(packaged(), Pacing::Stepped, t.clock()).unwrap();
        let mut page = Page::default();
        page.take(&mut w);
        let refused = [
            page.send(
                &mut w,
                PLAYER(),
                "player.pacing",
                json!({"pacing": {"pacing": "stepped"}}),
            ),
            page.send(&mut w, PLAYER(), "step", json!({"ticks": 1})),
            page.send(&mut w, PLAYER(), "time_control", json!({"pause": false})),
        ];
        let pacing =
            json!({"pacing": {"pacing": "real_time", "speed": 20, "pause_on_decision": true}});
        let set = page.send(&mut w, DEV(), "player.pacing", pacing);
        w.run(8.0);
        page.take(&mut w);
        for seq in refused {
            let r = page.reply(seq).unwrap();
            assert_eq!(r["error"]["code"], json!("permission.denied"), "{r:#}");
        }
        page.ok(set);
        // Real time at speed 20 would run a tick every 0.83 ms: two seconds pass, nothing runs.
        for _ in 0..20 {
            t.set(t.get() + 100.0);
            assert_eq!(w.run(8.0), Next::Command);
            page.take(&mut w);
        }
        assert_eq!(w.game().tick().0, 0);
        let s = page.send(&mut w, PLAYER(), "player.session", json!({}));
        page.run_until(&mut w, &t, 0.0, s, 5);
        assert_eq!(page.ok(s)["status"]["paused_for"], json!(["skipper"]));
        let wait = page.send(&mut w, PLAYER(), "player.wait", json!({}));
        page.run_until(&mut w, &t, 0.0, wait, 5);
        let pending = page.ok(wait);
        assert_eq!(
            (pending["stopped"].clone(), pending["ran"].clone()),
            (json!("decision"), json!(0))
        );
        // Acting with resume answers it: the world runs to the sightings of tick 1 and holds.
        let mut act = sail_to_mark1();
        act["resume"] = json!(true);
        let act = page.send(&mut w, PLAYER(), "player.act", act);
        page.run_until(&mut w, &t, 0.0, act, 5);
        page.ok(act);
        for _ in 0..40 {
            t.set(t.get() + 50.0);
            w.run(8.0);
            page.take(&mut w);
        }
        assert_eq!(w.game().tick().0, 1, "held at the sightings");
        // `continue` answers it; the long poll answers at the next decision, ticks later.
        let go = page.send(&mut w, PLAYER(), "player.continue", json!({}));
        let poll = page.send(
            &mut w,
            PLAYER(),
            "player.wait",
            json!({"max_wall_ms": 20000}),
        );
        page.run_until(&mut w, &t, 5.0, poll, 20_000);
        assert_eq!(page.ok(go)["stopped"], json!("answered"));
        let next = page.ok(poll);
        assert_eq!(next["stopped"], json!("decision"), "{next:#}");
        assert!(next["tick"].as_u64().unwrap() > 1, "{next:#}");
        // With the world held, a wait with a short wall limit answers at its limit.
        let short = page.send(
            &mut w,
            PLAYER(),
            "player.wait",
            json!({"until": {"event": "course.finished"}, "max_wall_ms": 100}),
        );
        let start = t.get();
        page.run_until(&mut w, &t, 10.0, short, 100);
        assert_eq!(page.ok(short)["stopped"], json!("wall_limit"));
        assert!(t.get() - start >= 100.0);
    });
}
