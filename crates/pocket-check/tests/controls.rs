//! The negative controls that need a defect in Rust (checks.md 8.6), as integration tests that
//! build the sailing game with an extra system and run the same check functions: state shared
//! through a process global, a value from the process's random hash keys, a write outside the
//! command queue, and a recorded input moved one tick. Each must fail with its stated code.
#![cfg(feature = "native")]
mod common;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use pocket_check::{
    ChildAnswer, ChildRequest, Subject, determinism, fork, replay, runs, verify_bytes,
};
use pocket_contract::Problem;
use pocket_runtime::GameBuilder;
use pocket_sim::{EventKind, NewEvent, PlainData, RunCondition, Sim, TickPhase};
use serde_json::json;

type Install = Arc<dyn Fn(&mut Sim) -> Result<(), Problem> + Send + Sync>;

/// A test-only system that emits `value()` every tick it runs: the defect reaches the world hash
/// through the event inbox.
fn emitting(key: &'static str, when: RunCondition, value: fn() -> f64) -> Install {
    Arc::new(move |sim: &mut Sim| {
        sim.add_exclusive(key, TickPhase::Update, when, move |world, _ctx| {
            let kind = EventKind::new("test.defect").expect("a kind");
            pocket_sim::event::emit(world, NewEvent::new(kind).data(PlainData::Number(value())));
        })
    })
}

fn with_defect(mut s: Subject, install: Install) -> Subject {
    s.build = Arc::new(move |setup, seed| {
        let install = install.clone();
        GameBuilder::new(setup.clone())
            .seed(seed)
            .system(move |sim| install(sim))
            .build()
    });
    s
}

static SHARED: AtomicU64 = AtomicU64::new(0);

fn shared_static() -> f64 {
    SHARED.fetch_add(1, Ordering::Relaxed) as f64
}

/// `shared-static`: a process-global counter written into the world fails the same-process
/// determinism variant and fork consistency (the original changes when a fork steps).
#[test]
fn shared_static_state_fails_determinism_and_fork() {
    common::big_stack(|| {
        let s = with_defect(
            common::subject("samples/sailing"),
            emitting("test.shared_static", RunCondition::Always, shared_static),
        );
        let d = determinism::check(&s, &[1], None);
        let codes: Vec<&str> = d.errors.iter().map(|e| e.code.as_str()).collect();
        assert_eq!(codes, ["determinism.diverged"], "{:#?}", d.errors);
        assert_eq!(d.errors[0].detail["variant"], json!("same_process"));
        let f = fork::check(&s, &[1]);
        let codes: Vec<&str> = f.errors.iter().map(|e| e.code.as_str()).collect();
        assert!(codes.contains(&"fork.original_changed"), "{codes:?}");
    });
}

/// A value from this process's random hash keys: the same for every game in the process, another
/// in the next process.
fn process_random() -> f64 {
    static V: OnceLock<f64> = OnceLock::new();
    *V.get_or_init(|| {
        use std::hash::{BuildHasher, RandomState};
        (RandomState::new().hash_one(42u64) >> 11) as f64
    })
}

fn process_random_subject() -> Subject {
    let mut s = with_defect(
        common::subject("samples/sailing"),
        emitting("test.process_random", RunCondition::Always, process_random),
    );
    s.config.run.ticks = 120;
    s
}

/// The child side of `process-random`: when the parent runs this binary again with
/// `POCKET_CHECK_CHILD=<seed>`, prints the defective game's chain.
#[test]
fn child_chain() {
    let Ok(seed) = std::env::var("POCKET_CHECK_CHILD") else {
        return;
    };
    let seed: u64 = seed.parse().expect("a seed");
    let chain = common::big_stack(move || {
        let s = process_random_subject();
        runs::chain(&s, seed, s.ticks()).expect("a chain")
    });
    let items: Vec<(u64, String)> = chain
        .iter()
        .map(|(t, h)| (t.tick.0, h.to_string()))
        .collect();
    println!("CHAIN {}", serde_json::to_string(&items).expect("JSON"));
}

fn hash(hex: &str) -> pocket_check::WorldHash {
    let b: Vec<u8> = (0..16)
        .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("hex"))
        .collect();
    pocket_check::WorldHash(b.try_into().expect("16 bytes"))
}

/// `process-random`: the same in one process, different across processes; only the
/// cross-process variant catches it, with this test binary run again as the child.
#[test]
fn process_random_fails_cross_process_determinism() {
    common::big_stack(|| {
        let s = process_random_subject();
        let child: pocket_check::Child = Arc::new(|_s: &Subject, req: &ChildRequest| {
            let ChildRequest::Chain { seed } = req else {
                return Err(Problem::new(
                    "check.command_failed",
                    "chains only",
                    pocket_contract::detail([]),
                ));
            };
            let out = std::process::Command::new(std::env::current_exe().expect("this binary"))
                .args(["--exact", "child_chain", "--nocapture", "--test-threads=1"])
                .env("POCKET_CHECK_CHILD", seed.to_string())
                .output()
                .expect("the child runs");
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            let line = text
                .lines()
                .find_map(|l| l.split_once("CHAIN ").map(|(_, chain)| chain))
                .expect("the child's chain");
            let items: Vec<(u64, String)> = serde_json::from_str(line).expect("a chain");
            Ok(ChildAnswer::Chain(
                items
                    .into_iter()
                    .map(|(t, h)| (runs::at(t), hash(&h)))
                    .collect(),
            ))
        });
        let d = determinism::check(&s, &[1], Some(&child));
        assert_eq!(d.errors.len(), 1, "{:#?}", d.errors);
        assert_eq!(d.errors[0].code, "determinism.diverged");
        assert_eq!(d.errors[0].detail["variant"], json!("cross_process"));
        assert_eq!(d.errors[0].detail["divergence"]["at"]["tick"], json!(1));
    });
}

fn unrecorded() -> f64 {
    50.0
}

/// `unrecorded-write`: a test-only path changes the world outside the command queue at tick 50 of
/// the recording run; replaying the recording names that tick.
#[test]
fn an_unrecorded_write_fails_the_replay() {
    common::big_stack(|| {
        let s = with_defect(
            common::subject("samples/sailing"),
            emitting(
                "test.unrecorded",
                RunCondition::every(1000, 50).expect("a condition"),
                unrecorded,
            ),
        );
        let r = replay::check(&s, &[1], &common::scratch("unrecorded"), None);
        assert_eq!(r.errors.len(), 1, "{:#?}", r.errors);
        assert_eq!(r.errors[0].code, "replay.diverged");
        assert_eq!(r.errors[0].detail["divergence"]["at"]["tick"], json!(50));
    });
}

/// A recorded input moved one tick later diverges exactly at the tick it was an input of, and one
/// dropped diverges there too, never earlier.
#[test]
fn a_moved_input_is_named_at_its_tick() {
    common::big_stack(|| {
        let s = common::subject("samples/sailing");
        let bytes = replay::record(&s, 2).expect("a recording");
        assert!(verify_bytes(&bytes).expect("a verify").identical);
        for how in [replay::Perturb::Delay, replay::Perturb::Drop] {
            let (copy, t) = replay::perturbed(&bytes, 100, how)
                .expect("a copy")
                .expect("a write at or after 100");
            assert_eq!(t, 100);
            let v = verify_bytes(&copy).expect("a verify");
            assert!(!v.identical);
            assert_eq!(v.divergence.expect("a divergence").at.tick.0, t, "{how:?}");
        }
    });
}
