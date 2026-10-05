//! The type check stage of `scripts.apply` and `scripts.check` (docs/spec/server.md; docs/sdk.md):
//! TypeScript's own checker, `tsc --noEmit` (TypeScript 7, the native compiler), over the project's
//! `tsconfig.json`, else the one `scripts.types` writes beside the declarations
//! (`.pocket/types/tsconfig.json`), both mapping `pocket` to them. It runs as a child process off
//! the game thread with a time limit; its findings are diagnostics with TypeScript locations,
//! reported beside the compile's, and never stop or delay a swap, since the in-process compiler
//! (oxc) and the lint decide whether scripts load.
//!
//! The compiler is `POCKET_TSC` as given, else the first TypeScript 7 (`tsc --version`) in a
//! `node_modules` (or `sdk/node_modules`, the repository's toolchain: `cd sdk && bun install`,
//! pinned by `sdk/bun.lock`) of the project or a directory above it, else in the repository this
//! binary was built from, else as `tsc` on `PATH`; an older `tsc` is passed over and named in the
//! reason. The native binary of the platform package is preferred over the `node` launcher in
//! `.bin`.
//!
//! [`run`] is the server's (tokio, after a swap); [`run_blocking`] is `pocket check`'s `types` step
//! (checks.md 6.4), which `pocket-app` hands to `pocket-check`.

use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// How long a type check may take.
const LIMIT: Duration = Duration::from_secs(60);

/// Where `scripts.types` writes the declarations and their `tsconfig.json`
/// (`pocket_script::types::TYPES_DIR`; this crate does not link the game crates).
const TYPES_DIR: &str = ".pocket/types";

/// The npm platform package's suffix (`typescript-darwin-arm64`), as Node names the platform.
fn platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    format!("{os}-{arch}")
}

/// The `tsc` inside a `node_modules` directory: the platform package's native binary, else the
/// launcher.
fn in_node_modules(nm: &Path) -> Option<PathBuf> {
    let exe = if cfg!(windows) { "tsc.exe" } else { "tsc" };
    let native = nm
        .join("@typescript")
        .join(format!("typescript-{}", platform()))
        .join("lib")
        .join(exe);
    if native.is_file() {
        return Some(native);
    }
    let launcher = nm
        .join(".bin")
        .join(if cfg!(windows) { "tsc.cmd" } else { "tsc" });
    launcher.is_file().then_some(launcher)
}

/// A `tsc` to run, with its version (`7.0.2`).
#[derive(Clone, Debug)]
pub struct Tsc {
    pub path: PathBuf,
    pub version: String,
}

/// `tsc --version`'s number, asked once per path for the life of the process.
fn version(tsc: &Path) -> Option<String> {
    static SEEN: OnceLock<Mutex<HashMap<PathBuf, Option<String>>>> = OnceLock::new();
    let seen = SEEN.get_or_init(Mutex::default);
    if let Some(v) = seen.lock().ok().and_then(|m| m.get(tsc).cloned()) {
        return v;
    }
    let v = Command::new(tsc)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|t| t.trim().strip_prefix("Version ").map(str::to_owned));
    if let Ok(mut m) = seen.lock() {
        m.insert(tsc.to_path_buf(), v.clone());
    }
    v
}

/// Whether a version is TypeScript 7 or later, which the SDK's declarations and docs assume.
fn is_supported(version: &str) -> bool {
    version
        .split('.')
        .next()
        .and_then(|m| m.parse::<u32>().ok())
        .is_some_and(|m| m >= 7)
}

/// The `tsc` to run for `project`, or why there is none (no `tsc`, or only older ones, named).
pub fn find_tsc(project: &Path) -> Result<Tsc, String> {
    if let Some(p) = std::env::var_os("POCKET_TSC") {
        let path = PathBuf::from(p);
        let version = version(&path)
            .ok_or_else(|| format!("POCKET_TSC={} does not answer `--version`", path.display()))?;
        return Ok(Tsc { path, version });
    }
    let start = std::fs::canonicalize(project).unwrap_or_else(|_| project.to_path_buf());
    let built_from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let in_dirs = start
        .ancestors()
        .chain(std::iter::once(built_from.as_path()))
        .flat_map(|d| [d.join("node_modules"), d.join("sdk").join("node_modules")])
        .filter_map(|nm| in_node_modules(&nm));
    let on_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .map(|d| d.join("tsc"))
        .filter(|p| p.is_file());
    let mut older = Vec::new();
    for path in in_dirs.chain(on_path) {
        match version(&path) {
            Some(version) if is_supported(&version) => return Ok(Tsc { path, version }),
            Some(v) => older.push(format!("{} is TypeScript {v}", path.display())),
            None => older.push(format!("{} does not answer `--version`", path.display())),
        }
    }
    let mut reason = "no TypeScript 7 tsc: run `bun install` in the repository's sdk/, or set \
                      POCKET_TSC"
        .to_owned();
    if !older.is_empty() {
        reason.push_str(&format!(" ({})", older.join("; ")));
    }
    Err(reason)
}

/// One diagnostic of `tsc --pretty false`: `file(line,col): error TS2322: message`, followed by
/// indented lines that elaborate it (`Did you mean ...?` is often the last), or one with no place
/// (`error TS5058: The specified path does not exist: ...`, a broken `tsconfig.json`).
fn parse_line(root: &Path, line: &str) -> Option<Value> {
    if let Some(rest) = line.strip_prefix("error ") {
        let (code, message) = rest.split_once(": ")?;
        return code.starts_with("TS").then(|| {
            json!({"file": null, "line": null, "column": null, "code": code, "message": message,
                   "severity": "error", "source": "tsc"})
        });
    }
    let (loc, rest) = line.split_once("): ")?;
    let (file, pos) = loc.rsplit_once('(')?;
    let (l, c) = pos.split_once(',')?;
    let (severity, rest) = rest.split_once(' ')?;
    let (code, message) = rest.split_once(": ")?;
    if !code.starts_with("TS") {
        return None;
    }
    let file = Path::new(file);
    let file = file
        .strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/");
    Some(json!({
        "file": file,
        "line": l.trim().parse::<u64>().ok(),
        "column": c.trim().parse::<u64>().ok(),
        "code": code,
        "message": message,
        "severity": severity,
        "source": "tsc",
    }))
}

/// Every diagnostic of `tsc`'s output, elaborations appended to their message.
fn parse(root: &Path, text: &str) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for line in text.lines() {
        if line.starts_with(' ') {
            if let Some(last) = out.last_mut()
                && let Some(m) = last["message"].as_str()
            {
                last["message"] = json!(format!("{m}\n{}", line.trim_start()));
            }
            continue;
        }
        if let Some(d) = parse_line(root, line) {
            out.push(d);
        }
    }
    out
}

fn unavailable(reason: &str) -> Value {
    json!({"typecheck": "unavailable", "diagnostics": [], "reason": reason})
}

/// The `tsc` command over the project: its `tsconfig.json`, else the declarations' own, or why
/// there is neither (`scripts.types` has not written them).
fn command(tsc: &Path, project: &Path) -> Result<Command, String> {
    // The project is the working directory: a relative project path would resolve twice.
    let config = if project.join("tsconfig.json").is_file() {
        ".".to_owned()
    } else {
        let generated = format!("{TYPES_DIR}/tsconfig.json");
        if !project.join(&generated).is_file() {
            return Err(format!(
                "the project has no tsconfig.json and no {generated}: run `pocket scripts types`"
            ));
        }
        generated
    };
    let mut cmd = Command::new(tsc);
    cmd.current_dir(project)
        .args(["--noEmit", "--pretty", "false", "-p"])
        .arg(config);
    Ok(cmd)
}

/// The outcome of a finished `tsc`.
fn outcome(project: &Path, tsc: &Tsc, success: bool, stdout: &[u8], started: Instant) -> Value {
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    let diagnostics = parse(project, &String::from_utf8_lossy(stdout));
    let state = if success { "ok" } else { "failed" };
    json!({"typecheck": state, "diagnostics": diagnostics, "tsc_ms": (ms * 10.0).round() / 10.0,
           "tsc_version": tsc.version})
}

fn timeout(tsc: &Tsc) -> Value {
    json!({"typecheck": "timeout", "diagnostics": [], "tsc_version": tsc.version,
           "reason": format!("tsc did not finish in {} s", LIMIT.as_secs())})
}

/// The `tsc` and its command for `project`, or the answer that says why there is none.
fn prepare(project: &Path) -> Result<(Tsc, Command), Value> {
    let tsc = find_tsc(project).map_err(|r| unavailable(&r))?;
    let cmd = command(&tsc.path, project).map_err(|r| unavailable(&r))?;
    Ok((tsc, cmd))
}

/// Runs the type check over the project: `{typecheck: "ok"|"failed"|"unavailable"|"timeout",
/// diagnostics, tsc_ms, tsc_version, reason}`.
pub async fn run(project: &Path) -> Value {
    // Finding tsc may ask a candidate its version (a process, once per path): off the runtime.
    let dir = project.to_path_buf();
    let prepared = match tokio::task::spawn_blocking(move || prepare(&dir)).await {
        Ok(p) => p,
        Err(e) => return unavailable(&format!("finding tsc failed: {e}")),
    };
    let (tsc, cmd) = match prepared {
        Ok(p) => p,
        Err(answer) => return answer,
    };
    let mut cmd = tokio::process::Command::from(cmd);
    cmd.kill_on_drop(true);
    let started = Instant::now();
    match tokio::time::timeout(LIMIT, cmd.output()).await {
        Ok(Ok(o)) => outcome(project, &tsc, o.status.success(), &o.stdout, started),
        Ok(Err(e)) => unavailable(&format!("{}: {e}", tsc.path.display())),
        Err(_) => timeout(&tsc),
    }
}

/// [`run`] without an async runtime, for `pocket check`: the same answer, the child killed at the
/// time limit.
pub fn run_blocking(project: &Path) -> Value {
    let (tsc, mut cmd) = match prepare(project) {
        Ok(p) => p,
        Err(answer) => return answer,
    };
    let started = Instant::now();
    let spawned = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => return unavailable(&format!("{}: {e}", tsc.path.display())),
    };
    // Read on a thread so that a long report cannot fill the pipe while this one waits.
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(out) = stdout.as_mut() {
            let _ = out.read_to_end(&mut buf);
        }
        buf
    });
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = reader.join().unwrap_or_default();
                return outcome(project, &tsc, status.success(), &out, started);
            }
            Ok(None) if started.elapsed() < LIMIT => {
                std::thread::sleep(Duration::from_millis(2));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return timeout(&tsc);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tsc_lines_become_diagnostics() {
        let d = parse_line(
            Path::new("/p"),
            "scripts/rules.ts(12,5): error TS2322: Type 'string' is not assignable to type 'number'.",
        )
        .unwrap();
        assert_eq!(d["file"], json!("scripts/rules.ts"));
        assert_eq!(d["line"], json!(12));
        assert_eq!(d["column"], json!(5));
        assert_eq!(d["code"], json!("TS2322"));
        assert!(parse_line(Path::new("/p"), "Found 2 errors.").is_none());
        let d = parse_line(
            Path::new("/p"),
            "error TS5058: The specified path does not exist: 'x'.",
        )
        .unwrap();
        assert_eq!(d["code"], json!("TS5058"));
        assert_eq!(d["file"], Value::Null);
    }

    #[test]
    fn only_typescript_7_is_trusted() {
        assert!(is_supported("7.0.2"));
        assert!(is_supported("7.1.0-dev.20261001"));
        assert!(!is_supported("5.9.3"));
        assert!(!is_supported("garbage"));
    }

    #[test]
    fn tsc_runs_with_the_projects_tsconfig_else_the_generated_one() {
        let dir = std::env::temp_dir().join(format!("pocket-tsc-cmd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(TYPES_DIR)).unwrap();
        let args = |c: &Command| -> Vec<String> {
            c.get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect()
        };
        let tsc = Path::new("tsc");
        assert!(command(tsc, &dir).is_err(), "no tsconfig.json anywhere");
        std::fs::write(dir.join(TYPES_DIR).join("tsconfig.json"), "{}").unwrap();
        let c = command(tsc, &dir).unwrap();
        assert_eq!(args(&c).last().unwrap(), ".pocket/types/tsconfig.json");
        std::fs::write(dir.join("tsconfig.json"), "{}").unwrap();
        let c = command(tsc, &dir).unwrap();
        assert_eq!(args(&c).last().unwrap(), ".");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn elaborations_join_their_diagnostic() {
        let out = "scripts/a.ts(5,48): error TS2322: Type '[\"Log.distnce\"]' is not assignable.\n  \
                   Type '\"Log.distnce\"' is not assignable to type '\"Log.distance\"'. Did you mean '\"Log.distance\"'?\n\
                   scripts/a.ts(9,1): error TS2304: Cannot find name 'x'.\n";
        let d = parse(Path::new("/p"), out);
        assert_eq!(d.len(), 2);
        assert!(
            d[0]["message"]
                .as_str()
                .unwrap()
                .ends_with("Did you mean '\"Log.distance\"'?")
        );
        assert_eq!(d[1]["code"], json!("TS2304"));
    }
}
