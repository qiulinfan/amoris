//! `editor`: the web editor's own checks (docs/spec/checks.md 4; editor.md 10). Its bun unit
//! tests (`bun test`) and type check (`bun run typecheck`), and, outside `--quick`, its browser
//! harnesses in headless Chrome over CDP against `amoris serve` (the release binary `build` made)
//! serving the editor built with `VITE_AMORIS_EXPOSE=1`: each harness gets a fresh copy of its
//! sample under `out/check/editor/` and a host of its own on a port the system picks, and passes
//! when it exits 0. With no bun or no `editor/node_modules` the step is skipped and says why; with
//! no Chrome the harnesses are inconclusive (`check.tool_missing`).

use crate::report::{Problem, StepResult};
use crate::run::{self, Env};
use serde_json::json;
use std::fs::{self, OpenOptions};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A browser harness: its file under `editor/tools/`, the sample it edits a copy of, and its own
/// arguments besides `--url` and `--out`.
pub struct Harness {
    pub name: &'static str,
    pub sample: &'static str,
    pub args: &'static [&'static str],
}

/// The harnesses that drive the built editor against a real host and judge by their exit code.
/// `edit-host` draws with the fallback view, so the step needs no WebGPU (the `gpus` and `web`
/// steps own the GPU); the others need none. The ones left out need the Vite dev server
/// (`sdk_check`, `viewport-loads`), WebGPU and the water (`water-authoring`), or measure
/// (`gizmo-drag`, `profiler-capture`, `serve-clients`).
pub const HARNESSES: &[Harness] = &[
    Harness {
        name: "edit-host",
        sample: "samples/anim",
        args: &["--viewport", "fallback"],
    },
    Harness {
        name: "rename-host",
        sample: "samples/sailing",
        args: &[],
    },
    Harness {
        name: "debug-host",
        sample: "samples/sailing",
        args: &[],
    },
];

/// How long a harness may take before it is stopped and fails.
const HARNESS_TIMEOUT: Duration = Duration::from_secs(300);
/// How long a host may take to answer on its port.
const HOST_TIMEOUT: Duration = Duration::from_secs(60);

/// bun: `AMORIS_BUN`, else `bun` on PATH, else bun's own install (`~/.bun/bin`).
pub fn bun() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = std::env::var_os("AMORIS_BUN")
        .map(PathBuf::from)
        .into_iter()
        .collect();
    candidates.push(PathBuf::from("bun"));
    candidates.push(
        run::home()
            .join(".bun")
            .join("bin")
            .join(format!("bun{}", std::env::consts::EXE_SUFFIX)),
    );
    candidates.into_iter().find(|b| {
        Command::new(b)
            .arg("--version")
            .stdin(Stdio::null())
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

pub fn run(env: &Env, amoris: &Result<PathBuf, Problem>, quick: bool) -> StepResult {
    let Some(bun) = bun() else {
        return StepResult::skipped(
            "editor",
            "no bun (AMORIS_BUN, PATH or ~/.bun/bin): the editor's tests and harnesses run on it",
        );
    };
    let editor = env.root.join("editor");
    if !editor.join("node_modules").is_dir() {
        return StepResult::skipped(
            "editor",
            "editor/node_modules is missing: run `cd editor && bun install`",
        );
    }
    let mut step = StepResult::new("editor");
    let log = env.new_log("editor");
    step.log = Some(env.rel(&log));
    let mut done = Vec::new();
    for (what, args, code) in [
        ("unit tests", &["test"][..], "editor.test_failed"),
        (
            "type check",
            &["run", "typecheck"][..],
            "editor.typecheck_failed",
        ),
    ] {
        let mut cmd = Command::new(&bun);
        cmd.args(args).current_dir(&editor);
        let line = run::describe(&cmd);
        match run::run_merged(cmd, &log, &mut |_| {}) {
            Ok(out) if out.ok() => done.push(what.to_string()),
            Ok(out) => step.error(Problem::new(
                code,
                format!("{line} failed (exit {:?})", out.code),
                json!({"command": line, "status": out.code, "tail": out.tail(30)}),
            )),
            Err(e) => step.problem(run::start_failed(&line, &e)),
        }
    }
    if quick {
        step.summary = format!(
            "{}; --quick leaves out the browser harnesses",
            done.join(", ")
        );
        return step;
    }
    let amoris = match amoris {
        Ok(p) => p.clone(),
        Err(p) => {
            step.error(p.clone());
            step.summary = format!("{}; no amoris binary for the harnesses", done.join(", "));
            return step;
        }
    };
    let out = env.out.join("editor");
    let dist = out.join("dist");
    let mut build = Command::new(&bun);
    build
        .args(["x", "vite", "build", "--outDir"])
        .arg(&dist)
        .arg("--emptyOutDir")
        .env("VITE_AMORIS_EXPOSE", "1")
        .current_dir(&editor);
    let line = run::describe(&build);
    match run::run_merged(build, &log, &mut |_| {}) {
        Ok(o) if o.ok() => {}
        Ok(o) => {
            step.error(Problem::new(
                "editor.build_failed",
                format!("{line} failed (exit {:?})", o.code),
                json!({"command": line, "status": o.code, "tail": o.tail(30)}),
            ));
            return step;
        }
        Err(e) => {
            step.problem(run::start_failed(&line, &e));
            return step;
        }
    }
    let mut passed = 0;
    for h in HARNESSES {
        match harness(env, &bun, &amoris, &dist, &out, h, &log) {
            Ok(()) => passed += 1,
            Err(p) => {
                let missing = p.code == "check.tool_missing";
                step.problem(p);
                if missing {
                    break;
                }
            }
        }
    }
    done.push(format!(
        "{passed} of {} browser harnesses against amoris serve",
        HARNESSES.len()
    ));
    step.summary = done.join(", ");
    step
}

/// One harness on a fresh copy of its sample, against a host of its own.
fn harness(
    env: &Env,
    bun: &Path,
    amoris: &Path,
    dist: &Path,
    out: &Path,
    h: &Harness,
    log: &Path,
) -> Result<(), Problem> {
    let dir = out.join(h.name);
    let project = dir.join("project");
    let _ = fs::remove_dir_all(&project);
    copy(&env.root.join(h.sample), &project).map_err(|e| {
        Problem::new(
            "check.command_failed",
            format!("cannot copy {} for {}: {e}", h.sample, h.name),
            json!({}),
        )
    })?;
    let port = free_port().map_err(|e| {
        Problem::new(
            "check.command_failed",
            format!("no free port for {}'s host: {e}", h.name),
            json!({}),
        )
    })?;
    let mut serve = Command::new(amoris);
    serve
        .arg("serve")
        .arg(&project)
        .args(["--port", &port.to_string(), "--editor"])
        .arg(dist)
        .current_dir(&env.root);
    let host = Guard(spawn_logged(serve, log)?);
    wait_for_port(port, HOST_TIMEOUT).map_err(|why| {
        Problem::new(
            "check.command_failed",
            format!(
                "{}'s host did not answer on 127.0.0.1:{port}: {why}",
                h.name
            ),
            json!({"harness": h.name}),
        )
    })?;
    let mut cmd = Command::new(bun);
    cmd.arg(format!("tools/{}.ts", h.name))
        .args(["--url", &format!("http://127.0.0.1:{port}/"), "--out"])
        .arg(&dir)
        .args(h.args)
        .current_dir(env.root.join("editor"));
    let line = run::describe(&cmd);
    let from = fs::metadata(log).map_or(0, |m| m.len());
    let mut child = spawn_logged(cmd, log)?;
    let deadline = Instant::now() + HARNESS_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(200)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    drop(host);
    let text = fs::read(log)
        .map(|b| String::from_utf8_lossy(&b[(from as usize).min(b.len())..]).into_owned())
        .unwrap_or_default();
    let failed: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("FAIL"))
        .take(10)
        .collect();
    let tail = {
        let lines: Vec<&str> = text.lines().collect();
        lines[lines.len().saturating_sub(20)..].join("\n")
    };
    if text.contains("no Chrome found") {
        return Err(Problem::new(
            "check.tool_missing",
            "no Chrome for the editor's browser harnesses (CHROME=<path> names one)",
            json!({"tool": "chrome"}),
        ));
    }
    match status.and_then(|s| s.code()) {
        Some(0) => Ok(()),
        None => Err(Problem::new(
            "editor.harness_failed",
            format!(
                "{} took over {} s and was stopped",
                h.name,
                HARNESS_TIMEOUT.as_secs()
            ),
            json!({"harness": h.name, "command": line, "tail": tail}),
        )),
        Some(code) => Err(Problem::new(
            "editor.harness_failed",
            format!(
                "{} exited {code}{}",
                h.name,
                failed
                    .first()
                    .map_or(String::new(), |f| format!(": {}", f.trim()))
            ),
            json!({"harness": h.name, "command": line, "status": code, "failed": failed,
                   "tail": tail, "out": dir.to_string_lossy()}),
        )),
    }
}

/// A host process, stopped when dropped.
struct Guard(Child);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Starts a command with its output appended to the log, after the command line.
fn spawn_logged(mut cmd: Command, log: &Path) -> Result<Child, Problem> {
    let line = run::describe(&cmd);
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .and_then(|mut f| {
            use std::io::Write as _;
            writeln!(f, "$ {line}")?;
            Ok(f)
        })
        .map_err(|e| run::start_failed(&line, &e))?;
    let err = file.try_clone().map_err(|e| run::start_failed(&line, &e))?;
    cmd.stdout(file)
        .stderr(err)
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| run::start_failed(&line, &e))
}

fn free_port() -> std::io::Result<u16> {
    Ok(TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port())
}

fn wait_for_port(port: u16, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(format!("nothing after {} s", timeout.as_secs()))
}

fn copy(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for e in fs::read_dir(src)? {
        let e = e?;
        let to = dst.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy(&e.path(), &to)?;
        } else {
            fs::copy(e.path(), to)?;
        }
    }
    Ok(())
}
