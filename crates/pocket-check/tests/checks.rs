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

/// The player scenario (docs/spec/player.md): the sailing course with the skipper's acts as
/// players' Writes passes determinism, fork, replay and reload equivalence; the world carries the
/// player declarations, so the replay rebuilds perception and the executors from its snapshot.
#[test]
fn the_sailing_course_player_scenario_passes_the_four_checks() {
    common::big_stack(|| {
        let s = common::subject("samples/sailing-course");
        let opts = Options {
            child: Some(common::in_process()),
            out_dir: Some(common::scratch("sailing-course-replays")),
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

/// checks.md 6.4: after the lint, the step writes the project's declarations and hands the project
/// to the caller's type checker; each of its diagnostics is a `types.error` with its place.
#[test]
fn the_types_step_writes_declarations_and_reports_tsc() {
    common::big_stack(|| {
        let src = common::repo().join("samples/sailing");
        let dir = common::scratch("types-tsc");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        for f in ["project.toml", "scene.json"] {
            std::fs::copy(src.join(f), dir.join(f)).unwrap();
        }
        for f in ["main.ts", "components.ts", "rules.ts"] {
            std::fs::copy(src.join("scripts").join(f), dir.join("scripts").join(f)).unwrap();
        }
        let seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let s2 = seen.clone();
        let opts = Options {
            only: Some(vec!["types".into()]),
            typecheck: Some(std::sync::Arc::new(move |p: &std::path::Path| {
                *s2.lock().unwrap() =
                    std::fs::read_to_string(p.join(".pocket/types/components.d.ts")).unwrap();
                json!({"typecheck": "failed", "tsc_ms": 40.0, "diagnostics": [{
                    "file": "scripts/rules.ts", "line": 22, "column": 89, "code": "TS2820",
                    "message": "Did you mean '\"Log.distance\"'?"}]})
            })),
            ..Options::default()
        };
        let r = check_project(&dir, &opts);
        let step = &r.steps[0];
        assert_eq!(step.verdict, Verdict::Fail, "{step:#?}");
        assert_eq!(codes(&r.steps, "types"), ["types.error"]);
        assert_eq!(step.errors[0].detail["path"], json!("scripts/rules.ts"));
        assert_eq!(step.errors[0].detail["line"], json!(22));
        let components = seen.lock().unwrap().clone();
        assert!(components.contains("Crew: {"), "{components}");
        assert!(components.contains("Boat: {"), "{components}");
        // A check writes only the engine's generated directory, never the source tree.
        assert!(!dir.join("tsconfig.json").exists());
        assert!(dir.join(".pocket/types/tsconfig.json").is_file());

        // Without a type checker that can run, the lint alone is no verdict.
        let opts = Options {
            only: Some(vec!["types".into()]),
            typecheck: Some(std::sync::Arc::new(|_: &std::path::Path| {
                json!({"typecheck": "unavailable", "diagnostics": [],
                       "reason": "no TypeScript 7 tsc"})
            })),
            ..Options::default()
        };
        let r = check_project(&dir, &opts);
        assert_eq!(
            r.steps[0].verdict,
            Verdict::Inconclusive,
            "{:#?}",
            r.steps[0]
        );
        assert_eq!(codes(&r.steps, "types"), ["check.tool_missing"]);
    });
}
