//! The engine version `pocket` installs at startup (docs/spec/versions.md 3.1, architecture.md
//! 4.13): its source is pocket-persist's `source_hash` over the files git lists under the paths
//! compiled into the engine, not the unbuilt engine's; its commit, profile and C compiler are this
//! build's; and a replay another engine recorded is refused by `pocket replay --verify` with
//! `version.mismatch` naming the engine source, its compiled modules compiled again rather than
//! reused (replay.md 2.4).

use std::path::{Path, PathBuf};
use std::process::Command;

use pocket_runtime::version::{BuiltEngine, QJS_CC, install_engine, source_hash};
use serde_json::{Value, json};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(root())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn pocket(args: &[&str]) -> (Value, Option<i32>) {
    let out = Command::new(env!("CARGO_BIN_EXE_pocket"))
        .args(args)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let v = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
    (v, out.status.code())
}

#[test]
fn pocket_reports_the_source_it_was_built_from() {
    let (v, code) = pocket(&["version"]);
    assert_eq!(code, Some(0));
    let Some(listed) = git(&[
        "ls-files",
        "-z",
        "--cached",
        "--others",
        "--exclude-standard",
    ]) else {
        eprintln!("no git checkout: the source is the unbuilt engine's, not compared");
        return;
    };
    // versions.md 3.1: crates/, shared/contract/rust/, third_party/ and the workspace's own files.
    let files: Vec<(String, Vec<u8>)> = listed
        .split('\0')
        .filter(|p| {
            ["crates/", "shared/contract/rust/", "third_party/"]
                .iter()
                .any(|d| p.starts_with(d))
                || [
                    "Cargo.toml",
                    "Cargo.lock",
                    "rust-toolchain.toml",
                    ".cargo/config.toml",
                ]
                .contains(p)
        })
        .filter_map(|p| Some((p.to_owned(), std::fs::read(root().join(p)).ok()?)))
        .collect();
    assert!(files.len() > 300, "{} files", files.len());
    assert_eq!(v["source"], json!(source_hash(&files).to_hex()), "{v}");
    assert_ne!(v["source"], json!(source_hash(&[]).to_hex()));
    let head = git(&["rev-parse", "HEAD"]).unwrap();
    assert_eq!(v["commit"], json!(head.trim()));
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    assert_eq!(v["profile"], json!(profile));
    assert!(
        v["target"]
            .as_str()
            .unwrap()
            .starts_with(std::env::consts::ARCH)
    );
    assert_eq!(v["c_compiler"], json!(QJS_CC));
    assert!(v["contract"].as_str().unwrap().contains('+'), "{v}");
}

#[test]
fn a_replay_of_another_engine_is_refused() {
    // This test process records as another engine would.
    let other = "77".repeat(32);
    install_engine(&BuiltEngine {
        semver: "0.0.0",
        commit: "unknown",
        source: &other,
        target: "test",
        profile: "test",
        contract: "0.1+unsynced",
    })
    .unwrap();
    let bytes = std::thread::Builder::new()
        .stack_size(pocket_runtime::GAME_STACK_BYTES)
        .spawn(|| {
            let subject =
                pocket_check::runs::Subject::load(&root().join("samples/sailing")).unwrap();
            pocket_check::replay::record(&subject, 1).unwrap()
        })
        .unwrap()
        .join()
        .unwrap();
    let dir = std::env::temp_dir().join(format!("pocket-engine-version-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("other-engine.replay");
    std::fs::write(&file, &bytes).unwrap();
    let (v, code) = pocket(&["replay", "--verify", file.to_str().unwrap(), "--json"]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(code, Some(1), "{v}");
    assert_eq!(v["identical"], json!(false), "{v}");
    assert_eq!(v["stopped"]["code"], json!("version.mismatch"), "{v}");
    let message = v["stopped"]["message"].as_str().unwrap();
    assert!(message.contains("engine source"), "{message}");
}
