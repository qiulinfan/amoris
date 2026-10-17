//! Fork lookahead for the skipper (`amoris_runtime::lookahead`; docs/spec/player.md 8.1): a decision
//! weighs its candidates in branches and leaves main exactly as it was; the same variant played
//! twice decides the same and ends at the same world hash, with kept branches as with fresh ones;
//! the branches keep to the benchmark's limits; the model reaches a branch as its seat alone; a
//! game is forked into only when it is a branch of main; with forks off the model never forks.
#![cfg(feature = "transpile")]
mod common;
#[path = "../examples/lookahead_bench/course.rs"]
mod course;

use std::sync::{Arc, OnceLock};

use amoris_persist::replay::{MemorySink, RecordOptions};
use amoris_runtime::lookahead::{self, Episode, Limits, Lookahead, Model, Plan, Seat};
use amoris_runtime::skipper::Skipper;
use amoris_runtime::{Command, Game, GameSetup, Project};
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

/// The course's variant `seed` at tick 0.
fn variant(seed: u64, omniscient: bool) -> Game {
    course::game(course(), seed, omniscient)
}

fn session(g: &mut Game, seat: &mut Seat) -> Value {
    let mut s = seat
        .call(g, "player.session", json!({}))
        .unwrap_or_else(|e| panic!("{e:#?}"));
    // Statistics count the calls themselves.
    s.as_object_mut().map(|o| o.remove("stats"));
    s
}

/// A decision on main forks it once per candidate and changes nothing on main: not its hash, its
/// tick, nor the seat's pending decision; the branches keep to 4 and 600 ticks between them.
#[test]
fn a_decision_weighs_its_candidates_in_branches_and_leaves_main_untouched() {
    common::big_stack(|| {
        let mut g = variant(1, false);
        let mut seat = Seat::new(0);
        let mut look = Lookahead::new(Limits::default(), 0);
        let mut weighed = 0;
        loop {
            let obs = seat
                .call(&mut g, "player.observe", lookahead::observe_request())
                .unwrap_or_else(|e| panic!("{e:#?}"));
            let before = (
                g.world_hash().unwrap(),
                g.tick(),
                session(&mut g, &mut seat),
            );
            let d = look.weigh(&g, &obs).unwrap_or_else(|e| panic!("{e:#?}"));
            let after = (
                g.world_hash().unwrap(),
                g.tick(),
                session(&mut g, &mut seat),
            );
            assert_eq!(
                before, after,
                "the decision at tick {} changed main",
                d.tick
            );
            if !d.branches.is_empty() {
                weighed += 1;
                assert!((2..=4).contains(&d.branches.len()), "{d:#?}");
                assert_eq!(d.branches[0].plan, Plan::Greedy, "{d:#?}");
                let ticks: u64 = d.branches.iter().map(|b| b.ticks).sum();
                assert!(ticks <= 600, "{ticks} branch ticks in one decision: {d:#?}");
                assert!(d.branches.iter().all(|b| b.refused.is_none()), "{d:#?}");
            }
            if let Some(a) = d.act {
                seat.call(&mut g, "player.act", a)
                    .unwrap_or_else(|e| panic!("{e:#?}"));
            }
            let w = seat
                .call(&mut g, "player.wait", json!({}))
                .unwrap_or_else(|e| panic!("{e:#?}"));
            if w["stopped"] == json!("done") {
                break;
            }
        }
        assert!(weighed >= 3, "only {weighed} decisions ran branches");
        assert!(look.stats.max_branch_ticks_per_decision <= 600);
        assert_eq!(look.stats.looked, weighed);
    });
}

fn play(seed: u64, omniscient: bool, model: Model) -> Episode {
    let mut g = variant(seed, omniscient);
    let mut m = model;
    lookahead::play(&mut g, &mut m, 0, 36_000, None).unwrap_or_else(|e| panic!("{e:#?}"))
}

fn lookahead() -> Model {
    Model::Lookahead(Box::new(Lookahead::new(Limits::default(), 0)))
}

/// The same variant played twice decides the same in every branch and ends at the same world
/// hash; branches forked into kept games decide as fresh forks do (a kept script host carries
/// nothing over); the course is finished, the branch limits held.
#[test]
fn the_lookahead_is_deterministic_and_kept_branches_decide_as_fresh_ones() {
    common::big_stack(|| {
        let a = play(1, false, lookahead());
        let b = play(1, false, lookahead());
        let fresh = play(
            1,
            false,
            Model::Lookahead(Box::new(
                Lookahead::new(Limits::default(), 0).fresh_branches(),
            )),
        );
        let stats = a.lookahead.clone().unwrap();
        assert!(a.finished, "{a:#?}");
        assert!(stats.forks > 0, "{stats:#?}");
        assert!(stats.max_branch_ticks_per_decision <= 600, "{stats:#?}");
        for other in [&b, &fresh] {
            assert_eq!(a.world_hash, other.world_hash);
            assert_eq!(a.acts_log, other.acts_log);
            assert_eq!(a.lookahead, other.lookahead);
            let values = |e: &Episode| -> Vec<Vec<f64>> {
                e.weighed
                    .iter()
                    .map(|d| d.branches.iter().map(|b| b.value).collect())
                    .collect()
            };
            assert_eq!(values(&a), values(other));
        }
    });
}

/// The variants are what their seed says: seed 0 is the shipped course, another seed moves the
/// wind and the crates, the same seed the same way; the omniscient condition binds the seat.
#[test]
fn course_variants_follow_their_seed() {
    common::big_stack(|| {
        let mut g = Game::new(course(), 1).unwrap_or_else(|e| panic!("{e:#?}"));
        let c = course::Course::read(&mut g);
        assert_eq!(c.variant(0, false), None);
        assert_eq!(c.variant(7, false), c.variant(7, false));
        assert_ne!(c.variant(7, false), c.variant(8, false));
        let v = c.variant(3, true).unwrap();
        let ops = v["ops"].as_array().unwrap();
        assert_eq!(ops.len(), 6, "{v:#}");
        let from = ops[0]["set"]["value"]["from_deg"].as_f64().unwrap();
        let speed = ops[0]["set"]["value"]["speed"].as_f64().unwrap();
        assert!(
            (250.0..290.0).contains(&from) && (5.0..7.0).contains(&speed),
            "{v:#}"
        );
        assert_eq!(ops[5]["set"]["component"], json!("Observer"), "{v:#}");
        // Every variant the bench plays places its crates.
        for seed in (0..=30).chain(100..=119) {
            c.variant(seed, false);
        }
    });
}

/// The model reaches a branch as its seat: anything but a player tool is refused before it is
/// sent, and the runtime refuses the seat's player source a developer's read or the omniscient
/// view (the model's only route into a world).
#[test]
fn a_branch_is_reached_as_the_seat_alone() {
    common::big_stack(|| {
        let g = variant(0, false);
        let mut b = g.fork().unwrap_or_else(|e| panic!("{e:#?}"));
        let mut seat = Seat::new(0);
        let e = seat
            .call(&mut b, "world.get", json!({"entity": "Crate4"}))
            .unwrap_err();
        assert_eq!(e.code, "permission.denied", "{e:#?}");
        let e = b
            .apply(&Command::new(
                seat.source(),
                9,
                "world.get",
                json!({"entity": "Crate4"}),
            ))
            .unwrap_err();
        assert_eq!(e.code, "permission.denied", "{e:#?}");
        let e = seat
            .call(&mut b, "player.observe", json!({"omniscient": true}))
            .unwrap_err();
        assert!(e.code.contains("omniscient"), "{e:#?}");
    });
}

/// `Game::fork_into` makes a kept branch the same world as a fresh fork: same hash at once and
/// after both step, the main untouched.
#[test]
fn a_kept_branch_forked_into_again_matches_a_fresh_fork() {
    common::big_stack(|| {
        let mut main = variant(2, false);
        let mut skipper = Skipper::new();
        let mut seat = Seat::new(0);
        let obs = seat
            .call(&mut main, "player.observe", lookahead::observe_request())
            .unwrap();
        if let Some(a) = skipper.decide(&obs) {
            seat.call(&mut main, "player.act", a).unwrap();
        }
        main.run(200).unwrap();
        let mut kept = main.fork().unwrap();
        kept.run(150).unwrap();
        main.run(100).unwrap();
        let hash = main.world_hash().unwrap();
        main.fork_into(&mut kept).unwrap();
        let mut fresh = main.fork().unwrap();
        assert_eq!(kept.world_hash().unwrap(), hash);
        assert_eq!(kept.tick(), main.tick());
        kept.run(120).unwrap();
        fresh.run(120).unwrap();
        assert_eq!(kept.world_hash().unwrap(), fresh.world_hash().unwrap());
        assert_eq!(main.world_hash().unwrap(), hash);
    });
}

/// `Game::fork_into` refuses a game that is not a branch of main, before changing it: one built
/// from another setup (the same project, loaded again) or one that records.
#[test]
fn fork_into_refuses_a_game_that_is_not_a_branch() {
    common::big_stack(|| {
        let mut main = variant(2, false);
        main.run(30).unwrap();
        let mut other = Game::new(Arc::new((*course()).clone()), 1).unwrap();
        let before = other.world_hash().unwrap();
        let e = main.fork_into(&mut other).unwrap_err();
        assert_eq!(e.code, "request.invalid_value", "{e:#?}");
        assert_eq!(other.world_hash().unwrap(), before);
        let mut recording = main.fork().unwrap();
        recording.run(10).unwrap();
        recording
            .record(Box::new(MemorySink::new()), RecordOptions::default())
            .unwrap();
        let before = recording.world_hash().unwrap();
        let e = main.fork_into(&mut recording).unwrap_err();
        assert_eq!(e.code, "request.invalid_value", "{e:#?}");
        assert_eq!(recording.world_hash().unwrap(), before);
        let mut branch = main.fork().unwrap();
        branch.run(10).unwrap();
        main.fork_into(&mut branch).unwrap();
        assert_eq!(branch.world_hash().unwrap(), main.world_hash().unwrap());
    });
}

/// With forks off the model weighs the same candidates from the chart alone: it never forks or
/// runs a branch tick, still overrides the greedy skipper somewhere, and decides the same twice.
#[test]
fn with_forks_off_the_model_never_forks() {
    common::big_stack(|| {
        let nofork = || {
            Model::Lookahead(Box::new(
                Lookahead::new(Limits::default(), 0).without_forks(),
            ))
        };
        let a = play(1, false, nofork());
        let b = play(1, false, nofork());
        let stats = a.lookahead.clone().unwrap();
        assert!(a.finished, "{a:#?}");
        assert_eq!((stats.forks, stats.branch_ticks), (0, 0), "{stats:#?}");
        assert!(stats.looked > 0, "{stats:#?}");
        assert!(
            a.weighed
                .iter()
                .all(|d| d.branches.iter().all(|b| b.ticks == 0)),
            "{a:#?}"
        );
        assert_eq!(a.acts_log, b.acts_log);
        assert_eq!(a.world_hash, b.world_hash);
    });
}
