//! The web report natively (script-host.md 12, tests 1, 2, 3 and 9; the cross-target check of the
//! script host): it must have no failure, its values are pinned, and the WebAssembly build must
//! print the same bytes, which `the_wasm_report_is_the_native_one` checks under Node. By hand:
//!
//!   cargo build -p pocket-script --example web_workload --target wasm32-unknown-unknown --release
//!   node crates/pocket-script/tests/web/run.mjs <target>/wasm32-unknown-unknown/release/examples/web_workload.wasm \
//!       <target>/tmp/pocket-script-web-set.json <target>/tmp/pocket-script-web-report.txt
#![cfg(feature = "transpile")]

use std::path::{Path, PathBuf};
use std::process::Command;

use pocket_script::{ScriptSource, compile};

/// The replaced `Math` functions' sweep, checked against the Rust library and pinned (test 2).
const MATH_REPLACED: &str = "ok math.replaced 2837a30d";
/// The kept operations' hashes (numbers, text) and the text's length: the same natively and on
/// the web (script-host.md 12, test 2).
const MATH_KEPT: &str = "math.kept [105198831,869719725,36862]";
/// The workload's world digest after `TICKS` ticks and the steps of all its calls (test 3).
const FINAL: &str = "final 507e8ac55b06797f steps 18654424";

/// Ticks of the workload in the report.
pub const TICKS: u64 = 600;

fn workload() -> pocket_script::CompiledSet {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/web/workload");
    let src = ScriptSource::read_dir(&root).unwrap();
    compile(&src, &Default::default()).unwrap_or_else(|e| panic!("{e:#?}"))
}

/// The report on a thread with the stack the host asks for (its depth check recurses to the limit).
fn report(json: &str) -> String {
    let json = json.to_owned();
    std::thread::Builder::new()
        .stack_size(pocket_script::host::THREAD_STACK_BYTES)
        .spawn(move || pocket_script::web::report(&json, TICKS))
        .unwrap()
        .join()
        .unwrap()
}

#[test]
fn web_report() {
    let set = workload();
    let json = set.to_json();
    let report = report(&json);
    let tmp = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::write(tmp.join("pocket-script-web-set.json"), &json).unwrap();
    std::fs::write(tmp.join("pocket-script-web-report.txt"), &report).unwrap();
    assert!(!report.contains("FAIL"), "{report}");
    // A second run in the same process, and a third from the JSON round trip, give the same bytes.
    assert_eq!(report, self::report(&json));
    let reread = pocket_script::CompiledSet::from_json(&json).unwrap();
    assert_eq!(reread, set);
    let lines: Vec<&str> = report.lines().collect();
    assert!(
        lines[0].starts_with("ok lockdown problems 0 "),
        "{}",
        lines[0]
    );
    assert_eq!(lines[1], MATH_REPLACED);
    assert_eq!(lines[2], "ok nan.canonical");
    assert_eq!(lines[3], MATH_KEPT);
    assert_eq!(lines[4], "ok depth 1000 script.call_depth");
    assert_eq!(lines[5], "ok depth 65 ok");
    assert_eq!(lines[6], "ok proxies 2000 script.call_depth");
    assert_eq!(lines[7], "ok proxies 150 ok");
    // The workload did things: crates were taken, and no system failed.
    assert!(
        report
            .lines()
            .filter(|l| l.starts_with("tick "))
            .all(|l| l.ends_with("errors 0")),
        "{report}"
    );
    let last = lines.last().unwrap();
    println!("{}\n{}\n{}\n{last}", lines[0], lines[1], lines[3]);
    assert_eq!(*last, FINAL);
}

/// The rustc beside the cargo running this test, which knows its toolchain's targets.
fn rustc() -> PathBuf {
    Path::new(env!("CARGO")).with_file_name(if cfg!(windows) { "rustc.exe" } else { "rustc" })
}

fn has_wasm_target() -> bool {
    Command::new(rustc())
        .args([
            "--print",
            "target-libdir",
            "--target",
            "wasm32-unknown-unknown",
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .is_some_and(|dir| {
            Path::new(dir.trim())
                .read_dir()
                .is_ok_and(|mut d| d.next().is_some())
        })
}

/// Cross-target determinism (script-host.md 12, tests 1 to 3 and 9 marked web): the example
/// `web_workload` built for `wasm32-unknown-unknown` prints, under Node, the very bytes of the native
/// report. It builds in its own target directory under `CARGO_TARGET_TMPDIR`, since the target
/// directory running this test may be locked. Without Node or the target it says so and passes.
#[test]
fn the_wasm_report_is_the_native_one() {
    if Command::new("node").arg("--version").output().is_err() {
        println!("skipped: node is not on PATH");
        return;
    }
    if !has_wasm_target() {
        println!("skipped: the wasm32-unknown-unknown target is not installed");
        return;
    }
    let set = workload();
    let json = set.to_json();
    let native = report(&json);
    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let set_path = tmp.join("pocket-script-wasm-set.json");
    let report_path = tmp.join("pocket-script-wasm-native-report.txt");
    std::fs::write(&set_path, &json).unwrap();
    std::fs::write(&report_path, &native).unwrap();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let target_dir = tmp.join("web-target");
    let built = Command::new(env!("CARGO"))
        .current_dir(manifest)
        .args([
            "build",
            "--package",
            "pocket-script",
            "--example",
            "web_workload",
            "--target",
            "wasm32-unknown-unknown",
            "--release",
            "--target-dir",
        ])
        .arg(&target_dir)
        .status()
        .unwrap();
    assert!(built.success(), "the wasm32 build of web_workload failed");
    let wasm = target_dir.join("wasm32-unknown-unknown/release/examples/web_workload.wasm");
    let ran = Command::new("node")
        .arg(manifest.join("tests/web/run.mjs"))
        .arg(&wasm)
        .arg(&set_path)
        .arg(&report_path)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&ran.stderr);
    println!("{stderr}");
    assert!(ran.status.success(), "{stderr}");
    assert_eq!(String::from_utf8_lossy(&ran.stdout), native);
}

#[test]
fn exponent_lowers_to_the_engine_pow() {
    // `**` is lowered to Math.pow by the transpiler, so it reaches the engine library, not libm.
    let src = ScriptSource::new().with(
        "scripts/main.ts",
        "import { game } from \"pocket\";\nexport const r = 2 ** 0.5;\nexport function f(x: number) { let y = x; y **= 3; return y; }\nexport default game({ systems: [] });\n",
    );
    let set = compile(&src, &Default::default()).unwrap();
    let js = &set.module("scripts/main.ts").unwrap().js;
    assert!(
        js.contains("Math.pow(2, .5)") || js.contains("Math.pow(2, 0.5)"),
        "{js}"
    );
    assert!(!js.contains("**"), "{js}");
}
