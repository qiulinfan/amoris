//! Pacing (shared/contract/time.md, Checks): pause-on-decision holds real time until the seat
//! answers or its thinking clock runs out, and `contract.time.pacing_equivalence`: a real-time
//! session at speed 4 with pause-on-decision and an agent stand-in, and a lockstep session of two
//! seats committing in random chunks, each replayed in stepped pacing from its applied actions,
//! give the same world at every tick. The real-time loop here is the game loop's (threads.md 3.3)
//! with its clock injected, so a run is reproducible; one test runs it on the wall clock.

mod common;

use common::fixture;
use common::ledger_hash;
use common::play::*;
use pocket_contract::Problem;
use pocket_interface::action::{ActDone, Caller};
use pocket_interface::time::control::Controller;
use pocket_interface::time::play::{DecisionFilter, PlayPacing, ThinkingClock};
use pocket_interface::time::session::{self, Ticker};
use pocket_interface::{Pace, Pacing, TimeModel};
use pocket_sim::math::{max, min};
use pocket_sim::{Sim, SimClock, StepReport, TickRate};
use serde_json::{Value, json};

/// A seeded generator for the agent stand-ins (not the world's: they are outside it).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A world that records its hash after every tick and every act it applied, with the tick it is
/// an input of: what a replay needs.
struct Recorded {
    sim: Sim,
    hashes: Vec<u64>,
    applied: Vec<(u64, Value)>,
}

impl Ticker for Recorded {
    fn world(&self) -> &bevy_ecs::prelude::World {
        self.sim.world()
    }

    fn tick(&mut self) -> Result<StepReport, Problem> {
        let r = self.sim.step(&mut pocket_sim::NoHooks)?;
        self.hashes.push(ledger_hash(&self.sim));
        Ok(r)
    }

    fn act(&mut self, caller: &Caller, raw: &Value) -> Result<ActDone, Problem> {
        let done = pocket_interface::action::act(self.sim.world_mut(), caller, raw)?;
        self.applied
            .push((done.applied_at.0, done.canonical.clone()));
        Ok(done)
    }
}

/// Replays `applied` in stepped pacing for `ticks` ticks into `sim`: the hash after every tick.
fn replay(
    mut sim: Sim,
    applied: &[(u64, Value)],
    ticks: u64,
    seat_of: impl Fn(&Value) -> String,
) -> Vec<u64> {
    let mut out = Vec::new();
    let mut next = 0;
    for t in 1..=ticks {
        while next < applied.len() && applied[next].0 == t {
            let caller = Caller::Player {
                seat: seat_of(&applied[next].1),
            };
            pocket_interface::action::act(sim.world_mut(), &caller, &applied[next].1)
                .unwrap_or_else(|p| panic!("replaying {}: {p:?}", applied[next].1));
            next += 1;
        }
        sim.step(&mut pocket_sim::NoHooks).unwrap();
        out.push(ledger_hash(&sim));
    }
    assert_eq!(next, applied.len(), "every recorded action was applied");
    out
}

/// One turn of the game loop at `now`: what the controller answered, after running a due tick.
fn turn(rec: &mut Recorded, ctl: &mut Controller, model: &mut TimeModel, now: f64) -> Pace {
    let p = ctl.pace(rec.world(), model, now);
    if p == Pace::RunTick {
        let r = rec.tick().unwrap();
        model.ran_tick(now);
        ctl.after_tick(rec.world(), &r.decisions, &r.errors, now);
    }
    p
}

fn real_time(clock: ThinkingClock, every_s: f64) -> (Controller, TimeModel) {
    let filter = DecisionFilter {
        events: Vec::new(),
        idle_s: None,
        every_s: Some(every_s),
    };
    let ctl = Controller::new(
        PlayPacing::RealTime {
            speed: 1.0,
            pause_on_decision: true,
            clock: Some(clock),
        },
        filter,
        Vec::new(),
    );
    (
        ctl,
        TimeModel::new(TickRate::DEFAULT, Pacing::RealTime { speed: 1.0 }),
    )
}

fn tick_of(rec: &Recorded) -> u64 {
    rec.world().resource::<SimClock>().tick.0
}

#[test]
fn a_decision_holds_real_time_until_answered_or_out_of_thinking_time() {
    let (sim, _) = world(0.0, 270.0, 6.0, 1);
    let mut rec = Recorded {
        sim,
        hashes: Vec::new(),
        applied: Vec::new(),
    };
    let clock = ThinkingClock {
        initial_s: 0.2,
        increment_s: 0.1,
        max_s: 0.25,
    };
    let (mut ctl, mut model) = real_time(clock, 1.0);
    ctl.attach(rec.world());
    // The episode's start is a decision: nothing runs while the clock runs.
    let mut now = 0.0;
    while now < 50.0 {
        assert_ne!(
            turn(&mut rec, &mut ctl, &mut model, now),
            Pace::RunTick,
            "at {now} ms"
        );
        now += 1.0;
    }
    // Answered at 50 ms: 0.15 s left, plus 0.1, capped at 0.25.
    let c = session::continue_(&rec, &mut ctl, &skipper(), &json!({}), &mut || 50.0).unwrap();
    assert_eq!(c["stopped"], json!("answered"));
    assert_eq!(ctl.clocks["skipper"].left_s, 0.25);
    // Real time runs; the decision after tick 60 holds it again.
    let mut held_at = None;
    while held_at.is_none() {
        match turn(&mut rec, &mut ctl, &mut model, now) {
            Pace::WaitUntil(t) => now = t,
            Pace::RunTick => {
                if ctl.pending("skipper").is_some() {
                    held_at = Some((now, tick_of(&rec)));
                }
            }
            p => panic!("{p:?} at {now}"),
        }
    }
    let (w, t) = held_at.unwrap();
    assert_eq!(t, 60);
    // No tick until the clock runs out 250 ms later; then it resumes within a tick's interval.
    let mut resumed = None;
    while resumed.is_none() {
        let p = turn(&mut rec, &mut ctl, &mut model, now);
        match p {
            Pace::RunTick => resumed = Some(now),
            Pace::WaitUntil(t) => now = t,
            Pace::WaitForCommand => now += 1.0,
            Pace::Quit => unreachable!(),
        }
    }
    let r = resumed.unwrap();
    assert!(
        r >= w + 250.0 && r <= w + 250.0 + 1000.0 / 60.0 + 1e-9,
        "held at {w}, resumed at {r}"
    );
    assert_eq!(tick_of(&rec), 61);
    let warnings = ctl.take_warnings("skipper");
    assert_eq!(warnings[0].code, "time.clock_out", "{warnings:?}");
    // Out of thinking time, later decisions do not hold the world: the seat plays in real time.
    let before = tick_of(&rec);
    let mut n = 0;
    while tick_of(&rec) < before + 120 && n < 100_000 {
        match turn(&mut rec, &mut ctl, &mut model, now) {
            Pace::WaitUntil(t) => now = t,
            Pace::WaitForCommand => panic!("held without thinking time at {now}"),
            _ => {}
        }
        n += 1;
    }
}

#[test]
fn pauses_hold_real_time_and_a_developers_resume_ends_a_halt() {
    let (sim, _) = world(0.0, 270.0, 6.0, 1);
    let mut rec = Recorded {
        sim,
        hashes: Vec::new(),
        applied: Vec::new(),
    };
    let dev = Caller::Developer;
    let filter = DecisionFilter {
        events: Vec::new(),
        idle_s: None,
        every_s: None,
    };
    let mut ctl = Controller::new(
        PlayPacing::RealTime {
            speed: 1.0,
            pause_on_decision: false,
            clock: None,
        },
        filter,
        Vec::new(),
    );
    let mut model = TimeModel::new(TickRate::DEFAULT, Pacing::RealTime { speed: 1.0 });
    ctl.attach(rec.world());
    let now = 0.0;
    // Runs the loop for `ms` of its clock: the ticks that ran.
    let mut clock = 0.0;
    let mut run = |rec: &mut Recorded, ctl: &mut Controller, ms: f64| {
        let before = tick_of(rec);
        let end = clock + ms;
        while clock < end {
            match turn(rec, ctl, &mut model, clock) {
                Pace::WaitUntil(t) => clock = min(t, end),
                Pace::WaitForCommand => clock = end,
                _ => {}
            }
        }
        tick_of(rec) - before
    };
    assert!(run(&mut rec, &mut ctl, 500.0) >= 29);
    // A developer's pause holds the world; its resume lets it run on.
    let pause = pocket_interface::time::pause::pause;
    let resume = pocket_interface::time::pause::resume;
    let s = pause(rec.world(), &mut ctl, &dev, &json!({}), now).unwrap();
    assert_eq!(
        (s["paused"].clone(), s["paused_by"].clone()),
        (json!(true), json!(["developer"]))
    );
    assert_eq!(run(&mut rec, &mut ctl, 1000.0), 0);
    resume(rec.world(), &mut ctl, &dev, &json!({}), now).unwrap();
    assert!(run(&mut rec, &mut ctl, 500.0) >= 29);
    // A seat the session does not allow to pause is refused; one it allows holds the world.
    let p = pause(rec.world(), &mut ctl, &skipper(), &json!({}), now).unwrap_err();
    assert_eq!(p.code, "time.cannot_pause");
    ctl.may_pause.insert("skipper".into());
    let s = pause(rec.world(), &mut ctl, &skipper(), &json!({}), now).unwrap();
    assert_eq!(s["paused_by"], json!(["seat"]));
    assert_eq!(run(&mut rec, &mut ctl, 1000.0), 0);
    resume(rec.world(), &mut ctl, &skipper(), &json!({}), now).unwrap();
    assert!(run(&mut rec, &mut ctl, 500.0) >= 29);
    // A script failure halts the world in every pacing until a developer resumes.
    let failure = Problem::new("script.failed", "a rule threw", pocket_contract::detail([]));
    ctl.after_tick(rec.world(), &[], &[failure], now);
    assert_eq!(run(&mut rec, &mut ctl, 1000.0), 0);
    let p = resume(rec.world(), &mut ctl, &skipper(), &json!({}), now);
    assert!(p.is_ok(), "a seat's resume ends only its own pause");
    assert_eq!(run(&mut rec, &mut ctl, 1000.0), 0, "still halted");
    resume(rec.world(), &mut ctl, &dev, &json!({}), now).unwrap();
    assert!(run(&mut rec, &mut ctl, 500.0) >= 29);
    // Stepped pacing has nothing to pause; a developer's resume still ends a halt there.
    let mut stepped = stepped();
    let p = pause(rec.world(), &mut stepped, &dev, &json!({}), now).unwrap_err();
    assert_eq!(p.code, "time.wrong_mode");
    assert_eq!(p.detail["takes"], json!(["step", "continue"]));
    let failure = Problem::new("script.failed", "again", pocket_contract::detail([]));
    stepped.after_tick(rec.world(), &[], &[failure], now);
    let a = session::step(
        &mut rec,
        &mut stepped,
        &skipper(),
        &json!({"ticks": 5}),
        &mut || 0.0,
    )
    .unwrap();
    assert_eq!(
        (a["stopped"].clone(), a["ran"].clone()),
        (json!("halted"), json!(0))
    );
    assert_eq!(
        a["halted"]["code"],
        json!("time.halted"),
        "a player sees the generic halt"
    );
    resume(rec.world(), &mut stepped, &dev, &json!({}), now).unwrap();
    let a = session::step(
        &mut rec,
        &mut stepped,
        &skipper(),
        &json!({"ticks": 5}),
        &mut || 0.0,
    )
    .unwrap();
    assert_eq!(a["ran"], json!(5), "{a}");
}

// The wall clock is what this test measures: the loop's waits, never a tick.
#[allow(clippy::disallowed_methods)]
#[test]
fn the_world_resumes_promptly_when_a_thinking_clock_runs_out_on_the_wall_clock() {
    let (sim, _) = world(0.0, 270.0, 6.0, 1);
    let mut rec = Recorded {
        sim,
        hashes: Vec::new(),
        applied: Vec::new(),
    };
    let clock = ThinkingClock {
        initial_s: 0.2,
        increment_s: 0.0,
        max_s: 0.2,
    };
    let (mut ctl, mut model) = real_time(clock, 600.0);
    ctl.attach(rec.world());
    let start = std::time::Instant::now();
    let now = || start.elapsed().as_secs_f64() * 1000.0;
    let held = now();
    // The loop's wait: sleeps of 1 ms until the instant it was told (threads.md 3.3).
    loop {
        match turn(&mut rec, &mut ctl, &mut model, now()) {
            Pace::RunTick => break,
            Pace::WaitUntil(t) => {
                while now() < t {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
            _ => std::thread::sleep(std::time::Duration::from_millis(1)),
        }
    }
    let late = now() - (held + 200.0);
    println!("resumed {late:.2} ms after the clock ran out");
    assert!(late >= 0.0, "resumed {late} ms before the clock ran out");
    // The budget is 2.3 ms on the reference laptop (time.md, Checks), which assumes the game
    // loop's 1 ms waits; on Windows without a raised timer resolution a 1 ms sleep takes up to
    // about 16 ms (threads.md 3.2), so a test machine is given room and the measure is printed.
    assert!(late < 50.0, "resumed {late} ms late");
}

#[test]
fn real_time_with_pauses_replays_tick_for_tick_in_stepped_pacing() {
    let scene = |sim: &mut Sim| {
        mark(sim, "Mark1", [0.0, 0.0, -300.0]);
        mark(sim, "Mark2", [300.0, 0.0, 0.0]);
    };
    let (mut sim, _) = world(0.0, 270.0, 6.0, 1);
    scene(&mut sim);
    let mut rec = Recorded {
        sim,
        hashes: Vec::new(),
        applied: Vec::new(),
    };
    let filter = DecisionFilter {
        every_s: Some(5.0),
        ..sailing_filter()
    };
    let clock = ThinkingClock {
        initial_s: 1.0,
        increment_s: 0.5,
        max_s: 2.0,
    };
    let mut ctl = Controller::new(
        PlayPacing::RealTime {
            speed: 4.0,
            pause_on_decision: true,
            clock: Some(clock),
        },
        filter,
        Vec::new(),
    );
    let mut model = TimeModel::new(TickRate::DEFAULT, Pacing::RealTime { speed: 4.0 });
    ctl.attach(rec.world());
    let mut rng = Lcg(42);
    let mut now = 0.0;
    let mut answer_at: Option<f64> = None;
    let headings = [0, 45, 90, 135, 180, 225, 315];
    let mut answered = 0;
    while tick_of(&rec) < 7200 {
        if ctl.pending("skipper").is_some() && answer_at.is_none() {
            answer_at = Some(now + rng.below(1500) as f64);
        }
        if let Some(at) = answer_at.filter(|at| *at <= now) {
            answer_at = None;
            let action = match rng.below(5) {
                0 => json!({"do": "start", "intent": "come_to_heading",
                            "params": {"heading_deg": headings[rng.below(7) as usize], "keep": true}}),
                1 => json!({"do": "start", "intent": "trim_sail", "params": {"keep": true}}),
                2 => {
                    json!({"do": "start", "intent": "sail_to", "target": if rng.below(2) == 0 { "Mark1" } else { "Mark2" }})
                }
                3 => {
                    json!({"do": "set", "controls": {"rudder": (rng.below(21) as f64 - 10.0) / 10.0}})
                }
                _ => Value::Null,
            };
            if action.is_null() {
                session::continue_(&rec, &mut ctl, &skipper(), &json!({}), &mut || at).unwrap();
            } else {
                // A refusal (an intent the boat cannot start now) changes nothing; it is not recorded.
                let _ = session::act(
                    &mut rec,
                    &mut ctl,
                    &skipper(),
                    &json!({"actions": [action], "resume": true}),
                    &mut || at,
                );
            }
            answered += 1;
        }
        match turn(&mut rec, &mut ctl, &mut model, now) {
            Pace::WaitUntil(t) => {
                now = max(answer_at.map_or(t, |a| min(a, t)), now);
            }
            Pace::WaitForCommand => now = max(answer_at.unwrap_or(now + 1.0), now),
            _ => {}
        }
    }
    assert!(
        answered > 20 && rec.applied.len() > 10,
        "{answered} answers, {} acts",
        rec.applied.len()
    );
    let (mut twin, _) = world(0.0, 270.0, 6.0, 1);
    scene(&mut twin);
    let stepped = replay(twin, &rec.applied, 7200, |_| "skipper".to_owned());
    assert_eq!(stepped.len(), rec.hashes.len());
    let first = stepped.iter().zip(&rec.hashes).position(|(a, b)| a != b);
    assert_eq!(
        first,
        None,
        "the stepped replay diverged at tick {:?}",
        first.map(|i| i + 1)
    );
}

#[test]
fn lockstep_of_two_seats_replays_tick_for_tick_in_stepped_pacing() {
    let (sim, _) = fixture::with_rules(Default::default(), true);
    let mut rec = Recorded {
        sim,
        hashes: Vec::new(),
        applied: Vec::new(),
    };
    let mut ctl = Controller::new(
        PlayPacing::Lockstep {
            seats: vec!["hand".into(), "foot".into()],
            delay_ticks: 0,
        },
        DecisionFilter::default(),
        Vec::new(),
    );
    let mut rng = Lcg(7);
    let mut pending = 0;
    let mut waited = 0;
    while tick_of(&rec) < 7200 {
        for seat in ["hand", "foot"] {
            let caller = Caller::Player { seat: seat.into() };
            if rng.below(3) == 0 {
                let action = if rng.below(2) == 0 {
                    json!({"do": "set", "controls": {"lever": rng.below(101)}})
                } else {
                    json!({"do": "start", "intent": "hold_for", "params": {"ticks": 1 + rng.below(50)}})
                };
                let (a, _) = session::act(
                    &mut rec,
                    &mut ctl,
                    &caller,
                    &json!({"actions": [action]}),
                    &mut || 0.0,
                )
                .unwrap();
                pending += usize::from(a["pending"] == json!(true));
            }
            let c = session::commit(
                &mut rec,
                &mut ctl,
                &caller,
                &json!({"ticks": 1 + rng.below(300)}),
                &mut || 0.0,
            )
            .unwrap();
            waited += usize::from(c["stopped"] == json!("waiting"));
        }
    }
    assert!(
        pending >= 3 && waited > 5,
        "{pending} pending acts, {waited} waiting answers"
    );
    let ticks = rec.hashes.len() as u64;
    let (twin, _) = fixture::with_rules(Default::default(), true);
    let stepped = replay(twin, &rec.applied, ticks, |v| {
        v["seat"].as_str().unwrap().to_owned()
    });
    let first = stepped.iter().zip(&rec.hashes).position(|(a, b)| a != b);
    assert_eq!(
        first,
        None,
        "the stepped replay diverged at tick {:?}",
        first.map(|i| i + 1)
    );
}
