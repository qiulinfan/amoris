//! Process-level checks of the executable slice, including two negative controls.
use serde_json::Value;
use std::process::Command;

fn invoke(args: &[&str]) -> (i32, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_pocket"))
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        serde_json::from_slice(&out.stdout).unwrap(),
    )
}

#[test]
fn assembled_foundation_runs_and_identifies_the_build() {
    let (code, report) = invoke(&["foundation", "--ticks", "12", "--seed", "17", "--json"]);
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["negative_divergence_tick"], 4);
    assert_eq!(report["checks"].as_array().unwrap().len(), 10);
    assert_ne!(report["source"]["commit"], "unknown");
    assert_eq!(report["source"]["contract"], "pocketengine-foundation-v1");
}

#[test]
fn separate_processes_repeat_hashes_and_seed_changes_state() {
    let (_, a) = invoke(&["foundation", "--ticks", "12", "--seed", "17", "--json"]);
    let (_, b) = invoke(&["foundation", "--ticks", "12", "--seed", "17", "--json"]);
    let (_, c) = invoke(&["foundation", "--ticks", "12", "--seed", "18", "--json"]);
    assert_eq!(a["world_hash"], b["world_hash"]);
    assert_ne!(a["world_hash"], c["world_hash"]);
}

#[test]
fn invalid_request_is_refused_before_running() {
    for args in [
        vec!["foundation", "--ticks", "0"],
        vec!["foundation", "--tiks", "12"],
    ] {
        let (code, report) = invoke(&args);
        assert_eq!(code, 2);
        assert_eq!(report["error"]["code"], "check.usage");
    }
}
