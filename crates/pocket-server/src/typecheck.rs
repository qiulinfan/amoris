//! The type check stage of `scripts.apply` and `scripts.check` (docs/spec/server.md): TypeScript's
//! own checker, `tsc --noEmit`, when it is installed (`POCKET_TSC` or `tsc` on `PATH`), run off the
//! game thread with a time limit. Its findings are diagnostics with TypeScript locations; they are
//! reported beside the compile's and never stop a swap, since the in-process compiler (oxc) and the
//! lint are what decide whether scripts load.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

/// How long a type check may take.
const LIMIT: Duration = Duration::from_secs(60);

/// The `tsc` to run, if any.
pub fn find_tsc() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("POCKET_TSC") {
        return Some(PathBuf::from(p));
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join("tsc"))
        .find(|p| p.is_file())
}

/// One line of `tsc --pretty false`: `file(line,col): error TS2322: message`.
fn parse_line(root: &Path, line: &str) -> Option<Value> {
    let (loc, rest) = line.split_once("): ")?;
    let (file, pos) = loc.rsplit_once('(')?;
    let (l, c) = pos.split_once(',')?;
    let (severity, rest) = rest.split_once(' ')?;
    let (code, message) = rest.split_once(": ")?;
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

/// Runs the type check over the project: `{typecheck: "ok"|"failed"|"unavailable"|"timeout",
/// diagnostics}`.
pub async fn run(project: &Path) -> Value {
    let Some(tsc) = find_tsc() else {
        return json!({"typecheck": "unavailable", "diagnostics": []});
    };
    let mut cmd = tokio::process::Command::new(&tsc);
    cmd.current_dir(project)
        .arg("--noEmit")
        .arg("--pretty")
        .arg("false");
    if project.join("tsconfig.json").is_file() {
        cmd.arg("-p").arg(project);
    } else {
        cmd.args(["--strict", "--target", "es2022", "--module", "es2022"]);
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
    cmd.kill_on_drop(true);
    let out = match tokio::time::timeout(LIMIT, cmd.output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            return json!({"typecheck": "unavailable", "diagnostics": [],
                          "reason": e.to_string()});
        }
        Err(_) => return json!({"typecheck": "timeout", "diagnostics": []}),
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let diagnostics: Vec<Value> = text
        .lines()
        .filter_map(|l| parse_line(project, l))
        .collect();
    let state = if out.status.success() { "ok" } else { "failed" };
    json!({"typecheck": state, "diagnostics": diagnostics})
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
    }
}
