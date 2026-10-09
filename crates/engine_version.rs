//! The engine's identity, computed by `pocket-app`'s and `pocket-web`'s build scripts with this one
//! file, so the native and the web build of one tree report one `EngineVersion.source`
//! (docs/spec/versions.md 3.1 and V9; architecture.md 4.13). It is emitted as `POCKET_ENGINE_*`
//! variables, which `pocket_runtime::install_engine_version!` reads where the binary starts.
//!
//! It sits beside the crates and belongs to none: each build script includes it with `#[path]`, so
//! neither reaches into another crate's directory, and `tools/crate-graph.toml` has no edge to draw
//! (architecture.md 4.13). It is under `crates/`, so the source hash covers it.
//!
//! `source` is `BLAKE3-derive_key("Amoris 2026-10-03 engine source v1", per file in path order:
//! ULEB128(len path) || path || BLAKE3(content))` over the files git lists (tracked, and untracked
//! but not ignored) under the paths compiled into the engine, the definition
//! `pocket_persist::version::source_hash` implements; `pocket-app`'s test `engine_version` holds the
//! two equal.

use std::path::{Path, PathBuf};
use std::process::Command;

/// `pocket_persist::version::SOURCE_CONTEXT`.
const SOURCE_CONTEXT: &str = "Amoris 2026-10-03 engine source v1";
/// The directories compiled into the engine (architecture.md 4.13): all of `third_party/`, not
/// only the vendored crate and its patches, so whatever is vendored there next is covered too.
const DIRS: &[&str] = &["crates/", "shared/contract/rust/", "third_party/"];
/// And the files.
const FILES: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    ".cargo/config.toml",
];

fn var(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Whether a path from `git ls-files` is part of the engine's source.
pub fn in_engine(path: &str) -> bool {
    DIRS.iter().any(|d| path.starts_with(d)) || FILES.contains(&path)
}

fn uleb(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// The source hash over `paths` (relative to `root`, `/`-separated), as 64 hex digits; a listed
/// file that is gone from the working tree (deleted, not yet committed) is not compiled, so it is
/// left out.
pub fn source_hash(root: &Path, paths: &[String]) -> String {
    let mut sorted: Vec<&String> = paths.iter().collect();
    sorted.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    let mut h = blake3::Hasher::new_derive_key(SOURCE_CONTEXT);
    let mut buf = Vec::new();
    for path in sorted {
        let Ok(content) = std::fs::read(root.join(path)) else {
            continue;
        };
        buf.clear();
        uleb(&mut buf, path.len() as u64);
        buf.extend_from_slice(path.as_bytes());
        buf.extend_from_slice(blake3::hash(&content).as_bytes());
        h.update(&buf);
    }
    h.finalize().to_hex().to_string()
}

/// The engine's files as git lists them, or None without git or a checkout.
pub fn listed(root: &Path) -> Option<Vec<String>> {
    let text = git(
        root,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    )?;
    let mut paths: Vec<String> = text
        .split('\0')
        .filter(|p| !p.is_empty() && in_engine(p))
        .map(str::to_owned)
        .collect();
    paths.sort();
    paths.dedup();
    Some(paths)
}

/// The shared contract's version and its reference commit, `unsynced` while `shared/SYNC.toml`
/// records none: `0.1+unsynced`.
fn contract(root: &Path) -> String {
    let readme =
        std::fs::read_to_string(root.join("shared/contract/README.md")).unwrap_or_default();
    let version = readme
        .lines()
        .find_map(|l| {
            l.trim_start_matches("- ")
                .strip_prefix("Contract version: ")
        })
        .and_then(|v| v.split_whitespace().next())
        .unwrap_or("unknown")
        .to_owned();
    let sync = std::fs::read_to_string(root.join("shared/SYNC.toml")).unwrap_or_default();
    let commit = sync
        .lines()
        .find_map(|l| l.trim().strip_prefix("commit"))
        .map(|rest| {
            rest.trim_start()
                .trim_start_matches('=')
                .trim()
                .trim_matches('"')
        })
        .filter(|c| !c.is_empty())
        .unwrap_or("unsynced")
        .to_owned();
    format!("{version}+{commit}")
}

/// The cargo profile's name, `release`, `debug` or `web`: the directory above `build/` in
/// `OUT_DIR` (`target/<triple>/<profile>/build/<package>/out`), since `PROFILE` names only the
/// profile a custom one inherits from.
fn profile() -> String {
    let out = PathBuf::from(var("OUT_DIR"));
    out.ancestors()
        .nth(3)
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| var("PROFILE"))
}

/// Paths whose change must rerun the build script, all existing (cargo reruns a script on every
/// build while a named path is missing):
///
/// - the engine's sources and the files that decide which of them git lists: the root
///   `.gitignore` (nested ones are inside the watched directories) and the repository's
///   `info/exclude`;
/// - the contract's version: `shared/contract/README.md` and `shared/SYNC.toml`;
/// - for the commit, where git keeps `HEAD` and the branch it names, resolved by
///   `git rev-parse --git-path` (a worktree's `HEAD` is its own, its branches and `packed-refs`
///   are the common directory's): `HEAD`, the branch's loose ref, and `packed-refs`. A packed
///   branch has no loose ref until its next commit writes one, so the nearest existing directory
///   above it is watched instead, whose new file reruns the script. A reftable repository keeps
///   every ref in `reftable/`, which is watched whole.
pub fn watched(root: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = DIRS
        .iter()
        .chain(FILES)
        .map(|p| root.join(p.trim_end_matches('/')))
        .collect();
    paths.push(root.join(".gitignore"));
    paths.push(root.join("shared/contract/README.md"));
    paths.push(root.join("shared/SYNC.toml"));
    let git_path =
        |name: &str| git(root, &["rev-parse", "--git-path", name]).map(|p| root.join(p.trim()));
    for name in ["HEAD", "packed-refs", "reftable", "info/exclude"] {
        paths.extend(git_path(name));
    }
    if let Some(branch) = git(root, &["symbolic-ref", "-q", "HEAD"])
        && let Some(loose) = git_path(branch.trim())
    {
        // The loose ref, or the nearest directory above it that exists.
        paths.extend(
            loose
                .ancestors()
                .find(|p| p.exists())
                .map(Path::to_path_buf),
        );
    }
    paths.retain(|p| p.exists());
    paths.sort();
    paths.dedup();
    paths
}

fn watch(root: &Path) {
    for p in watched(root) {
        println!("cargo:rerun-if-changed={}", p.display());
    }
}

/// Emits `POCKET_ENGINE_SOURCE`, `_COMMIT`, `_TARGET`, `_PROFILE` and `_CONTRACT`.
pub fn emit() {
    let manifest = PathBuf::from(var("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(Path::parent)
        .expect("the crate sits in <root>/crates/")
        .to_path_buf();
    watch(&root);
    let (source, commit) = match listed(&root) {
        Some(paths) => {
            let commit = git(&root, &["rev-parse", "HEAD"])
                .map(|c| c.trim().to_owned())
                .filter(|c| c.len() == 40)
                .unwrap_or_else(|| "unknown".to_owned());
            (source_hash(&root, &paths), commit)
        }
        None => {
            println!(
                "cargo:warning=git ls-files failed in {}: EngineVersion.source is the hash over no \
                 files, as an unbuilt engine's (docs/spec/versions.md 3.1)",
                root.display()
            );
            (source_hash(&root, &[]), "unknown".to_owned())
        }
    };
    println!("cargo:rustc-env=POCKET_ENGINE_SOURCE={source}");
    println!("cargo:rustc-env=POCKET_ENGINE_COMMIT={commit}");
    println!("cargo:rustc-env=POCKET_ENGINE_TARGET={}", var("TARGET"));
    println!("cargo:rustc-env=POCKET_ENGINE_PROFILE={}", profile());
    println!("cargo:rustc-env=POCKET_ENGINE_CONTRACT={}", contract(&root));
}
