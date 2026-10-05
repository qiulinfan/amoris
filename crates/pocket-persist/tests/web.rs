//! The pinned vectors natively (persistence.md 12, P4; checks.md 7.2, `tests`): each must pass, and
//! the report the WebAssembly build prints must equal this one. The report is written to
//! `<target>/tmp/pocket-persist-web-report.txt` for `tests/web/run.mjs` to compare against.

#[test]
fn web_report() {
    let report = pocket_persist::vectors::web_report();
    let path =
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("pocket-persist-web-report.txt");
    std::fs::write(&path, &report).unwrap();
    assert!(!report.contains("FAIL"), "{report}");
    assert_eq!(
        report.lines().filter(|l| l.starts_with("ok ")).count(),
        pocket_persist::vectors::WEB_TESTS.len()
    );
}
