//! Tests of the generators and the `shared/SYNC.toml` record.

use super::*;
use crate::report::Verdict;
use std::fs;
use std::path::PathBuf;

/// The repository, found at run time: `env!` would bake in the directory the test binary was
/// built in, and cargo reuses that binary for a copy of the workspace with the same layout.
fn repo() -> PathBuf {
    PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo test sets it"))
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
fn sha256_matches_the_fips_vector() {
    assert_eq!(
        sha256(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        sha256(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn modified_new_and_gone_files_are_named() {
    let sync = Sync {
        commit: String::new(),
        files: BTreeMap::from([
            ("contract/a.md".to_string(), "11".to_string()),
            ("contract/b.md".to_string(), "22".to_string()),
            ("contract/gone.md".to_string(), "33".to_string()),
        ]),
    };
    let now = BTreeMap::from([
        ("contract/a.md".to_string(), "11".to_string()),
        ("contract/b.md".to_string(), "99".to_string()),
        ("contract/new.md".to_string(), "44".to_string()),
    ]);
    let found: Vec<(String, Option<String>, Option<String>)> = shared_problems(&sync, &now)
        .into_iter()
        .map(|p| {
            assert_eq!(p.code, "gen.shared_modified");
            let s = |k: &str| p.detail[k].as_str().map(String::from);
            (s("path").unwrap(), s("recorded"), s("actual"))
        })
        .collect();
    assert_eq!(
        found,
        [
            (
                "shared/contract/b.md".into(),
                Some("22".into()),
                Some("99".into())
            ),
            ("shared/contract/gone.md".into(), Some("33".into()), None),
            ("shared/contract/new.md".into(), None, Some("44".into())),
        ]
    );
}

#[test]
fn hashes_over_a_tree_catch_an_edit() {
    let root = std::env::temp_dir().join(format!("xtask-sync-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("shared/contract")).unwrap();
    fs::write(root.join("shared/contract/a.md"), "a\n").unwrap();
    fs::write(root.join("shared/SYNC.toml"), "x").unwrap();
    let listed = vec![
        "shared/SYNC.toml".to_string(),
        "shared/contract/a.md".to_string(),
    ];
    let before = shared_hashes(&root, &listed);
    assert_eq!(before.keys().collect::<Vec<_>>(), ["contract/a.md"]);
    let sync = Sync {
        commit: String::new(),
        files: before,
    };
    assert!(shared_problems(&sync, &shared_hashes(&root, &listed)).is_empty());
    fs::write(root.join("shared/contract/a.md"), "a, edited\n").unwrap();
    let problems = shared_problems(&sync, &shared_hashes(&root, &listed));
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].detail["path"], json!("shared/contract/a.md"));
    let _ = fs::remove_dir_all(&root);
}

/// Rewriting the committed record with its own hashes gives it back byte for byte, so
/// `cargo xtask gen --shared` changes only what changed.
#[test]
fn the_record_round_trips() {
    let old = fs::read_to_string(repo().join(SYNC)).unwrap();
    let sync: Sync = config::parse(SYNC, &old).unwrap();
    assert_eq!(sync_text(&old, &sync.commit, &sync.files), old);
}

#[test]
fn every_crate_gets_the_same_clippy_toml_and_it_parses() {
    let source: ClippySource = config::load(&repo(), CLIPPY_SOURCE).unwrap();
    let outs = clippy_outputs(&source);
    assert_eq!(outs.len(), source.crates.len());
    assert!(outs.iter().all(|o| o.text == outs[0].text));
    assert!(files::is_generated(outs[0].text.as_bytes()));
    let parsed: toml::Table = outs[0].text.parse().unwrap();
    assert_eq!(
        parsed["disallowed-methods"].as_array().unwrap().len(),
        source.methods.len()
    );
    assert_eq!(
        parsed["disallowed-types"].as_array().unwrap().len(),
        source.types.len()
    );
    // The lists of simulation.md 10 and numeric.md 5.
    for path in [
        "std::time::Instant::now",
        "f64::sin",
        "f32::powf",
        "f64::mul_add",
        "bevy_ecs::system::Query::iter",
    ] {
        assert!(source.methods.iter().any(|e| e.path == path), "{path}");
    }
    assert!(
        source
            .types
            .iter()
            .any(|e| e.path == "std::collections::HashMap")
    );
}

#[test]
fn a_stale_copy_and_an_orphan_fail_the_step() {
    let root = std::env::temp_dir().join(format!("xtask-gen-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("tools")).unwrap();
    fs::create_dir_all(root.join("crates/pocket-sim")).unwrap();
    fs::create_dir_all(root.join("shared")).unwrap();
    fs::write(
        root.join(CLIPPY_SOURCE),
        "crates = [\"crates/pocket-sim\", \"crates/absent\"]\ndisallowed-methods = [{ path = \"f64::sin\", reason = \"r\" }]\ndisallowed-types = []\n",
    )
    .unwrap();
    fs::write(
        root.join("crates/pocket-sim/clippy.toml"),
        "# @generated\nold\n",
    )
    .unwrap();
    fs::write(
        root.join("crates/pocket-sim/x.rs"),
        "// @generated by hand\n",
    )
    .unwrap();
    fs::write(
        root.join("Cargo.lock"),
        "# This file is automatically @generated by Cargo.\n",
    )
    .unwrap();
    fs::write(root.join(SYNC), "commit = \"\"\n\n[files]\n").unwrap();
    let listed: Vec<String> = [
        "Cargo.lock",
        "crates/pocket-sim/clippy.toml",
        "crates/pocket-sim/x.rs",
        "shared/SYNC.toml",
    ]
    .map(String::from)
    .into();
    let step = check(&root, &listed);
    assert_eq!(step.verdict, Verdict::Fail);
    let codes: Vec<(&str, &str)> = step
        .errors
        .iter()
        .map(|p| (p.code.as_str(), p.detail["path"].as_str().unwrap()))
        .collect();
    assert_eq!(
        codes,
        [
            ("gen.stale", "crates/pocket-sim/clippy.toml"),
            ("gen.orphan", "crates/pocket-sim/x.rs")
        ]
    );
    // Writing the outputs makes the copy current; the missing crate directory is left alone.
    for o in outputs(&root).unwrap() {
        fs::write(root.join(&o.path), o.text).unwrap();
    }
    assert!(!root.join("crates/absent").exists());
    let step = check(&root, &listed);
    assert_eq!(step.errors.len(), 1, "{:?}", step.errors);
    let _ = fs::remove_dir_all(&root);
}
