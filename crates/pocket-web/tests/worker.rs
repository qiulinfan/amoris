//! The worker's loop and the page's presenter, natively (docs/spec/threads.md 7 and 11): the same
//! code the browser runs, driven with a test clock. The sailing sample's hashes through the loop and
//! the web package equal `pocket check`'s chain; publication keeps at most two snapshots in flight
//! and still streams every tick's hash; commands are held for their tick and refused once it has
//! passed, or at once when they claim Host or repeat a seq; a boundary applies sources in the
//! canonical order whatever their arrival; a poisoned world halts and keeps answering; real time
//! follows the clock; and every snapshot rebuilds on the presenter to the worker's hash, while a
//! reordered or altered one is reported.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use pocket_check::{Subject, runs};
use pocket_contract::{Problem, detail};
use pocket_link::{Kind, source_json};
use pocket_runtime::{Game, GameBuilder};
use pocket_sim::{EventKind, NewEvent, RunCondition, Sim, SimClock, Tick, TickPhase};
use pocket_web::Pacing;
use pocket_web::package::Package;
use pocket_web::presenter::PresenterCore;
use pocket_web::worker::{Clock, MAX_IN_FLIGHT, Next, Out, WorkerCore};
use serde_json::{Value, json};

const TICKS: u64 = 600;

fn sailing() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../samples/sailing")
}

/// Runs `f` on a thread with the stack the script host needs.
fn on_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(pocket_runtime::GAME_STACK_BYTES)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap()
}

/// The time of a clock the test moves, in milliseconds (an `f64`'s bits).
#[derive(Clone)]
struct Time(Arc<AtomicU64>);

impl Time {
    fn set(&self, ms: f64) {
        self.0.store(ms.to_bits(), Ordering::SeqCst);
    }

    fn get(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::SeqCst))
    }
}

/// A clock the test moves.
fn clock() -> (Time, Clock) {
    let time = Time(Arc::new(AtomicU64::new(0)));
    let read = time.clone();
    (time, Arc::new(move || read.get()))
}

/// A clock that moves a millisecond each time it is read, so a task's 8 ms budget runs out after a
/// few ticks and the page's acknowledgements land between them.
fn moving_clock() -> Clock {
    let (time, _) = clock();
    Arc::new(move || {
        time.set(time.get() + 1.0);
        time.get()
    })
}

/// The game of the sailing sample's web package (built natively, read back as the worker reads it).
fn packaged(subject: &Subject, seed: u64) -> Game {
    let bytes = Package::from_setup(&subject.setup, seed).to_bytes();
    let setup = Package::from_bytes(&bytes).unwrap().setup().unwrap();
    Game::new(Arc::new(setup), seed).unwrap()
}

fn cmd(source: Value, seq: u64, at: Option<u64>, name: &str, params: Value) -> Value {
    let mut m = json!({"t": "cmd", "source": source, "seq": seq, "name": name, "params": params.to_string()});
    if let Some(at) = at {
        m["at"] = json!(at);
    }
    m
}

/// The sample's inputs as the page sends them: each with its tick as `at`.
fn push_inputs(w: &mut WorkerCore, subject: &Subject) {
    for t in 1..=subject.ticks() {
        for i in subject.inputs.at(t) {
            w.push(&cmd(
                source_json(i.source),
                i.seq,
                Some(t),
                &i.name,
                i.params.clone(),
            ));
        }
    }
}

/// What the page saw: every message, and the hash stream gathered from the `snap`s.
#[derive(Default)]
struct Page {
    hashes: Vec<(u64, String)>,
    replies: Vec<Value>,
    /// `ready`, `status` and `fatal`.
    others: Vec<Value>,
    snaps: Vec<(Value, Vec<u8>)>,
    unacked: Vec<u64>,
    max_unacked: usize,
}

impl Page {
    fn take(&mut self, w: &mut WorkerCore) {
        for o in w.take_out() {
            match o {
                Out::Message(m) if m["t"] == "reply" => self.replies.push(m),
                Out::Message(m) => self.others.push(m),
                Out::Snap(meta, bytes) => {
                    for h in meta["hashes"].as_array().unwrap() {
                        self.hashes
                            .push((h[0].as_u64().unwrap(), h[1].as_str().unwrap().to_owned()));
                    }
                    self.unacked.push(meta["version"].as_u64().unwrap());
                    self.max_unacked = self.max_unacked.max(self.unacked.len());
                    self.snaps.push((meta, bytes));
                }
            }
        }
    }

    fn ack_all(&mut self, w: &mut WorkerCore) {
        for v in std::mem::take(&mut self.unacked) {
            w.ack(v);
            self.take(w);
        }
    }
}

/// Runs the loop until `done`, acknowledging snapshots when `ack` says so.
fn drive(w: &mut WorkerCore, page: &mut Page, ack: bool, done: impl Fn(&Page) -> bool) {
    for _ in 0..100_000 {
        let next = w.run(8.0);
        page.take(w);
        if ack {
            page.ack_all(w);
        }
        if done(page) {
            return;
        }
        assert_ne!(next, Next::Quit);
        if next == Next::Command && !ack {
            page.ack_all(w);
        }
    }
    panic!("the loop never finished");
}

#[test]
fn the_loop_gives_the_native_chain_with_and_without_acknowledgements() {
    on_stack(|| {
        let subject = Subject::load(&sailing()).unwrap();
        let native = runs::chain(&subject, 1, TICKS).unwrap();
        for ack in [true, false] {
            let mut w =
                WorkerCore::new(packaged(&subject, 1), Pacing::Stepped, moving_clock()).unwrap();
            let mut page = Page::default();
            page.take(&mut w);
            push_inputs(&mut w, &subject);
            w.push(&cmd(
                json!("editor"),
                1,
                None,
                "step",
                json!({"ticks": TICKS}),
            ));
            drive(&mut w, &mut page, ack, |p| {
                p.replies.iter().any(|r| r["source"] == "editor")
                    && p.hashes.last().map(|h| h.0) == Some(TICKS)
            });
            assert!(
                page.max_unacked <= MAX_IN_FLIGHT as usize,
                "{}",
                page.max_unacked
            );
            let ticks: Vec<u64> = page.hashes.iter().map(|h| h.0).collect();
            assert_eq!(
                ticks,
                (0..=TICKS).collect::<Vec<_>>(),
                "every tick once, in order"
            );
            for (t, h) in &page.hashes {
                assert_eq!(
                    *h,
                    native[*t as usize].1.to_string(),
                    "tick {t}, acks {ack}"
                );
            }
            let errors: Vec<&Value> = page
                .replies
                .iter()
                .filter(|r| r.get("error").is_some())
                .collect();
            assert!(errors.is_empty(), "{errors:?}");
            let step = page
                .replies
                .iter()
                .find(|r| r["source"] == "editor")
                .unwrap();
            assert_eq!(step["ok"]["tick"], json!(TICKS));
            assert_eq!(
                step["ok"]["world_hash"],
                json!(native[TICKS as usize].1.to_string())
            );
            if !ack {
                // Without acknowledgements the worker posted two snapshots and skipped the rest.
                let skipped = w.timings().iter().filter(|t| !t.posted).count();
                assert!(skipped as u64 >= TICKS - 2, "{skipped}");
            }
        }
    });
}

#[test]
fn commands_are_held_for_their_tick_and_refused_once_it_passed() {
    on_stack(|| {
        let subject = Subject::load(&sailing()).unwrap();
        let (_, now) = clock();
        let mut w = WorkerCore::new(packaged(&subject, 1), Pacing::Stepped, now).unwrap();
        let mut page = Page::default();
        let status = json!({});
        w.push(&cmd(
            json!({"developer": 0}),
            1,
            Some(3),
            "status",
            status.clone(),
        ));
        w.push(&cmd(
            json!({"developer": 0}),
            2,
            None,
            "step",
            json!({"ticks": 5}),
        ));
        drive(&mut w, &mut page, true, |p| p.replies.len() == 2);
        // The status was applied at boundary 2, before tick 3.
        assert_eq!(
            page.replies[0]["ok"]["tick"],
            json!(2),
            "{:?}",
            page.replies
        );
        assert_eq!(page.replies[1]["ok"]["tick"], json!(5));
        w.push(&cmd(json!({"developer": 0}), 3, Some(2), "status", status));
        w.push(&cmd(
            json!({"player": 0}),
            1,
            None,
            "no_such_command",
            json!({}),
        ));
        w.push(&json!({"t": "cmd", "source": {"developr": 0}, "seq": 1, "name": "status"}));
        w.push(&json!({"t": "cmd", "source": "editor", "seq": 1, "name": "status", "parms": "{}"}));
        // Host is never handed out: the page cannot pass `command.host_only` by claiming it.
        let swap = json!({"bundle": "0".repeat(64)});
        w.push(&cmd(json!("host"), 1, None, "scripts.swap", swap));
        // Each source's seqs rise strictly: a repeat and a step back are refused, not queued.
        w.push(&cmd(json!({"developer": 0}), 3, None, "status", json!({})));
        w.push(&cmd(json!({"developer": 0}), 2, None, "status", json!({})));
        w.push(&cmd(json!({"player": 0}), 1, None, "status", json!({})));
        drive(&mut w, &mut page, true, |p| p.replies.len() == 10);
        let codes: Vec<&str> = page.replies[2..]
            .iter()
            .map(|r| r["error"]["code"].as_str().unwrap_or("ok"))
            .collect();
        assert_eq!(
            codes[..5],
            [
                "request.invalid_value",
                "request.unknown_field",
                "source.in_use",
                "request.out_of_range",
                "request.out_of_range",
            ],
            "{:?}",
            page.replies
        );
        assert_eq!(page.replies[4]["source"], json!("host"));
        assert_eq!(page.replies[4]["error"]["detail"]["source"], json!("host"));
        let back = &page.replies[6]["error"]["detail"];
        assert_eq!(
            (&back["path"], &back["got"], &back["min_exclusive"]),
            (&json!("/seq"), &json!(2), &json!(3))
        );
        assert!(codes.contains(&"command.tick_passed"), "{codes:?}");
        assert!(codes.contains(&"request.unknown_method"), "{codes:?}");
        // The repeated player seq was answered once only, as `request.out_of_range`; the first
        // `{"player": 0}, 1` is the unknown method's.
        let player_one: Vec<&str> = page
            .replies
            .iter()
            .filter(|r| r["source"] == json!({"player": 0}) && r["seq"] == 1)
            .map(|r| r["error"]["code"].as_str().unwrap_or("ok"))
            .collect();
        assert_eq!(
            player_one,
            ["request.out_of_range", "request.unknown_method"],
            "{:?}",
            page.replies
        );
        // A full queue refuses and keeps nothing.
        for seq in 0..pocket_link::QUEUE_CAPACITY as u64 + 1 {
            w.push(&cmd(
                json!({"player": 1}),
                seq + 1,
                Some(1_000_000),
                "status",
                json!({}),
            ));
        }
        page.take(&mut w);
        assert_eq!(
            page.replies.last().unwrap()["error"]["code"],
            json!("queue.full")
        );
        w.close();
        page.take(&mut w);
        let stopped = page
            .replies
            .iter()
            .filter(|r| r["error"]["code"] == "game.stopped")
            .count();
        assert_eq!(stopped, pocket_link::QUEUE_CAPACITY);
        assert_eq!(w.run(8.0), Next::Quit);
    });
}

/// threads.md 5.2: a held command whose tick the world passed without stopping at its boundary (a
/// world replaced at a later tick; here a test command moves the clock, as the game thread's test
/// does) is answered `command.tick_passed` at the next boundary and gives back its place in the
/// queue, rather than waiting for a boundary that never comes.
#[test]
fn held_commands_whose_tick_passed_are_answered_and_freed() {
    on_stack(|| {
        let subject = Subject::load(&sailing()).unwrap();
        let bytes = Package::from_setup(&subject.setup, 1).to_bytes();
        let setup = Package::from_bytes(&bytes).unwrap().setup().unwrap();
        let game = GameBuilder::new(Arc::new(setup))
            .seed(1)
            .command(
                "test.jump",
                Kind::Write,
                Arc::new(|b, _| {
                    b.world_mut().resource_mut::<SimClock>().tick = Tick(50);
                    Ok((json!({}), json!({})))
                }),
            )
            .build()
            .unwrap();
        let (_, now) = clock();
        let mut w = WorkerCore::new(game, Pacing::Stepped, now).unwrap();
        let mut page = Page::default();
        let held = pocket_link::QUEUE_CAPACITY as u64 - 1;
        for seq in 1..=held {
            w.push(&cmd(
                json!({"player": 0}),
                seq,
                Some(5),
                "status",
                json!({}),
            ));
        }
        w.run(8.0);
        page.take(&mut w);
        assert!(page.replies.is_empty(), "{:?}", page.replies);
        w.push(&cmd(
            json!({"developer": 0}),
            1,
            None,
            "test.jump",
            json!({}),
        ));
        // The jump applies at this boundary; the next one answers every held command of tick 5.
        w.run(8.0);
        assert_eq!(w.game().tick().0, 50);
        w.run(8.0);
        page.take(&mut w);
        let passed: Vec<&Value> = page
            .replies
            .iter()
            .filter(|r| r["source"] == json!({"player": 0}))
            .collect();
        assert_eq!(passed.len() as u64, held, "{:?}", page.replies);
        for r in passed {
            assert_eq!(r["error"]["code"], json!("command.tick_passed"), "{r}");
            assert_eq!(r["error"]["detail"]["boundary"], json!(50), "{r}");
        }
        // Their places are free again: the queue takes as many held commands once more.
        page.replies.clear();
        for seq in held + 1..=2 * held {
            w.push(&cmd(
                json!({"player": 0}),
                seq,
                Some(1_000),
                "status",
                json!({}),
            ));
        }
        page.take(&mut w);
        assert!(page.replies.is_empty(), "{:?}", page.replies);
    });
}

#[test]
fn real_time_follows_the_clock_and_pauses() {
    on_stack(|| {
        let subject = Subject::load(&sailing()).unwrap();
        let (time, now) = clock();
        let speed = 2.0;
        let mut w =
            WorkerCore::new(packaged(&subject, 1), Pacing::RealTime { speed }, now).unwrap();
        let mut page = Page::default();
        let interval = 1000.0 / 60.0 / speed;
        let Next::At(due) = w.run(8.0) else {
            panic!("real time waits for its first tick");
        };
        assert!((due - interval).abs() < 1e-9);
        time.set(due);
        w.run(8.0);
        page.take(&mut w);
        page.ack_all(&mut w);
        assert_eq!(w.game().tick().0, 1);
        // Far behind: at most the catch-up's ticks run, then the excess is dropped.
        time.set(due + 1000.0);
        let mut runs = 0;
        while matches!(w.run(8.0), Next::Now) || runs == 0 {
            runs += 1;
            page.take(&mut w);
            page.ack_all(&mut w);
            assert!(runs < 100);
        }
        let caught = w.game().tick().0;
        assert!((2..=8).contains(&caught), "{caught}");
        w.push(&cmd(
            json!("editor"),
            1,
            None,
            "time_control",
            json!({"pause": true}),
        ));
        assert_eq!(w.run(8.0), Next::Command);
        page.take(&mut w);
        let r = page.replies.last().unwrap();
        assert_eq!(r["ok"]["paused"], json!(true));
        time.set(due + 5000.0);
        assert_eq!(w.run(8.0), Next::Command);
        assert_eq!(w.game().tick().0, caught);
        w.push(&cmd(
            json!("editor"),
            2,
            None,
            "time_control",
            json!({"pacing": {"real_time": {"speed": 0}}}),
        ));
        w.run(8.0);
        page.take(&mut w);
        assert_eq!(
            page.replies.last().unwrap()["error"]["code"],
            json!("request.out_of_range")
        );
    });
}

#[test]
fn the_presenter_rebuilds_every_snapshot_and_reports_what_does_not_fit() {
    on_stack(|| {
        let subject = Subject::load(&sailing()).unwrap();
        let mut w =
            WorkerCore::new(packaged(&subject, 2), Pacing::Stepped, moving_clock()).unwrap();
        let mut page = Page::default();
        push_inputs(&mut w, &subject);
        w.push(&cmd(
            json!("editor"),
            1,
            None,
            "step",
            json!({"ticks": 120}),
        ));
        drive(&mut w, &mut page, true, |p| {
            p.replies.iter().any(|r| r["source"] == "editor")
        });
        assert!(
            page.snaps.len() > 20,
            "acknowledged between tasks: {}",
            page.snaps.len()
        );
        let mut presenter = PresenterCore::new();
        for (meta, bytes) in &page.snaps {
            let r = presenter.receive(meta, bytes).unwrap();
            assert_eq!(r["world_hash"], meta["world_hash"]);
        }
        assert!(
            presenter.problems().is_empty(),
            "{:?}",
            presenter.problems()
        );
        assert_eq!(presenter.received(), page.snaps.len() as u64);
        let streamed = presenter.take_hashes();
        assert_eq!(streamed.len(), 121);
        let view = presenter.view().unwrap();
        let sloop = view["entities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "Sloop")
            .unwrap_or_else(|| panic!("{view}"));
        assert!(
            sloop.get("Transform").is_some() && sloop.get("Boat").is_some(),
            "{sloop}"
        );
        // A snapshot out of order, and one whose bytes are not what the worker hashed.
        let mut fresh = PresenterCore::new();
        let (meta, bytes) = &page.snaps[1];
        fresh.receive(meta, bytes).unwrap();
        assert_eq!(
            fresh.problems()[0].detail["test"],
            json!("publication_order")
        );
        let (meta, _) = &page.snaps[2];
        let (_, other) = &page.snaps[3];
        let e = fresh.receive(meta, other).unwrap_err();
        assert_eq!(e.code, "web.threads_failed");
        assert_eq!(e.detail["test"], json!("rebuild"));
    });
}

/// The tick whose system faults in [`faulting`].
const FAULT_AT: u64 = 20;

/// A system that emits an event every tick (so a stream that sent an inbox twice would show) and
/// reports a fault at [`FAULT_AT`], as a script out of memory does: the world is poisoned mid-tick.
fn faults(sim: &mut Sim) -> Result<(), Problem> {
    sim.add_exclusive(
        "test.fault",
        TickPhase::Update,
        RunCondition::Always,
        |world, ctx| {
            let kind = EventKind::new("test.tick").expect("a valid kind");
            pocket_sim::event::emit(world, NewEvent::new(kind));
            if ctx.tick().0 == FAULT_AT {
                ctx.fault(Problem::new(
                    "script.out_of_memory",
                    "the test system ran out of memory",
                    detail([]),
                ));
            }
        },
    )
}

/// The sailing package's game with [`faults`] in its schedule.
fn faulting(subject: &Subject) -> Game {
    let bytes = Package::from_setup(&subject.setup, 1).to_bytes();
    let setup = Package::from_bytes(&bytes).unwrap().setup().unwrap();
    GameBuilder::new(Arc::new(setup))
        .seed(1)
        .system(faults)
        .build()
        .unwrap()
}

/// threads.md 8: a poisoned world halts the web game, never stops it. With and without
/// acknowledgements during the step that faults: no `fatal`, the status says halted, the hash
/// stream reaches the last good tick, every later snapshot is the last good one republished as
/// halted (none is built from the poisoned world), commands are still answered, a resume runs
/// nothing and streams no event twice.
#[test]
fn a_poisoned_world_halts_keeps_answering_and_streams_to_the_last_good_tick() {
    on_stack(|| {
        let subject = Subject::load(&sailing()).unwrap();
        for ack in [true, false] {
            let mut w =
                WorkerCore::new(faulting(&subject), Pacing::Stepped, moving_clock()).unwrap();
            let mut page = Page::default();
            page.take(&mut w);
            w.push(&cmd(json!("editor"), 1, None, "step", json!({"ticks": 40})));
            drive(&mut w, &mut page, ack, |p| {
                p.replies.iter().any(|r| r["source"] == "editor")
                    && p.hashes.last().map(|h| h.0) == Some(FAULT_AT - 1)
            });
            let step = page
                .replies
                .iter()
                .find(|r| r["source"] == "editor")
                .unwrap();
            assert_eq!(
                step["error"]["code"],
                json!("script.out_of_memory"),
                "{step}"
            );
            let ticks: Vec<u64> = page.hashes.iter().map(|h| h.0).collect();
            assert_eq!(ticks, (0..FAULT_AT).collect::<Vec<_>>(), "acks {ack}");
            let halted = |p: &Page| {
                p.others
                    .iter()
                    .filter(|m| m["t"] == "status" && m["state"] == "halted")
                    .count()
            };
            assert_eq!(halted(&page), 1, "{:?}", page.others);
            // The last snapshot built before the fault, and every one posted after it.
            let first_halted = page
                .snaps
                .iter()
                .position(|(m, _)| m["time"]["halted"] == true)
                .expect("a halted republication");
            let (good, good_bytes) = page.snaps[first_halted - 1].clone();
            assert!(good["header"]["tick"].as_u64().unwrap() < FAULT_AT);
            // Later commands: a player's edit, a Read, a refused step, then a resume.
            let edit = json!({"edits": [{"op": "set", "entity": "Sloop", "component": "Boat",
                "value": {"rudder": 0.5}}]});
            w.push(&cmd(json!({"player": 0}), 1, None, "world_edit", edit));
            w.push(&cmd(json!({"developer": 0}), 1, None, "status", json!({})));
            w.push(&cmd(
                json!({"developer": 0}),
                2,
                None,
                "step",
                json!({"ticks": 1}),
            ));
            drive(&mut w, &mut page, ack, |p| p.replies.len() == 4);
            // Real time asks for a tick once resumed; the poisoned world refuses it and halts again.
            let resume = json!({"pacing": {"real_time": {"speed": 8}}, "pause": false});
            w.push(&cmd(json!("editor"), 2, None, "time_control", resume));
            drive(&mut w, &mut page, ack, |p| halted(p) == 2);
            page.ack_all(&mut w);
            let reply = |source: Value, seq: u64| {
                page.replies
                    .iter()
                    .find(|r| r["source"] == source && r["seq"] == seq)
                    .unwrap_or_else(|| panic!("no reply to {source} {seq}: {:?}", page.replies))
            };
            let edited = reply(json!({"player": 0}), 1);
            assert!(edited.get("ok").is_some() || edited.get("error").is_some());
            let status = reply(json!({"developer": 0}), 1);
            assert_eq!(status["ok"]["poisoned"], json!(FAULT_AT), "{status}");
            let refused = reply(json!({"developer": 0}), 2);
            assert_eq!(refused["error"]["code"], json!("sim.world_poisoned"));
            let resumed = reply(json!("editor"), 2);
            assert!(resumed.get("ok").is_some(), "{resumed}");
            assert_eq!(halted(&page), 2, "the resume asked once and halted again");
            assert_eq!(w.run(8.0), Next::Command, "paused again");
            page.take(&mut w);
            assert!(
                !page.others.iter().any(|m| m["t"] == "fatal"),
                "{:?}",
                page.others
            );
            let ticks: Vec<u64> = page.hashes.iter().map(|h| h.0).collect();
            assert_eq!(
                ticks,
                (0..FAULT_AT).collect::<Vec<_>>(),
                "nothing after the halt"
            );
            for (meta, bytes) in &page.snaps[first_halted..] {
                assert_eq!(meta["time"]["halted"], json!(true), "{meta}");
                assert_eq!(meta["world_hash"], good["world_hash"]);
                assert_eq!(meta["header"], good["header"]);
                assert!(
                    *bytes == good_bytes,
                    "a snapshot built from the poisoned world"
                );
            }
            // The stream: one event per tick that ran (ticks 1 to 19), each once, in order.
            let records: Vec<Value> = page
                .snaps
                .iter()
                .flat_map(|(m, _)| m["events"].as_array().unwrap().clone())
                .collect();
            let ids: Vec<u64> = records
                .iter()
                .filter(|e| e["event"]["kind"] == "test.tick")
                .map(|e| e["id"].as_u64().unwrap())
                .collect();
            assert_eq!(ids.len() as u64, FAULT_AT - 1, "acks {ack}: {ids:?}");
            assert!(ids.windows(2).all(|p| p[0] < p[1]), "{ids:?}");
            let stream: Vec<u64> = records.iter().map(|e| e["seq"].as_u64().unwrap()).collect();
            assert_eq!(stream, (1..=stream.len() as u64).collect::<Vec<_>>());
        }
    });
}

/// threads.md 11, "Web ordering": edits of one Boat from the editor, two developers and a player,
/// all inputs of one tick, arrive in every order; the boundary applies them in the canonical order
/// (`pocket_link::canonical_order`), so every arrival order gives one world hash and the player's
/// value, the last applied.
#[test]
fn a_boundary_applies_sources_in_the_canonical_order_whatever_their_arrival() {
    const AT: u64 = 5;
    on_stack(|| {
        let subject = Subject::load(&sailing()).unwrap();
        let sources = [
            json!("editor"),
            json!({"developer": 0}),
            json!({"developer": 1}),
            json!({"player": 0}),
        ];
        let rudders = [0.11, -0.22, 0.33, -0.44];
        let mut orders: Vec<Vec<usize>> = vec![vec![]];
        for _ in 0..sources.len() {
            orders = orders
                .into_iter()
                .flat_map(|o| {
                    (0..sources.len())
                        .filter(|i| !o.contains(i))
                        .map(|i| [o.clone(), vec![i]].concat())
                        .collect::<Vec<_>>()
                })
                .collect();
        }
        assert_eq!(orders.len(), 24);
        let mut first: Option<Value> = None;
        for order in &orders {
            let mut w =
                WorkerCore::new(packaged(&subject, 1), Pacing::Stepped, moving_clock()).unwrap();
            let mut page = Page::default();
            for &i in order {
                let edit = json!({"edits": [{"op": "set", "entity": "Sloop", "component": "Boat",
                    "value": {"rudder": rudders[i]}}]});
                w.push(&cmd(sources[i].clone(), 1, Some(AT), "world_edit", edit));
            }
            let reader = json!({"developer": 2});
            w.push(&cmd(reader.clone(), 1, None, "step", json!({"ticks": AT})));
            drive(&mut w, &mut page, true, |p| p.replies.len() == 5);
            let get = json!({"entity": "Sloop", "components": ["Boat"]});
            w.push(&cmd(reader, 2, None, "world_get", get));
            drive(&mut w, &mut page, true, |p| p.replies.len() == 6);
            let errors: Vec<&Value> = page
                .replies
                .iter()
                .filter(|r| r.get("error").is_some())
                .collect();
            assert!(errors.is_empty(), "{order:?}: {errors:?}");
            let step = &page.replies[4]["ok"];
            let rudder = &page.replies[5]["ok"]["components"]["Boat"]["rudder"];
            assert_eq!(
                *rudder,
                json!(rudders[3]),
                "{order:?}: the player's edit is last"
            );
            match &first {
                None => first = Some(step["world_hash"].clone()),
                Some(h) => assert_eq!(step["world_hash"], *h, "{order:?}"),
            }
        }
    });
}

/// The worker's render feed carries the packaged scene's models to the page's viewport.
#[test]
fn the_render_feed_carries_the_packaged_scene() {
    on_stack(|| {
        let subject = Subject::load(&sailing()).unwrap();
        let mut game = packaged(&subject, 1);
        let feed = pocket_assets::Feed::new();
        let mailbox = feed.subscribe();
        let mut ex = pocket_runtime::Extractor::new();
        game.present(&mut ex, &feed);
        let full = mailbox.take();
        assert!(full.reset);
        assert_eq!(full.instances.len(), 5, "{full:?}");
        let decoded = pocket_assets::RenderFrame::decode(&full.encode()).unwrap();
        assert_eq!(decoded.instances.len(), 5);
    });
}
