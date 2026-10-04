//! Cargo orchestrates F0's actual checks; later capabilities add explicit steps here.
mod metadata;

use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    process::{Command, ExitCode},
};

const GAME: &[&str] = &[
    "pocket-contract",
    "pocket-sim",
    "pocket-persist",
    "pocket-link",
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned()
}

fn cargo() -> Command {
    let mut cmd = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cmd.current_dir(root()).env_remove("POCKET_BLESS");
    cmd
}

fn run(command: &mut Command) -> Result<(), String> {
    eprintln!("RUN {command:?}");
    let status = command.status().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{command:?} exited {status}"))
    }
}

fn deps_for(platform: &str) -> Result<(), String> {
    let bytes = cargo()
        .args([
            "metadata",
            "--locked",
            "--format-version",
            "1",
            "--filter-platform",
            platform,
        ])
        .output()
        .map_err(|e| e.to_string())?;
    if !bytes.status.success() {
        return Err(String::from_utf8_lossy(&bytes.stderr).into());
    }
    let metadata: Value = serde_json::from_slice(&bytes.stdout).map_err(|e| e.to_string())?;
    let graph: toml::Value = toml::from_str(
        &std::fs::read_to_string(root().join("tools/crate-graph.toml"))
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let declared = graph["crate"].as_table().ok_or("missing crate table")?;
    let members: BTreeSet<_> = metadata["workspace_members"]
        .as_array()
        .ok_or("missing members")?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let packages = metadata["packages"].as_array().ok_or("missing packages")?;
    let by_id: BTreeMap<_, _> = packages
        .iter()
        .map(|p| (p["id"].as_str().unwrap(), p))
        .collect();
    let names: BTreeSet<_> = members
        .iter()
        .map(|id| by_id[id]["name"].as_str().unwrap())
        .collect();
    if names != declared.keys().map(String::as_str).collect() {
        return Err("workspace and declared graph differ".into());
    }
    for id in &members {
        let p = by_id[id];
        let name = p["name"].as_str().unwrap();
        let allowed: BTreeSet<_> = declared[name]["deps"]
            .as_array()
            .ok_or("missing deps")?
            .iter()
            .filter_map(toml::Value::as_str)
            .collect();
        for dep in p["dependencies"].as_array().ok_or("missing dependencies")? {
            let dep = dep["name"].as_str().unwrap();
            if names.contains(dep) && !allowed.contains(dep) {
                return Err(format!("forbidden edge {name} -> {dep}"));
            }
        }
    }
    let resolved: metadata::Metadata =
        serde_json::from_slice(&bytes.stdout).map_err(|e| e.to_string())?;
    for features in resolved.features_of("bevy_ecs") {
        if features
            .iter()
            .any(|f| matches!(f.as_str(), "multi_threaded" | "serialize"))
        {
            return Err("game ECS enables a forbidden simulation feature".into());
        }
    }
    let forbidden: BTreeSet<_> = graph["game"]["forbid"]
        .as_array()
        .ok_or("missing forbidden deps")?
        .iter()
        .filter_map(toml::Value::as_str)
        .collect();
    for id in &members {
        let name = by_id[id]["name"].as_str().unwrap();
        if !declared[name]["game"].as_bool().unwrap_or(false) {
            continue;
        }
        for dependency in resolved.direct_normal(id) {
            if forbidden.contains(dependency.as_str()) {
                return Err(format!("{name} directly depends on forbidden {dependency}"));
            }
        }
        for (dependency, chain) in resolved.normal_closure(id) {
            if forbidden.contains(dependency.as_str()) {
                return Err(format!(
                    "{name} reaches forbidden {dependency}: {}",
                    chain.join(" -> ")
                ));
            }
        }
    }
    println!(
        "PASS deps ({platform}): {} workspace crates, {} active packages, game/presenter separation",
        resolved.members().len(),
        resolved.built().len()
    );
    Ok(())
}

fn deps() -> Result<(), String> {
    let output = Command::new("rustc")
        .arg("-vV")
        .current_dir(root())
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("rustc could not identify the host".into());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let host = text
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .ok_or("rustc reported no host")?;
    for platform in [host, "wasm32-unknown-unknown"] {
        deps_for(platform)?;
    }
    Ok(())
}

fn check() -> Result<(), String> {
    if std::env::var_os("POCKET_BLESS").is_some() {
        return Err("POCKET_BLESS must be unset: checks never rewrite golden data".into());
    }
    deps()?;
    run(cargo().args(["fmt", "--all", "--", "--check"]))?;
    run(cargo().args([
        "clippy",
        "--workspace",
        "--all-targets",
        "--locked",
        "--",
        "-D",
        "warnings",
    ]))?;
    run(cargo().args(["test", "--workspace", "--locked", "--no-fail-fast"]))?;
    for seed in ["1", "7", "42"] {
        run(cargo().args([
            "run",
            "--quiet",
            "--locked",
            "-p",
            "pocket-app",
            "--",
            "foundation",
            "--seed",
            seed,
            "--ticks",
            "600",
        ]))?;
    }
    let mut wasm = cargo();
    wasm.args([
        "check",
        "--locked",
        "--target",
        "wasm32-unknown-unknown",
        "--lib",
    ]);
    for name in GAME {
        wasm.args(["-p", name]);
    }
    run(&mut wasm)?;
    // Each package has a web_vectors example. Build and run them in sequence: Cargo's public
    // examples/web_vectors.wasm name is shared, so do not run after the other package overwrote it.
    let target = metadata_target()?;
    for name in ["pocket-sim", "pocket-persist"] {
        run(cargo().args([
            "build",
            "--locked",
            "-p",
            name,
            "--example",
            "web_vectors",
            "--target",
            "wasm32-unknown-unknown",
            "--release",
        ]))?;
        run(Command::new("node")
            .current_dir(root())
            .arg(format!("crates/{name}/tests/web/run.mjs"))
            .arg(target.join("wasm32-unknown-unknown/release/examples/web_vectors.wasm"))
            .arg(target.join(format!("tmp/{name}-web-report.txt"))))?;
    }
    println!("PASS F0: deps, fmt, clippy, native tests/scenario, wasm build and vector parity");
    Ok(())
}

fn metadata_target() -> Result<PathBuf, String> {
    let out = cargo()
        .args(["metadata", "--locked", "--no-deps", "--format-version", "1"])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("cargo metadata failed".into());
    }
    let value: metadata::Metadata =
        serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    if value.target_directory.is_empty() {
        Err("no target directory".into())
    } else {
        Ok(PathBuf::from(value.target_directory))
    }
}

fn main() -> ExitCode {
    if std::env::args().skip(1).collect::<Vec<_>>() != ["check"] {
        eprintln!("usage: cargo xtask check");
        return ExitCode::from(2);
    }
    match check() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("FAIL {error}");
            ExitCode::FAILURE
        }
    }
}
