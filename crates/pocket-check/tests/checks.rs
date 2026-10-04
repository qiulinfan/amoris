//! The charter's four checks on the sailing scene, and the project negative controls of
//! tests/fixtures/controls (checks.md 8.6): each fails with its stated code.
#![cfg(feature = "native")]
mod common;

use pocket_check::{Options, Report, StepResult, Verdict, check_project, check_subject};
use serde_json::json;

fn codes(steps: &[StepResult], name: &str) -> Vec<String> {
    steps
        .iter()
        .find(|s| s.name == name)
        .map(|s| s.errors.iter().map(|e| e.code.clone()).collect())
        .unwrap_or_default()
}

/// Charter 10, slice 1: determinism, fork, replay and reload equivalence pass on the sailing scene.
#[test]
fn the_sailing_scene_passes_the_four_checks() {
    common::big_stack(|| {
        let s = common::subject("samples/sailing");
        let opts = Options {
            child: Some(common::in_process()),
            out_dir: Some(common::scratch("sailing-replays")),
            ..Options::default()
        };
        let steps = check_subject(&s, &opts);
        let names: Vec<&str> = steps.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["determinism", "fork", "replay", "reload"]);
        for step in &steps {
            assert_eq!(
                step.verdict,
                Verdict::Pass,
                "{}: {:#?}",
                step.name,
                step.errors
            );
        }
        let replay = steps
            .iter()
            .find(|s| s.name == "replay")
            .expect("a replay row");
        let found: Vec<f64> = replay
            .measurements
            .iter()
            .filter(|m| m.name.ends_with("_write_found_at"))
            .map(|m| m.value)
            .collect();
        assert_eq!(found.len(), 6, "{:#?}", replay.measurements);
        assert!(found.iter().all(|t| *t == 330.0), "{found:?}");
    });
}

#[test]
fn closure_and_module_state_fail_reload_equivalence() {
    common::big_stack(|| {
        for control in ["closure-state", "module-state"] {
            let dir = common::repo().join("tests/fixtures/controls").join(control);
            let opts = Options {
                only: Some(vec!["reload".into()]),
                ..Options::default()
            };
            let r: Report = check_project(&dir, &opts);
            assert_eq!(r.verdict, Verdict::Fail, "{control}");
            assert_eq!(codes(&r.steps, "reload"), ["reload.diverged"], "{control}");
            let e = &r.steps[0].errors[0];
            assert_eq!(e.detail["tick"], json!(31), "{control}: {e:#?}");
            let d = &e.detail["divergence"];
            assert_eq!(d["fields"][0]["path"], json!("/n"), "{control}");
        }
    });
}

#[test]
fn module_state_with_the_lint_on_fails_types() {
    let dir = common::repo().join("tests/fixtures/controls/module-state-lint");
    let opts = Options {
        only: Some(vec!["types".into()]),
        ..Options::default()
    };
    let r = check_project(&dir, &opts);
    assert!(codes(&r.steps, "types").contains(&"lint.module_let".to_owned()));
    // The closure is refused by the lint too: only with the lint off does it reach the backstop.
    let dir = common::repo().join("tests/fixtures/controls/closure-state");
    let r = check_project(&dir, &opts);
    let found = codes(&r.steps, "types");
    assert!(
        found.contains(&"lint.load_time_code".to_owned()),
        "{found:?}"
    );
}

/// checks.md 11: a misspelt or mistyped `check.toml` is `check.config_invalid` with the line of
/// the key, so `pocket check` can refuse it with exit code 2 before any check runs.
#[test]
fn a_misspelt_check_toml_names_its_line() {
    let dir = common::scratch("misspelt-check-toml");
    for (text, line) in [
        ("# a comment\n[run]\nseeds = [1]\ntiks = 60\n", 4),
        ("[run]\nseeds = [1]\nticks = \"sixty\"\n", 3),
        ("[run]\nseeds = [1]\nticks = 60\n[relaod]\nat = [1]\n", 4),
    ] {
        std::fs::write(dir.join("check.toml"), text).unwrap();
        let e = pocket_check::CheckToml::load(&dir).unwrap_err();
        assert_eq!(e.code, "check.config_invalid", "{text}: {e:#?}");
        assert_eq!(e.detail["line"], json!(line), "{text}: {e:#?}");
    }
}
