//! The `lint-fixture` control (docs/spec/checks.md 5.3 and 8.6): a crate outside the workspace
//! that uses every entry of `tools/clippy-determinism.toml`, which clippy must report. It proves
//! the lists only for the versions it links, so first the packages it depends on directly
//! (`bevy_ecs`, `bevy_platform`, `hashbrown`) must resolve, in its own `Cargo.lock`, to exactly the
//! versions the tick-code crates reach in the workspace's; otherwise a dependency update could
//! leave the workspace on paths that no longer resolve while the fixture still passes.

use super::cargo::{command_failed, go};
use super::diag;
use super::generate::{CLIPPY_SOURCE, ClippySource};
use crate::config;
use crate::report::{Problem, StepResult};
use crate::run::Env;
use serde::Deserialize;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const LINT_FIXTURE: &str = "tools/lint-fixture";
const FIXTURE_PACKAGE: &str = "lint-fixture";

/// The part of a `Cargo.lock` the comparison reads.
#[derive(Debug, Default, Deserialize)]
pub struct Lock {
    #[serde(default)]
    pub package: Vec<LockPackage>,
}

#[derive(Debug, Deserialize)]
pub struct LockPackage {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
}

/// The versions of every package that `roots` reach through the lock's dependency lists (every
/// kind and target, as `--all-targets` and the web target link them). An entry is `name`,
/// `name version` or `name version (source)`; the version is there when the name is ambiguous.
pub fn reached(lock: &Lock, roots: &BTreeSet<String>) -> BTreeMap<String, BTreeSet<String>> {
    let mut seen = vec![false; lock.package.len()];
    let mut stack: Vec<usize> = (0..lock.package.len())
        .filter(|&i| roots.contains(&lock.package[i].name))
        .collect();
    while let Some(i) = stack.pop() {
        if std::mem::replace(&mut seen[i], true) {
            continue;
        }
        for dep in &lock.package[i].dependencies {
            let mut parts = dep.split_whitespace();
            let name = parts.next().unwrap_or_default();
            let version = parts.next();
            stack.extend((0..lock.package.len()).filter(|&j| {
                let p = &lock.package[j];
                p.name == name && version.is_none_or(|v| v == p.version)
            }));
        }
    }
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (p, _) in lock.package.iter().zip(seen).filter(|(_, s)| *s) {
        out.entry(p.name.clone())
            .or_default()
            .insert(p.version.clone());
    }
    out
}

/// `gen.stale` for each package the fixture depends on directly whose versions in the fixture's
/// lock differ from those the tick-code crates reach in the workspace's lock.
pub fn stale_versions(fixture: &Lock, workspace: &Lock, tick: &BTreeSet<String>) -> Vec<Problem> {
    let path = format!("{LINT_FIXTURE}/Cargo.toml");
    let Some(me) = fixture.package.iter().find(|p| p.name == FIXTURE_PACKAGE) else {
        return vec![Problem::new(
            "gen.stale",
            format!("{LINT_FIXTURE}/Cargo.lock has no package {FIXTURE_PACKAGE}"),
            json!({"path": format!("{LINT_FIXTURE}/Cargo.lock")}),
        )];
    };
    let direct: BTreeSet<&str> = me
        .dependencies
        .iter()
        .filter_map(|d| d.split_whitespace().next())
        .collect();
    let in_fixture = reached(fixture, &BTreeSet::from([FIXTURE_PACKAGE.to_string()]));
    let in_workspace = reached(workspace, tick);
    let none = BTreeSet::new();
    let mut out = Vec::new();
    for package in direct {
        let f = in_fixture.get(package).unwrap_or(&none);
        let w = in_workspace.get(package).unwrap_or(&none);
        if f == w {
            continue;
        }
        out.push(Problem::new(
            "gen.stale",
            format!(
                "the lint fixture links {package} {f:?}, the tick-code crates {w:?}: pin the workspace's versions in {path} and run `cargo update --manifest-path {path}`, or the fixture proves the lists for versions the workspace does not build"
            ),
            json!({"path": path, "package": package, "fixture": f, "workspace": w}),
        ));
    }
    out
}

/// The package names of the tick-code crates the lists apply to (`crates` of the source, the
/// fixture aside), read from their manifests; a crate not created yet is left out.
fn tick_crates(root: &Path, source: &ClippySource) -> BTreeSet<String> {
    source
        .crates
        .iter()
        .filter(|dir| dir.as_str() != LINT_FIXTURE)
        .filter_map(|dir| std::fs::read_to_string(root.join(dir).join("Cargo.toml")).ok())
        .filter_map(|text| {
            let manifest: toml::Value = toml::from_str(&text).ok()?;
            Some(manifest.get("package")?.get("name")?.as_str()?.to_string())
        })
        .collect()
}

/// Lints the fixture, which must make clippy report every entry of the lists, once its versions
/// match the workspace's. Returns the summary's words.
pub fn lint(env: &Env, step: &mut StepResult, log: &Path, target_dir: Option<&str>) -> String {
    let source: ClippySource = match config::load(&env.root, CLIPPY_SOURCE) {
        Ok(s) => s,
        Err(p) => {
            step.error(p);
            return "not run".into();
        }
    };
    let locks = config::load::<Lock>(&env.root, &format!("{LINT_FIXTURE}/Cargo.lock"))
        .and_then(|f| config::load::<Lock>(&env.root, "Cargo.lock").map(|w| (f, w)));
    let stale = match locks {
        Ok((fixture, workspace)) => {
            stale_versions(&fixture, &workspace, &tick_crates(&env.root, &source))
        }
        Err(p) => vec![p],
    };
    if !stale.is_empty() {
        for p in stale {
            step.error(p);
        }
        return "not run: its versions differ from the workspace's".into();
    }
    let manifest = format!("{LINT_FIXTURE}/Cargo.toml");
    let mut args = vec![
        "clippy".to_string(),
        "--manifest-path".into(),
        manifest,
        "--locked".into(),
        "--message-format=json".into(),
    ];
    if let Some(dir) = target_dir {
        args.extend(["--target-dir".into(), dir.into()]);
    }
    eprintln!("  clippy over the lint fixture");
    let cmd = env.cargo(args);
    let line = crate::run::describe(&cmd);
    let Some(out) = go(step, cmd, log) else {
        return "not run".into();
    };
    let reported: BTreeSet<String> = diag::all(&out.text)
        .into_iter()
        .filter(|d| {
            matches!(
                d.lint.as_deref(),
                Some("clippy::disallowed_methods" | "clippy::disallowed_types")
            )
        })
        .filter_map(|d| d.message.split('`').nth(1).map(String::from))
        .collect();
    if !out.ok() {
        command_failed(step, &line, &out);
        return "did not build".into();
    }
    let expected: Vec<&str> = source
        .methods
        .iter()
        .chain(&source.types)
        .map(|e| e.path.as_str())
        .collect();
    let missing: Vec<&str> = expected
        .iter()
        .copied()
        .filter(|p| !reported.contains(*p))
        .collect();
    for p in &missing {
        step.error(Problem::new(
            "check.control_passed",
            format!("the lint fixture uses {p}, but clippy did not report it: the entry no longer resolves, or the fixture lost its use"),
            json!({"control": "lint-fixture", "expected": format!("clippy.warning for {p}")}),
        ));
    }
    step.measure(
        "clippy.fixture_reported",
        (expected.len() - missing.len()) as f64,
        "count",
    );
    format!(
        "{} of {} entries reported",
        expected.len() - missing.len(),
        expected.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock(text: &str) -> Lock {
        config::parse("Cargo.lock", text).unwrap()
    }

    const WORKSPACE: &str = r#"
version = 4
[[package]]
name = "pocket-sim"
version = "0.1.0"
dependencies = ["bevy_ecs", "indexmap"]
[[package]]
name = "pocket-render"
version = "0.1.0"
dependencies = ["wgpu"]
[[package]]
name = "wgpu"
version = "29.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
dependencies = ["hashbrown 0.15.5"]
[[package]]
name = "bevy_ecs"
version = "0.19.1"
dependencies = ["bevy_platform"]
[[package]]
name = "bevy_platform"
version = "0.19.1"
dependencies = ["hashbrown 0.16.1"]
[[package]]
name = "indexmap"
version = "2.14.2"
dependencies = ["hashbrown 0.17.1"]
[[package]]
name = "hashbrown"
version = "0.15.5"
[[package]]
name = "hashbrown"
version = "0.16.1"
[[package]]
name = "hashbrown"
version = "0.17.1"
"#;

    const FIXTURE: &str = r#"
version = 4
[[package]]
name = "lint-fixture"
version = "0.1.0"
dependencies = ["bevy_ecs", "bevy_platform", "hashbrown 0.16.1"]
[[package]]
name = "bevy_ecs"
version = "0.19.1"
dependencies = ["bevy_platform", "indexmap"]
[[package]]
name = "bevy_platform"
version = "0.19.1"
dependencies = ["hashbrown 0.16.1"]
[[package]]
name = "indexmap"
version = "2.14.2"
dependencies = ["hashbrown 0.17.1"]
[[package]]
name = "hashbrown"
version = "0.16.1"
[[package]]
name = "hashbrown"
version = "0.17.1"
"#;

    fn tick() -> BTreeSet<String> {
        BTreeSet::from(["pocket-sim".to_string(), "pocket-physics".to_string()])
    }

    #[test]
    fn only_what_the_roots_reach_counts() {
        let got = reached(&lock(WORKSPACE), &tick());
        assert_eq!(
            got["hashbrown"],
            BTreeSet::from(["0.16.1".to_string(), "0.17.1".to_string()])
        );
        assert!(!got.contains_key("wgpu"));
    }

    #[test]
    fn matching_versions_are_not_stale() {
        assert_eq!(
            stale_versions(&lock(FIXTURE), &lock(WORKSPACE), &tick()),
            Vec::<Problem>::new()
        );
    }

    #[test]
    fn a_workspace_bump_makes_the_fixture_stale() {
        let bumped = WORKSPACE.replace(
            "name = \"bevy_platform\"\nversion = \"0.19.1\"",
            "name = \"bevy_platform\"\nversion = \"0.19.2\"",
        );
        let problems = stale_versions(&lock(FIXTURE), &lock(&bumped), &tick());
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(problems[0].code, "gen.stale");
        assert_eq!(
            problems[0].detail,
            json!({"path": "tools/lint-fixture/Cargo.toml", "package": "bevy_platform",
                "fixture": ["0.19.1"], "workspace": ["0.19.2"]})
            .as_object()
            .unwrap()
            .clone()
        );
        // A tick crate that starts linking another hashbrown is not covered by the fixture either.
        let wider = WORKSPACE.replace(
            "dependencies = [\"bevy_ecs\", \"indexmap\"]",
            "dependencies = [\"bevy_ecs\", \"indexmap\", \"wgpu\"]",
        );
        let problems = stale_versions(&lock(FIXTURE), &lock(&wider), &tick());
        assert_eq!(problems[0].detail["package"], json!("hashbrown"));
        assert_eq!(
            problems[0].detail["workspace"],
            json!(["0.15.5", "0.16.1", "0.17.1"])
        );
    }
}
