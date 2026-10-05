//! Tests of project discovery, the rows `pocket check` reports and the negative controls' verdicts.

use super::*;
use std::fs;

fn tree(name: &str, files: &[(&str, &str)]) -> (PathBuf, Vec<String>) {
    let root = std::env::temp_dir().join(format!("xtask-projects-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    for (path, text) in files {
        let p = root.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
    (root, files.iter().map(|(p, _)| p.to_string()).collect())
}

const SAIL: &str = "[run]\nseeds = [1, 2, 3]\nticks = 600\n\n[fork]\nat = [120]\nticks = 120\n\n[reload]\nat = [60]\n";
const CONTROL: &str = "[run]\nseeds = [1]\nticks = 120\n\n[expect]\nfail = \"reload.diverged\"\n";

#[test]
fn projects_are_found_under_samples_and_fixtures() {
    let (root, listed) = tree(
        "find",
        &[
            ("samples/sail/check.toml", SAIL),
            ("samples/sail/scripts/main.ts", ""),
            ("tests/fixtures/controls/module-state/check.toml", CONTROL),
            ("docs/check.toml", "not a project"),
        ],
    );
    let projects = discover(&root, &listed).unwrap();
    let dirs: Vec<&str> = projects.iter().map(|p| p.dir.as_str()).collect();
    assert_eq!(
        dirs,
        ["samples/sail", "tests/fixtures/controls/module-state"]
    );
    assert_eq!(projects[0].control_step(), None);
    assert_eq!(projects[1].control_step(), Some("reload"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_misspelt_check_toml_is_refused() {
    let (root, listed) = tree(
        "bad",
        &[(
            "samples/x/check.toml",
            "[run]\nseeds = [1]\nticks = 1\ntick = 2\n",
        )],
    );
    let p = discover(&root, &listed).err().unwrap();
    assert_eq!(p.code, "check.config_invalid");
    assert_eq!(p.detail["path"], json!("samples/x/check.toml"));
    let (root2, listed) = tree(
        "fam",
        &[(
            "samples/y/check.toml",
            "[run]\nseeds = [1]\nticks = 1\n[expect]\nfail = \"nope\"\n",
        )],
    );
    assert_eq!(
        discover(&root2, &listed).err().unwrap().code,
        "check.config_invalid"
    );
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(root2);
}

fn project(dir: &str, text: &str) -> Project {
    Project {
        dir: dir.into(),
        config: config::parse("check.toml", text).unwrap(),
    }
}

fn row(check: &str, codes: &[&str]) -> StepResult {
    let mut r = StepResult::new(check);
    for c in codes {
        r.error(Problem::new(c, "m", json!({})));
    }
    r
}

#[test]
fn a_projects_failures_are_tagged_with_it() {
    let mut step = StepResult::new("determinism");
    merge(
        &mut step,
        &project("samples/sail", SAIL),
        "determinism",
        &row("determinism", &["determinism.diverged"]),
    );
    assert_eq!(step.verdict, Verdict::Fail);
    assert_eq!(step.errors[0].detail["project"], json!("samples/sail"));
}

#[test]
fn controls_must_fail_with_their_code() {
    let control = project("tests/fixtures/controls/module-state", CONTROL);
    let mut caught = StepResult::new("reload");
    merge(
        &mut caught,
        &control,
        "reload",
        &row("reload", &["reload.diverged"]),
    );
    assert_eq!(caught.verdict, Verdict::Pass);

    let mut passed = StepResult::new("reload");
    merge(&mut passed, &control, "reload", &row("reload", &[]));
    assert_eq!(passed.errors[0].code, "check.control_passed");
    assert_eq!(passed.errors[0].detail["control"], json!("module-state"));

    let mut wrong = StepResult::new("reload");
    merge(
        &mut wrong,
        &control,
        "reload",
        &row("reload", &["reload.not_performed"]),
    );
    assert_eq!(wrong.errors[0].code, "check.control_wrong_failure");
    assert_eq!(
        wrong.errors[0].detail["got"],
        json!(["reload.not_performed"])
    );
}

#[test]
fn families() {
    assert_eq!(family("reload.diverged"), Some("reload"));
    assert_eq!(family("lint.module_state"), Some("types"));
    assert_eq!(family("web.cross_target_diverged"), Some("web"));
    assert_eq!(family("perf.noisy"), None);
}

#[test]
fn a_binary_without_check_is_a_command_failure_with_its_status() {
    // Any program that prints no report stands in for the stub `pocket` (rustc refuses `--json`).
    let p = probe(Path::new("rustc"), Path::new(".")).unwrap_err();
    assert_eq!(p.code, "check.command_failed");
    assert_ne!(p.detail["status"], json!(0));
    assert!(p.detail["status"].is_i64(), "{p:?}");
    assert!(!p.detail["tail"].as_str().unwrap().is_empty(), "{p:?}");
    let p = probe(Path::new("no-such-pocket-binary"), Path::new(".")).unwrap_err();
    assert_eq!(p.code, "check.command_failed");
    let p = pocket_binary(Path::new("no-such-target-dir")).unwrap_err();
    assert_eq!(p.code, "check.command_failed");
    assert!(p.message.contains("no pocket binary"), "{}", p.message);
}

/// Without `pocket check`, a step is Skipped only while no project takes part in it; a project
/// with a check.toml, or a control of that step, makes it a failure (checks.md 14.8).
#[test]
fn without_pocket_check_projects_fail_and_no_projects_skip() {
    let why = Problem::new(
        "check.command_failed",
        "pocket.exe check --json gave no report (exit Some(101))",
        json!({"command": "pocket.exe check --json", "status": 101, "tail": "panicked"}),
    );
    let projects = [
        project("samples/sail", SAIL),
        project("tests/fixtures/controls/module-state", CONTROL),
    ];
    let skipped = unchecked(
        "determinism",
        &participants(&[], "determinism"),
        why.clone(),
    );
    assert_eq!(skipped.verdict, Verdict::Skipped);
    assert!(
        skipped
            .skipped_because
            .as_deref()
            .unwrap()
            .starts_with("pocket-check not built yet: "),
        "{skipped:?}"
    );
    let failed = unchecked("reload", &participants(&projects, "reload"), why.clone());
    assert_eq!(failed.verdict, Verdict::Fail);
    assert_eq!(failed.errors[0].code, "check.command_failed");
    assert_eq!(failed.errors[0].detail["status"], json!(101));
    assert_eq!(
        failed.errors[0].detail["projects"],
        json!(["samples/sail", "tests/fixtures/controls/module-state"])
    );
    // The reload control alone does not take part in determinism; samples/sail does.
    let only_control = [project("tests/fixtures/controls/module-state", CONTROL)];
    let none = unchecked(
        "determinism",
        &participants(&only_control, "determinism"),
        why.clone(),
    );
    assert_eq!(none.verdict, Verdict::Skipped);
    let one = unchecked("determinism", &participants(&projects, "determinism"), why);
    assert_eq!(one.errors[0].detail["projects"], json!(["samples/sail"]));
}

fn least(check: &str, verdict: &str) -> String {
    format!(
        r#"{{"format": 1, "verdict": "{verdict}", "steps": [{{"name": "{check}", "verdict": "{verdict}"}}]}}"#
    )
}

/// The least `pocket check --json` may print: the format, the verdict and the check's step.
#[test]
fn a_minimal_report_from_pocket_check_is_read() {
    let text = r#"{"format": 1, "verdict": "Fail", "steps": [{"name": "determinism", "verdict": "Fail",
        "errors": [{"code": "determinism.diverged", "message": "m", "detail": {"tick": 377}}]}]}"#;
    let row = read_row(text, Some(1), "determinism").unwrap();
    let mut step = StepResult::new("determinism");
    merge(
        &mut step,
        &project("samples/sail", SAIL),
        "determinism",
        &row,
    );
    assert_eq!(step.errors[0].detail["tick"], json!(377));
    assert_eq!(step.errors[0].detail["project"], json!("samples/sail"));
}

/// A row that gives only a verdict still decides the step: Fail fails it, Inconclusive makes it
/// inconclusive, each with a problem that names the project.
#[test]
fn a_verdict_without_a_problem_still_counts() {
    let sail = project("samples/sail", SAIL);
    let row = read_row(&least("determinism", "Fail"), Some(1), "determinism").unwrap();
    let mut failed = StepResult::new("determinism");
    merge(&mut failed, &sail, "determinism", &row);
    assert_eq!(failed.verdict, Verdict::Fail);
    assert_eq!(failed.errors[0].code, "check.command_failed");
    assert_eq!(failed.errors[0].detail["verdict"], json!("Fail"));
    assert_eq!(failed.errors[0].detail["project"], json!("samples/sail"));

    let row = read_row(&least("fork", "Inconclusive"), Some(3), "fork").unwrap();
    let mut inconclusive = StepResult::new("fork");
    merge(&mut inconclusive, &sail, "fork", &row);
    assert_eq!(inconclusive.verdict, Verdict::Inconclusive);
    assert_eq!(
        inconclusive.errors[0].detail["verdict"],
        json!("Inconclusive")
    );

    let row = read_row(&least("replay", "Pass"), Some(0), "replay").unwrap();
    let mut passed = StepResult::new("replay");
    merge(&mut passed, &sail, "replay", &row);
    assert_eq!(passed.verdict, Verdict::Pass);
    assert!(passed.errors.is_empty());
}

#[test]
fn errors_past_the_cap_are_still_counted() {
    let mut row = row("determinism", &["determinism.diverged"]);
    row.more_errors = 7;
    let mut step = StepResult::new("determinism");
    merge(
        &mut step,
        &project("samples/sail", SAIL),
        "determinism",
        &row,
    );
    assert_eq!(step.more_errors, 7);
}

/// The exit status must be the one checks.md 11 gives the row's verdict.
#[test]
fn an_exit_status_that_disagrees_with_the_row_is_refused() {
    let why = read_row(&least("reload", "Fail"), Some(0), "reload").unwrap_err();
    assert!(why.contains("exited Some(0)"), "{why}");
    assert!(read_row(&least("reload", "Pass"), Some(1), "reload").is_err());
    assert!(read_row(&least("reload", "Inconclusive"), Some(1), "reload").is_err());
    assert!(read_row(&least("reload", "Pass"), None, "reload").is_err());
    assert!(read_row(&least("reload", "Pass"), Some(0), "fork").is_err());
    assert!(read_row("thread 'main' panicked", Some(101), "reload").is_err());
    assert!(read_row(&least("reload", "Skipped"), Some(0), "reload").is_ok());
}
