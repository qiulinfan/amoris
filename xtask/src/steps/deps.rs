//! `deps`: the crate graph of docs/spec/architecture.md 5 (as `tools/crate-graph.toml`), the
//! external crates forbidden below the game group (architecture.md 6), resolved features, and the
//! build flags of architecture.md 7.3 (docs/spec/checks.md 5.2).

use super::metadata::Metadata;
use crate::config;
use crate::report::{Problem, StepResult};
use crate::run::{self, Env};
use serde::Deserialize;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

pub const CONFIG: &str = "tools/crate-graph.toml";
pub const WEB_TARGET: &str = "wasm32-unknown-unknown";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Graph {
    #[serde(rename = "crate")]
    pub crates: BTreeMap<String, CrateEntry>,
    #[serde(default)]
    pub group: BTreeMap<String, Group>,
    #[serde(default)]
    pub features: BTreeMap<String, FeatureRule>,
    pub build: BuildRule,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrateEntry {
    pub group: String,
    pub deps: Vec<String>,
    pub web: bool,
    /// Every crate may depend on it (architecture.md 4.16).
    #[serde(default)]
    pub any: bool,
    /// In architecture.md 5 but not created yet: its absence from the workspace is not an error.
    #[serde(default)]
    pub planned: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    #[serde(default)]
    pub forbid: Vec<String>,
    #[serde(default)]
    pub forbid_direct: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeatureRule {
    #[serde(default)]
    pub require: Vec<String>,
    #[serde(default)]
    pub forbid: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildRule {
    pub cflags_must_contain: String,
    pub rustflags_forbid: Vec<String>,
}

/// Loads the graph file and checks that its edges name crates it lists.
pub fn load(root: &std::path::Path) -> Result<Graph, Problem> {
    let graph: Graph = config::load(root, CONFIG)?;
    for (name, entry) in &graph.crates {
        for dep in &entry.deps {
            if !graph.crates.contains_key(dep) {
                return Err(config::invalid(
                    CONFIG,
                    None,
                    &format!("crate {name} lists {dep}, which has no [crate.{dep}]"),
                ));
            }
        }
    }
    Ok(graph)
}

impl Graph {
    /// The listed crates that build for the web and exist in the workspace.
    pub fn web_crates(&self, members: &BTreeSet<String>, group: Option<&str>) -> Vec<String> {
        self.crates
            .iter()
            .filter(|(n, e)| e.web && members.contains(*n) && group.is_none_or(|g| e.group == g))
            .map(|(n, _)| n.clone())
            .collect()
    }
}

/// What the rules read besides the metadata: the build flags from `.cargo/config.toml` and the
/// environment, each with where it came from.
#[derive(Debug, Default)]
pub struct Flags {
    pub cflags: Vec<(String, String)>,
    pub rustflags: Vec<(String, String)>,
}

pub fn flags(root: &std::path::Path) -> Flags {
    let mut f = Flags::default();
    if let Ok(text) = std::fs::read_to_string(root.join(".cargo/config.toml"))
        && let Ok(table) = text.parse::<toml::Table>()
    {
        if let Some(v) = table.get("env").and_then(|e| e.get("CFLAGS")) {
            let value = v
                .as_str()
                .or_else(|| v.get("value").and_then(|x| x.as_str()))
                .unwrap_or("");
            f.cflags
                .push((".cargo/config.toml [env] CFLAGS".into(), value.into()));
        }
        let mut flags_of = |source: String, v: Option<&toml::Value>| {
            if let Some(v) = v {
                let value = match v {
                    toml::Value::Array(a) => a
                        .iter()
                        .filter_map(|x| x.as_str())
                        .collect::<Vec<_>>()
                        .join(" "),
                    other => other.as_str().unwrap_or("").to_string(),
                };
                f.rustflags.push((source, value));
            }
        };
        flags_of(
            ".cargo/config.toml build.rustflags".into(),
            table.get("build").and_then(|b| b.get("rustflags")),
        );
        if let Some(targets) = table.get("target").and_then(|t| t.as_table()) {
            for (name, t) in targets {
                flags_of(
                    format!(".cargo/config.toml target.{name}.rustflags"),
                    t.get("rustflags"),
                );
            }
        }
    }
    if let Ok(v) = std::env::var("CFLAGS") {
        f.cflags.push(("environment CFLAGS".into(), v));
    }
    for var in [
        "RUSTFLAGS",
        "CARGO_BUILD_RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
    ] {
        if let Ok(v) = std::env::var(var) {
            f.rustflags
                .push((format!("environment {var}"), v.replace('\u{1f}', " ")));
        }
    }
    f
}

/// The rules over one metadata per target (the host's first).
pub fn check(graph: &Graph, targets: &[(String, Metadata)], flags: &Flags) -> StepResult {
    let mut step = StepResult::new("deps");
    let Some((_, host)) = targets.first() else {
        return step;
    };
    let members: BTreeSet<String> = host.members().iter().map(|p| p.name.clone()).collect();
    for m in &members {
        if !graph.crates.contains_key(m) {
            step.error(Problem::new(
                "deps.crate_unlisted",
                format!("{m} is a workspace member but has no [crate.{m}] in {CONFIG} (architecture.md 5)"),
                json!({"crate": m}),
            ));
        }
    }
    for (name, entry) in &graph.crates {
        if !entry.planned && !members.contains(name) {
            step.error(Problem::new(
                "deps.crate_missing",
                format!("{CONFIG} lists {name}, which is not a workspace member"),
                json!({"crate": name}),
            ));
        }
    }
    // Edges between workspace crates, of every kind, as the manifests declare them.
    for p in host.members() {
        let Some(entry) = graph.crates.get(&p.name) else {
            continue;
        };
        let mut seen = BTreeSet::new();
        for d in &p.dependencies {
            if !members.contains(&d.name) || d.name == p.name {
                continue;
            }
            let kind = d.kind.clone().unwrap_or_else(|| "normal".into());
            let allowed =
                entry.deps.contains(&d.name) || graph.crates.get(&d.name).is_some_and(|e| e.any);
            if !allowed && seen.insert((d.name.clone(), kind.clone())) {
                step.error(Problem::new(
                    "deps.edge_not_allowed",
                    format!(
                        "{} may not depend on {} ({kind}); architecture.md 5 allows {:?}",
                        p.name, d.name, entry.deps
                    ),
                    json!({"from": p.name, "to": d.name, "kind": kind}),
                ));
            }
        }
    }
    let mut reported = BTreeSet::new();
    for (target, meta) in targets {
        for p in meta.members() {
            let Some(entry) = graph.crates.get(&p.name) else {
                continue;
            };
            let Some(group) = graph.group.get(&entry.group) else {
                continue;
            };
            for (dep, chain) in meta.normal_closure(&p.id) {
                if group.forbid.contains(&dep) && reported.insert((p.name.clone(), dep.clone())) {
                    step.error(Problem::new(
                        "deps.external_not_allowed",
                        format!("{} (group {}) reaches {dep} on {target}: {}", p.name, entry.group, chain.join(" -> ")),
                        json!({"crate": p.name, "dependency": dep, "chain": chain, "target": target}),
                    ));
                }
            }
            for dep in meta.direct_normal(&p.id) {
                if group.forbid_direct.contains(&dep)
                    && reported.insert((p.name.clone(), dep.clone()))
                {
                    step.error(Problem::new(
                        "deps.external_not_allowed",
                        format!("{} (group {}) may not depend on {dep} directly", p.name, entry.group),
                        json!({"crate": p.name, "dependency": dep, "chain": [p.name, dep], "target": target}),
                    ));
                }
            }
        }
        for (package, rule) in &graph.features {
            for features in meta.features_of(package) {
                for f in rule.forbid.iter().filter(|f| features.contains(f)) {
                    step.error(Problem::new(
                        "deps.feature_not_allowed",
                        format!("{package} is built with feature {f} on {target}, which {CONFIG} forbids"),
                        json!({"package": package, "feature": f, "target": target}),
                    ));
                }
                for f in rule.require.iter().filter(|f| !features.contains(f)) {
                    step.error(Problem::new(
                        "deps.feature_missing",
                        format!("{package} is built without feature {f} on {target}, which {CONFIG} requires"),
                        json!({"package": package, "feature": f, "target": target}),
                    ));
                }
            }
        }
    }
    let want = &graph.build.cflags_must_contain;
    if !flags.cflags.iter().any(|(s, _)| s.starts_with(".cargo")) {
        step.error(Problem::new(
            "deps.cflags_missing",
            format!(".cargo/config.toml sets no [env] CFLAGS; it must contain {want} (architecture.md 7.3)"),
            json!({"found": null}),
        ));
    }
    for (source, value) in &flags.cflags {
        if !value.split_whitespace().any(|w| w == want) {
            step.error(Problem::new(
                "deps.cflags_missing",
                format!("{source} is '{value}', without {want} (architecture.md 7.3)"),
                json!({"found": value, "source": source}),
            ));
        }
    }
    for (source, value) in &flags.rustflags {
        if let Some(bad) = graph
            .build
            .rustflags_forbid
            .iter()
            .find(|b| value.contains(b.as_str()))
        {
            step.error(Problem::new(
                "deps.rustflags_not_allowed",
                format!("{source} holds {bad}: '{value}' (architecture.md 7.3)"),
                json!({"source": source, "value": value}),
            ));
        }
    }
    let externals: BTreeSet<&str> = host
        .built()
        .into_iter()
        .map(|id| host.name_of(id))
        .filter(|name| !members.contains(*name))
        .collect();
    step.measure("deps.crates", members.len() as f64, "count");
    step.measure("deps.external", externals.len() as f64, "count");
    step.summary = format!(
        "{} workspace crates, {} external packages, {} targets",
        members.len(),
        externals.len(),
        targets.len()
    );
    step
}

/// Runs `cargo metadata` for the host and the web target and checks the rules.
pub fn run(env: &Env, graph: &Graph, log: &std::path::Path) -> (StepResult, Option<Metadata>) {
    let mut targets = Vec::new();
    for target in [env.host.clone(), WEB_TARGET.to_string()] {
        let cmd = env.cargo([
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--filter-platform",
            &target,
        ]);
        let line = run::describe(&cmd);
        match run::run_stdout(cmd, log) {
            Ok(out) if out.ok() => match serde_json::from_str::<Metadata>(&out.text) {
                Ok(meta) => targets.push((target, meta)),
                Err(e) => return (failed(&line, &e.to_string()), None),
            },
            Ok(out) => {
                // The log's last lines that cargo printed, without the command headers xtask writes.
                let text = std::fs::read_to_string(log).unwrap_or_default();
                let lines: Vec<&str> = text
                    .lines()
                    .filter(|l| !l.starts_with("$ ") && !l.starts_with("# "))
                    .collect();
                let tail = &lines[lines.len().saturating_sub(12)..];
                return (
                    failed(&line, &format!("exit {:?}\n{}", out.code, tail.join("\n"))),
                    None,
                );
            }
            Err(e) => {
                let mut step = StepResult::new("deps");
                step.problem(run::start_failed(&line, &e));
                return (step, None);
            }
        }
    }
    let mut step = check(graph, &targets, &flags(&env.root));
    if env.root.join(VENDORED).is_dir() {
        vendor(env, &mut step, log);
    }
    let host = targets.into_iter().next().map(|(_, m)| m);
    (step, host)
}

/// The vendored, patched QuickJS-ng (architecture.md 7.5).
pub const VENDORED: &str = "third_party/rquickjs-sys-0.14.0";

/// `deps.vendor_stale {path}` for each file of [`VENDORED`] that differs from what
/// `third_party/vendor.py --check --offline` rebuilds from the pinned crate and patches. Without
/// the pinned crate in cargo's cache (exit 3) the comparison cannot run, which is a warning, since
/// the check fetches nothing.
fn vendor(env: &Env, step: &mut StepResult, log: &std::path::Path) {
    let Some(python) = run::python() else {
        step.inconclusive(Problem::new(
            "check.tool_missing",
            "no Python 3 to run third_party/vendor.py --check (python or python3, or POCKET_PYTHON)",
            json!({"tool": "python"}),
        ));
        return;
    };
    let mut cmd = std::process::Command::new(python);
    cmd.args(["third_party/vendor.py", "--check", "--offline"])
        .current_dir(&env.root)
        .env("PYTHONUTF8", "1");
    let line = run::describe(&cmd);
    let out = match run::run_merged(cmd, log, &mut |_| {}) {
        Ok(o) => o,
        Err(e) => {
            step.problem(run::start_failed(&line, &e));
            return;
        }
    };
    let stale = vendor_stale(&out.text);
    for path in &stale {
        step.error(Problem::new(
            "deps.vendor_stale",
            format!(
                "{path} differs from what the pinned crate and third_party/patches/ give; \
                 run python third_party/vendor.py, or record the change as a patch"
            ),
            json!({"path": path}),
        ));
    }
    match out.code {
        Some(0) => step.summary.push_str("; QuickJS-ng as its pins and patches give it"),
        Some(3) => step.warn(Problem::new(
            "deps.vendor_unchecked",
            "the pinned rquickjs-sys crate is not in cargo's cache, so the vendored QuickJS-ng is not compared; run python third_party/vendor.py --check once",
            json!({"tail": out.tail(3)}),
        )),
        _ if stale.is_empty() => step.error(Problem::new(
            "check.command_failed",
            format!("{line} failed (exit {:?})", out.code),
            json!({"command": line, "status": out.code, "tail": out.tail(20)}),
        )),
        _ => {}
    }
}

/// The paths `vendor.py --check` names as `deps.vendor_stale: <dir>/<file>`, from the root.
pub fn vendor_stale(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|l| l.strip_prefix("deps.vendor_stale: "))
        .map(|p| format!("third_party/{}", p.trim()))
        .collect()
}

fn failed(command: &str, message: &str) -> StepResult {
    let mut step = StepResult::new("deps");
    step.error(Problem::new(
        "check.command_failed",
        format!("{command} failed"),
        json!({"command": command, "output": message}),
    ));
    step.summary = "cargo metadata failed".into();
    step
}

#[cfg(test)]
#[path = "deps_tests.rs"]
mod tests;
