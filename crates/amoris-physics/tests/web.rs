//! The web tests natively (checks.md 7.2, `tests`): each must pass, and the report the
//! WebAssembly build prints must equal this one. The report is written to
//! `<target>/tmp/amoris-physics-web-report.txt` for `tests/web/run.mjs` to compare against.
//!
//! `AMORIS_BLESS=1 cargo test -p amoris-physics --test web` rewrites `src/probe/golden.txt`, the
//! sailing scene's committed hash chain, after a deliberate change of results (a Rapier, libm or
//! force-model change), whose reason goes in the commit message.

use amoris_physics::probe::{CHAIN_TICKS, GOLDEN, chain, golden_line};

#[test]
fn resident_checkpoint_section_preserves_the_legacy_sailing_state_chain() {
    // Adding an empty persisted resource deliberately changes the complete world hash. Verify
    // that removing only that new section recovers the exact previous 1200-tick chain before
    // blessing its new complete-state golden. This also guards unchanged CPU physics behavior.
    let ledger = amoris_physics::probe::ledger();
    let mut sim = amoris_physics::probe::sailing_world();
    let mut hashes = Vec::new();
    for tick in 0..=CHAIN_TICKS {
        assert!(
            sim.world()
                .resource::<amoris_physics::water::resident::WaterCheckpoints>()
                .fields
                .is_empty()
        );
        let mut snap = ledger.snapshot(sim.world()).unwrap();
        let count = snap.sections.len();
        snap.sections
            .retain(|(name, _)| *name != "WaterCheckpoints");
        assert_eq!(snap.sections.len() + 1, count);
        hashes.push(snap.hash());
        if tick < CHAIN_TICKS {
            sim.step(&mut amoris_sim::NoHooks).unwrap();
        }
    }
    assert_eq!(golden_line(&hashes), "chain 529a28aaa772e3c9 ticks 1200\n");
}

#[test]
fn web_report() {
    if std::env::var_os("AMORIS_BLESS").is_some() {
        let line = golden_line(&chain(CHAIN_TICKS).unwrap());
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/probe/golden.txt");
        std::fs::write(path, &line).unwrap();
        println!("blessed {}", line.trim());
        return;
    }
    let report = amoris_physics::web_report();
    let path =
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("amoris-physics-web-report.txt");
    std::fs::write(&path, &report).unwrap();
    assert!(
        !report.contains("FAIL"),
        "{}",
        &report[..report.len().min(2000)]
    );
    assert_eq!(
        report.lines().filter(|l| l.starts_with("ok ")).count(),
        amoris_physics::WEB_TESTS.len()
    );
    assert!(GOLDEN.starts_with("chain "));
}
