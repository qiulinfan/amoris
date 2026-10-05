//! The web tests natively (checks.md 7.2, `tests`): each must pass, and the report the
//! WebAssembly build prints must equal this one. The report is written to
//! `<target>/tmp/pocket-physics-web-report.txt` for `tests/web/run.mjs` to compare against.
//!
//! `POCKET_BLESS=1 cargo test -p pocket-physics --test web` rewrites `src/probe/golden.txt`, the
//! sailing scene's committed hash chain, after a deliberate change of results (a Rapier, libm or
//! force-model change), whose reason goes in the commit message.

use pocket_physics::probe::{CHAIN_TICKS, GOLDEN, chain, golden_line};

#[test]
fn web_report() {
    if std::env::var_os("POCKET_BLESS").is_some() {
        let line = golden_line(&chain(CHAIN_TICKS).unwrap());
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/probe/golden.txt");
        std::fs::write(path, &line).unwrap();
        println!("blessed {}", line.trim());
        return;
    }
    let report = pocket_physics::web_report();
    let path =
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("pocket-physics-web-report.txt");
    std::fs::write(&path, &report).unwrap();
    assert!(
        !report.contains("FAIL"),
        "{}",
        &report[..report.len().min(2000)]
    );
    assert_eq!(
        report.lines().filter(|l| l.starts_with("ok ")).count(),
        pocket_physics::WEB_TESTS.len()
    );
    assert!(GOLDEN.starts_with("chain "));
}
