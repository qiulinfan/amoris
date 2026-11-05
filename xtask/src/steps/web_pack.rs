//! What the web step puts beside the page before loading it (docs/spec/checks.md 7.1): the module,
//! built once for wasm32 with the `web` profile and bound by `wasm-bindgen --target web`, and once
//! more through `wasm-opt -O3`, with their sizes raw and gzipped; and for every project marked
//! `[web] workload = true`, its web package (written by `amoris-web`'s `pack` example, the stand-in
//! for `amoris pack --web`), its inputs, the native `amoris hashes` chain of every seed and the
//! replays the `replay` step recorded, listed in `expected.json` for the check page. A negative
//! control of the web page (`[expect] fail = "web.cross_target_diverged"`, checks.md 8.6) is listed
//! with its `expect`; the page reports its outcome apart and `web` judges it.

use super::deps::WEB_TARGET;
use super::diag;
use super::projects::Project;
use crate::report::{Problem, StepResult};
use crate::run::{self, Env};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A project the page runs.
pub struct Workload {
    pub dir: String,
    pub slug: String,
    pub seeds: Vec<u64>,
    pub ticks: u64,
    pub inputs: Option<String>,
    /// A negative control's expected code (checks.md 8.6).
    pub expect: Option<String>,
}

/// `samples/sailing` to `samples-sailing`, as `amoris check` names its replays.
pub fn slug(dir: &str) -> String {
    dir.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// The projects marked `[web] workload = true`: the positive ones and the web page's own negative
/// controls (`[expect] fail` with a `web.` code), not another check's controls.
pub fn workloads(projects: &[Project]) -> Vec<Workload> {
    projects
        .iter()
        .filter(|p| p.config.web.as_ref().is_some_and(|w| w.workload))
        .filter(|p| p.config.expect.is_none() || p.control_step() == Some("web"))
        .map(|p| Workload {
            dir: p.dir.clone(),
            slug: slug(&p.dir),
            seeds: p.config.run.seeds.clone(),
            ticks: p.config.run.ticks,
            inputs: p.config.run.inputs.clone(),
            expect: p.config.expect.as_ref().map(|e| e.fail.clone()),
        })
        .collect()
}

/// Runs a command into the log; a failure becomes `check.command_failed` with its first errors.
pub fn command(step: &mut StepResult, cmd: Command, log: &Path, what: &str) -> bool {
    let line = run::describe(&cmd);
    match run::run_merged(cmd, log, &mut |_| {}) {
        Ok(out) if out.ok() => true,
        Ok(out) => {
            let errors: Vec<String> = diag::all(&out.text)
                .iter()
                .filter(|d| d.level == "error")
                .map(|d| format!("{}: {}", d.at(), d.message))
                .take(10)
                .collect();
            step.error(Problem::new(
                "check.command_failed",
                format!("{what} failed (exit {:?})", out.code),
                json!({"command": line, "errors": errors, "tail": diag::plain_tail(&out.text, 20)}),
            ));
            false
        }
        Err(e) => {
            step.problem(run::start_failed(&line, &e));
            false
        }
    }
}

/// A tool from its variable, a default place, or PATH.
pub fn tool(var: &str, fallback: PathBuf, name: &str) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(var) {
        return Some(PathBuf::from(p));
    }
    if fallback.is_file() {
        return Some(fallback);
    }
    let exe = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(&exe))
            .find(|p| p.is_file())
    })
}

pub fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// The gzip size at level 9 (budgets.md 7.4), through Python's `gzip`, which the step needs anyway.
fn gzip_size(path: &Path) -> Option<u64> {
    let python = run::python()?;
    let out = Command::new(python)
        .args([
            "-c",
            "import gzip,sys; print(len(gzip.compress(open(sys.argv[1],'rb').read(), 9)))",
        ])
        .arg(path)
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

fn sizes(step: &mut StepResult, wasm: &Path, suffix: &str) {
    let raw = std::fs::metadata(wasm).map(|m| m.len()).unwrap_or(0);
    step.measure(
        &format!("web.size.wasm{suffix}"),
        raw as f64 / 1024.0,
        "KiB",
    );
    if let Some(gz) = gzip_size(wasm) {
        step.measure(
            &format!("web.size.wasm.gz{suffix}"),
            gz as f64 / 1024.0,
            "KiB",
        );
    }
}

/// Builds and binds the module into `<page>/pkg` and, when `wasm-opt` is found, the `-O3` variant
/// into `<opt>/pkg`; whether the variant was built.
pub fn module(
    env: &Env,
    step: &mut StepResult,
    target_dir: &Path,
    page: &Path,
    opt: &Path,
    log: &Path,
) -> Option<bool> {
    let cargo_bin = run::home()
        .join(".cargo/bin")
        .join(format!("wasm-bindgen{}", std::env::consts::EXE_SUFFIX));
    let Some(bindgen) = tool("AMORIS_WASM_BINDGEN", cargo_bin, "wasm-bindgen") else {
        step.inconclusive(Problem::new(
            "check.tool_missing",
            "wasm-bindgen 0.2.129 is not installed",
            json!({"tool": "wasm-bindgen"}),
        ));
        return None;
    };
    let build = env.cargo([
        "build",
        "-p",
        "amoris-web",
        "--target",
        WEB_TARGET,
        "--profile",
        "web",
        "--locked",
        "--message-format=json",
    ]);
    if !command(step, build, log, "the web build of amoris-web") {
        return None;
    }
    let wasm = target_dir
        .join(WEB_TARGET)
        .join("web")
        .join("amoris_web.wasm");
    let mut bind = Command::new(&bindgen);
    bind.args(["--target", "web", "--no-typescript", "--out-dir"])
        .arg(page.join("pkg"))
        .arg(&wasm);
    if !command(step, bind, log, "wasm-bindgen") {
        return None;
    }
    let bound = page.join("pkg").join("amoris_web_bg.wasm");
    sizes(step, &bound, "");
    let glue = std::fs::metadata(page.join("pkg/amoris_web.js")).map_or(0, |m| m.len());
    step.measure("web.size.glue", glue as f64 / 1024.0, "KiB");
    let emsdk = run::home()
        .join(".amoris-tools/emsdk/upstream/bin")
        .join(format!("wasm-opt{}", std::env::consts::EXE_SUFFIX));
    let Some(wasm_opt) = tool("AMORIS_WASM_OPT", emsdk, "wasm-opt") else {
        step.warn(Problem::new(
            "check.tool_missing",
            "no wasm-opt: the -O3 variant was not built",
            json!({"tool": "wasm-opt"}),
        ));
        return Some(false);
    };
    if let Err(e) = copy_dir(&page.join("pkg"), &opt.join("pkg")) {
        step.error(Problem::new(
            "check.command_failed",
            format!("cannot copy the bound module: {e}"),
            json!({}),
        ));
        return None;
    }
    let mut o = Command::new(wasm_opt);
    // The features wasm-bindgen's output and rustc 1.98's wasm32 defaults use.
    o.arg("-O3")
        .args([
            "--enable-bulk-memory",
            "--enable-nontrapping-float-to-int",
            "--enable-sign-ext",
            "--enable-mutable-globals",
            "--enable-multivalue",
            "--enable-reference-types",
        ])
        .arg(&bound)
        .arg("-o")
        .arg(opt.join("pkg").join("amoris_web_bg.wasm"));
    if !command(step, o, log, "wasm-opt -O3") {
        return Some(false);
    }
    let optimized = opt.join("pkg").join("amoris_web_bg.wasm");
    // The glue's start grows the table it reaches as `__wbindgen_externrefs`. Binaryen 108 (Ubuntu
    // 24.04's) moves that export onto the function table, whose maximum is its size, so the module
    // stops at `Table.grow()` in every browser.
    match externref_table_has_maximum(&optimized) {
        Ok(false) => {}
        Ok(true) => {
            step.error(Problem::new(
                "check.command_failed",
                "wasm-opt exported `__wbindgen_externrefs` as a table with a maximum, which the \
                 glue cannot grow; use a newer wasm-opt (binaryen 108 does this)",
                json!({"tool": "wasm-opt"}),
            ));
            return None;
        }
        Err(e) => {
            step.error(Problem::new(
                "check.command_failed",
                format!("cannot read the -O3 module's tables: {e}"),
                json!({}),
            ));
            return None;
        }
    }
    sizes(step, &optimized, ".O3");
    Some(true)
}

/// Whether the table a module exports as `__wbindgen_externrefs` declares a maximum size.
fn externref_table_has_maximum(path: &Path) -> Result<bool, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut r = Reader {
        bytes: &bytes,
        at: 8,
    };
    // Each table's "has a maximum", imported tables first, as the index space counts them.
    let mut tables = Vec::new();
    let mut exported = None;
    while r.at < bytes.len() {
        let id = r.byte()?;
        let size = r.leb()? as usize;
        let end = r.at + size;
        match id {
            2 => {
                for _ in 0..r.leb()? {
                    r.name()?;
                    r.name()?;
                    match r.byte()? {
                        0 => {
                            r.leb()?;
                        }
                        1 => tables.push(r.table_type()?),
                        2 => {
                            r.limits()?;
                        }
                        3 => {
                            r.byte()?;
                            r.byte()?;
                        }
                        4 => {
                            r.byte()?;
                            r.leb()?;
                        }
                        kind => return Err(format!("import of unknown kind {kind}")),
                    }
                }
            }
            4 => {
                for _ in 0..r.leb()? {
                    tables.push(r.table_type()?);
                }
            }
            7 => {
                for _ in 0..r.leb()? {
                    let name = r.name()?;
                    let kind = r.byte()?;
                    let index = r.leb()?;
                    if kind == 1 && name == b"__wbindgen_externrefs" {
                        exported = Some(index as usize);
                    }
                }
            }
            _ => {}
        }
        r.at = end;
    }
    let index = exported.ok_or("no table is exported as `__wbindgen_externrefs`")?;
    tables
        .get(index)
        .copied()
        .ok_or_else(|| format!("the exported table {index} is not declared"))
}

/// A cursor over a module's bytes, reading what `externref_table_has_maximum` needs.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Result<u8, String> {
        let b = *self.bytes.get(self.at).ok_or("the module ends early")?;
        self.at += 1;
        Ok(b)
    }

    fn leb(&mut self) -> Result<u64, String> {
        let (mut value, mut shift) = (0u64, 0);
        loop {
            let b = self.byte()?;
            value |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
            if shift > 63 {
                return Err("an integer is too long".into());
            }
        }
    }

    fn name(&mut self) -> Result<&'a [u8], String> {
        let len = self.leb()? as usize;
        let name = self
            .bytes
            .get(self.at..self.at + len)
            .ok_or("the module ends early")?;
        self.at += len;
        Ok(name)
    }

    /// Whether the limits declare a maximum.
    fn limits(&mut self) -> Result<bool, String> {
        let flags = self.byte()?;
        self.leb()?;
        if flags & 1 != 0 {
            self.leb()?;
        }
        Ok(flags & 1 != 0)
    }

    fn table_type(&mut self) -> Result<bool, String> {
        match self.byte()? {
            0x70 | 0x6f => self.limits(),
            t => Err(format!("a table of reference type {t:#x}")),
        }
    }
}

/// Writes the workloads' files into `page` (and `opt` when given) and `expected.json`; false when
/// something the page needs could not be made (the problem is in the step).
pub fn workload_files(
    env: &Env,
    step: &mut StepResult,
    amoris: &Path,
    workloads: &[Workload],
    page: &Path,
    log: &Path,
) -> bool {
    let mut projects = Vec::new();
    for w in workloads {
        let rel = format!("projects/{}", w.slug);
        let dir = page.join(&rel);
        let _ = std::fs::create_dir_all(&dir);
        let pack = env.cargo([
            "run",
            "-p",
            "amoris-web",
            "--example",
            "pack",
            "--release",
            "--locked",
            "--message-format=json",
            "--",
        ]);
        let mut pack = pack;
        pack.arg(env.root.join(&w.dir))
            .arg(dir.join("package.json"));
        if !command(step, pack, log, &format!("packing {} for the web", w.dir)) {
            return false;
        }
        let inputs = match &w.inputs {
            Some(i) => {
                std::fs::copy(env.root.join(&w.dir).join(i), dir.join("inputs.jsonl")).is_ok()
            }
            None => std::fs::write(dir.join("inputs.jsonl"), "").is_ok(),
        };
        if !inputs {
            step.error(Problem::new(
                "check.command_failed",
                format!("cannot copy the inputs of {}", w.dir),
                json!({"project": w.dir}),
            ));
            return false;
        }
        let mut runs = Vec::new();
        for seed in &w.seeds {
            let mut cmd = Command::new(amoris);
            cmd.current_dir(&env.root).args([
                "hashes",
                &w.dir,
                "--seed",
                &seed.to_string(),
                "--ticks",
                &w.ticks.to_string(),
            ]);
            if let Some(i) = &w.inputs {
                cmd.args(["--inputs", i]);
            }
            let line = run::describe(&cmd);
            let out = match run::run_stdout(cmd, log) {
                Ok(o) => o,
                Err(e) => {
                    step.problem(run::start_failed(&line, &e));
                    return false;
                }
            };
            let chain: Option<Value> = serde_json::from_str(&out.text).ok();
            match chain.filter(|c| out.ok() && c["hashes"].is_array()) {
                Some(c) => runs.push(json!({"seed": seed, "ticks": w.ticks,
                    "bundle": c["bundle"], "hashes": c["hashes"]})),
                None => {
                    step.error(Problem::new(
                        "check.command_failed",
                        format!("{line} gave no hash chain (exit {:?})", out.code),
                        json!({"command": line, "status": out.code, "tail": out.tail(20)}),
                    ));
                    return false;
                }
            }
        }
        let mut replays = Vec::new();
        // The `replay` step does not run the web page's controls, so they have no replays.
        for seed in w.seeds.iter().filter(|_| w.expect.is_none()) {
            let name = format!("{}-{seed}.p3dreplay", w.slug);
            let from = env.out.join("replays").join(&name);
            if from.is_file() && std::fs::copy(&from, dir.join(&name)).is_ok() {
                replays.push(json!(format!("{rel}/{name}")));
            } else {
                step.warn(Problem::new(
                    "web.replay_missing",
                    format!("{} has no replay of seed {seed} to verify in the worker (the replay step writes it)", w.dir),
                    json!({"project": w.dir, "seed": seed, "path": env.rel(&from)}),
                ));
            }
        }
        projects.push(json!({
            "name": w.dir,
            "package": format!("{rel}/package.json"),
            "inputs": format!("{rel}/inputs.jsonl"),
            "runs": runs,
            "replays": replays,
            "expect": w.expect,
        }));
    }
    let expected = json!({"format": "amoris-web-expected", "projects": projects});
    if let Err(e) = std::fs::write(page.join("expected.json"), expected.to_string()) {
        step.error(Problem::new(
            "check.command_failed",
            format!("cannot write expected.json: {e}"),
            json!({}),
        ));
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_name_replays_as_amoris_check_does() {
        assert_eq!(slug("samples/sailing"), "samples-sailing");
        assert_eq!(
            slug("tests/fixtures/controls/x_y"),
            "tests-fixtures-controls-x-y"
        );
    }
}
