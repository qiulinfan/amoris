//! The type check stage of `scripts.apply` and `scripts.check` (docs/spec/server.md; docs/sdk.md):
//! TypeScript's own checker, `tsc --noEmit` (TypeScript 7, the native compiler), over the project's
//! `tsconfig.json`, which maps `pocket` to the declarations `scripts.types` writes. It runs as a
//! child process off the game thread with a time limit; its findings are diagnostics with
//! TypeScript locations, reported beside the compile's, and never stop or delay a swap, since the
//! in-process compiler (oxc) and the lint decide whether scripts load.
//!
//! The compiler is found by `POCKET_TSC`, else in a `node_modules` (or `sdk/node_modules`, the
//! repository's toolchain: `cd sdk && bun install`) of the project or a directory above it, else in
//! the repository this binary was built from, else as `tsc` on `PATH`. The native binary of the
//! platform package is preferred over the `node` launcher in `.bin`.
//!
//! [`run`] is the server's (tokio, beside a swap); [`run_blocking`] is `pocket check`'s `types` step
//! (checks.md 6.4), which `pocket-app` hands to `pocket-check`.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// How long a type check may take.
const LIMIT: Duration = Duration::from_secs(60);

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

/// The `tsc` to run for `project`, if any.
pub fn find_tsc(project: &Path) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("POCKET_TSC") {
        return Some(PathBuf::from(p));
    }
    let start = std::fs::canonicalize(project).unwrap_or_else(|_| project.to_path_buf());
    let built_from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let found = start
        .ancestors()
        .chain(std::iter::once(built_from.as_path()))
        .flat_map(|d| [d.join("node_modules"), d.join("sdk").join("node_modules")])
        .find_map(|nm| in_node_modules(&nm));
    if found.is_some() {
        return found;
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join("tsc"))
        .find(|p| p.is_file())
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

fn unavailable() -> Value {
    json!({"typecheck": "unavailable", "diagnostics": [],
           "reason": "no tsc: set POCKET_TSC, or run `bun install` in the repository's sdk/"})
}

/// The `tsc` command over the project: its `tsconfig.json`, else the options it would hold over
/// the scripts.
fn command(tsc: &Path, project: &Path) -> Command {
    let mut cmd = Command::new(tsc);
    cmd.current_dir(project)
        .arg("--noEmit")
        .arg("--pretty")
        .arg("false");
    if project.join("tsconfig.json").is_file() {
        // The project is the working directory: a relative project path would resolve twice.
        cmd.arg("-p").arg(".");
    } else {
        cmd.args(["--strict", "--target", "es2023", "--module", "esnext"]);
        cmd.args(["--moduleResolution", "bundler", "--skipLibCheck"]);
        let mut files = Vec::new();
        crate::local::walk(project, &project.join("scripts"), &mut files);
        cmd.args(
            files
                .iter()
                .filter(|(f, _)| f.ends_with(".ts"))
                .map(|(f, _)| f.clone()),
        );
    }
    cmd
}

/// The outcome of a finished `tsc`.
fn outcome(project: &Path, success: bool, stdout: &[u8], started: Instant) -> Value {
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    let diagnostics = parse(project, &String::from_utf8_lossy(stdout));
    let state = if success { "ok" } else { "failed" };
    json!({"typecheck": state, "diagnostics": diagnostics, "tsc_ms": (ms * 10.0).round() / 10.0})
}

/// Runs the type check over the project: `{typecheck: "ok"|"failed"|"unavailable"|"timeout",
/// diagnostics, tsc_ms}`.
pub async fn run(project: &Path) -> Value {
    let Some(tsc) = find_tsc(project) else {
        return unavailable();
    };
    let mut cmd = tokio::process::Command::from(command(&tsc, project));
    cmd.kill_on_drop(true);
    let started = Instant::now();
    match tokio::time::timeout(LIMIT, cmd.output()).await {
        Ok(Ok(o)) => outcome(project, o.status.success(), &o.stdout, started),
        Ok(Err(e)) => json!({"typecheck": "unavailable", "diagnostics": [],
                             "reason": format!("{}: {e}", tsc.display())}),
        Err(_) => json!({"typecheck": "timeout", "diagnostics": []}),
    }
}

/// [`run`] without an async runtime, for `pocket check`: the same answer, the child killed at the
/// time limit.
pub fn run_blocking(project: &Path) -> Value {
    let Some(tsc) = find_tsc(project) else {
        return unavailable();
    };
    let started = Instant::now();
    let spawned = command(&tsc, project)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            return json!({"typecheck": "unavailable", "diagnostics": [],
                          "reason": format!("{}: {e}", tsc.display())});
        }
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
                return outcome(project, status.success(), &out, started);
            }
            Ok(None) if started.elapsed() < LIMIT => {
                std::thread::sleep(Duration::from_millis(2));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return json!({"typecheck": "timeout", "diagnostics": []});
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
