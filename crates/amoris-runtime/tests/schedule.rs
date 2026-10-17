//! The real schedules of the samples, pinned (simulation-slice1.md 8): amoris-sim's golden fixes the
//! order rules with test systems, this one fixes what the runtime actually registers (engine,
//! physics, assets and the project's script systems), so a new or moved system is a reviewed diff.
//! `AMORIS_BLESS=1 cargo test -p amoris-runtime --features transpile --test schedule` rewrites the
//! goldens. Loading a project from disk compiles its TypeScript, so the test needs the crate's
//! `transpile` (a workspace run has it through amoris-app).
#![cfg(feature = "transpile")]

mod common;

use std::sync::Arc;

use amoris_runtime::{Game, Project};

fn check(sample: &str) {
    let dir = common::repo().join("samples").join(sample);
    let golden = common::repo().join(format!(
        "crates/amoris-runtime/tests/golden/schedule-{sample}.txt"
    ));
    let listing = common::big_stack(move || {
        let p = Project::load(&dir).unwrap_or_else(|e| panic!("{e:#?}"));
        let setup = Arc::new(p.setup(false).unwrap_or_else(|e| panic!("{e:#?}")));
        let game = Game::new(setup, p.manifest.seed).unwrap_or_else(|e| panic!("{e:#?}"));
        game.sim().schedule_listing()
    });
    if std::env::var_os("AMORIS_BLESS").is_some() {
        std::fs::write(&golden, &listing).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&golden)
        .unwrap_or_else(|_| panic!("{} is missing: run with AMORIS_BLESS=1", golden.display()));
    assert_eq!(
        listing, want,
        "the {sample} schedule changed: review, then AMORIS_BLESS=1"
    );
}

#[test]
fn the_sailing_schedule_is_pinned() {
    check("sailing");
}

#[test]
fn the_anim_schedule_is_pinned() {
    check("anim");
}

#[test]
fn the_sailing_course_schedule_is_pinned() {
    check("sailing-course");
}
