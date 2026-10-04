//! The real schedules of the samples, pinned (simulation-slice1.md 8): pocket-sim's golden fixes the
//! order rules with test systems, this one fixes what the runtime actually registers (engine,
//! physics, assets and the project's script systems), so a new or moved system is a reviewed diff.
//! `POCKET_BLESS=1 cargo test -p pocket-runtime --test schedule` rewrites the goldens.

mod common;

use std::sync::Arc;

use pocket_runtime::{Game, Project};

fn check(sample: &str) {
    let dir = common::repo().join("samples").join(sample);
    let golden = common::repo().join(format!("crates/pocket-runtime/tests/golden/schedule-{sample}.txt"));
    let listing = common::big_stack(move || {
        let p = Project::load(&dir).unwrap_or_else(|e| panic!("{e:#?}"));
        let setup = Arc::new(p.setup(false).unwrap_or_else(|e| panic!("{e:#?}")));
        let game = Game::new(setup, p.manifest.seed).unwrap_or_else(|e| panic!("{e:#?}"));
        game.sim().schedule_listing()
    });
    if std::env::var_os("POCKET_BLESS").is_some() {
        std::fs::write(&golden, &listing).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&golden)
        .unwrap_or_else(|_| panic!("{} is missing: run with POCKET_BLESS=1", golden.display()));
    assert_eq!(listing, want, "the {sample} schedule changed: review, then POCKET_BLESS=1");
}

#[test]
fn the_sailing_schedule_is_pinned() {
    check("sailing");
}

#[test]
fn the_anim_schedule_is_pinned() {
    check("anim");
}
