//! Type checking: TypeScript projects read against the SDK's types by the TypeScript compiler
//! (TypeScript 7, the native one, installed as the `typescript` dependency). The bundler strips
//! types without reading them; this is where they are read, so an agent or a person hears about a
//! misspelt component field or a wrong callback before the game runs.

use crate::deps;
use crate::manifest::Workspace;
use crate::report::{Diagnostic, Report};
use crate::toolchain;
use anyhow::{bail, Context, Result};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The compiler, set up on first use (a download of about 9 MB).
fn tsc(ws: &Workspace) -> Result<PathBuf> {
    let dep = ws.file.dependencies.iter().find(|d| d.name == "typescript").context("pocket.toml has no typescript dependency")?;
    if !deps::is_ready(ws, dep) {
        let mut log = vec![];
        deps::setup_one(ws, dep, false, &mut log, "native")?;
        for line in log {
            eprintln!("{line}");
        }
    }
    let exe = deps::prefix(ws, dep).join("lib").join(format!("tsc{}", std::env::consts::EXE_SUFFIX));
    if !exe.exists() {
        bail!("the typescript dependency has no compiler at {} (run `pocket setup --force`)", exe.display());
    }
    Ok(exe)
}

/// What one run reads: every TypeScript file of a project, or of the workspace (the SDK, the
/// editor, the TypeScript tests, the samples and the integrations).
fn includes(ws: &Workspace, project: Option<&Path>) -> Result<(String, Vec<PathBuf>)> {
    if let Some(p) = project {
        let dir = if p.is_file() { p.parent().unwrap_or(p).to_path_buf() } else { p.to_path_buf() };
        let dir = std::fs::canonicalize(&dir).with_context(|| format!("no project at {}", p.display()))?;
        let name = dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "project".into());
        return Ok((name, vec![dir]));
    }
    let root = std::fs::canonicalize(&ws.root).unwrap_or(ws.root.clone());
    let mut dirs = vec![root.join("sdk").join("runtime"), root.join("editor"), root.join("tests").join("ts")];
    if let Ok(rd) = std::fs::read_dir(root.join("samples")) {
        let mut samples: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.join("project.toml").exists()).collect();
        samples.sort();
        dirs.extend(samples);
    }
    Ok(("workspace".into(), dirs.into_iter().filter(|d| d.is_dir()).collect()))
}

/// The compiler's options: the SDK as the `pocket` module, JSX through its runtime, the language
/// the script host runs (no DOM, no Node), and strict checks.
fn tsconfig(ws: &Workspace, dirs: &[PathBuf], extra: &[PathBuf]) -> serde_json::Value {
    let sdk = std::fs::canonicalize(ws.root.join("sdk").join("runtime")).unwrap_or(ws.root.join("sdk").join("runtime"));
    let mut include = vec![];
    let mut exclude = vec![];
    for f in extra {
        include.push(f.display().to_string());
    }
    for d in dirs {
        include.push(format!("{}/**/*.ts", d.display()));
        include.push(format!("{}/**/*.tsx", d.display()));
        for skip in ["build", ".pocket", "node_modules", "dist"] {
            exclude.push(format!("{}/{skip}", d.display()));
        }
    }
    json!({
        "compilerOptions": {
            "strict": true,
            "noEmit": true,
            "target": "ES2022",
            "module": "ESNext",
            "moduleResolution": "bundler",
            "jsx": "react-jsx",
            "jsxImportSource": "pocket",
            "lib": ["ES2023"],
            "types": [],
            "skipLibCheck": true,
            "paths": {
                "pocket": [sdk.join("pocket.ts")],
                "pocket/*": [format!("{}/*", sdk.display())],
            },
        },
        "include": include,
        "exclude": exclude,
    })
}

/// `file(line,col): error TS2304: message`, with indented lines continuing the message before;
/// a line without a place (`error TS5023: ...`) is about the configuration.
pub fn parse_tsc(text: &str, root: &Path) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> = vec![];
    for line in text.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(last) = out.last_mut() {
                last.message.push('\n');
                last.message.push_str(line.trim());
            }
            continue;
        }
        let (place, rest) = match line.find("): ") {
            Some(i) if line[..i].contains('(') => (Some(&line[..i + 1]), &line[i + 3..]),
            _ => (None, line),
        };
        let Some((severity, message)) = rest.split_once(' ').and_then(|(sev, m)| if sev == "error" || sev == "warning" { Some((sev, m)) } else { None }) else { continue };
        let mut d = Diagnostic { severity: severity.to_string(), message: message.trim().to_string(), file: None, line: None, column: None };
        if let Some(place) = place {
            let open = place.rfind('(').unwrap();
            let file = &place[..open];
            let mut nums = place[open + 1..place.len() - 1].split(',');
            d.line = nums.next().and_then(|n| n.trim().parse().ok());
            d.column = nums.next().and_then(|n| n.trim().parse().ok());
            // tsc names files from where it ran (the workspace root): kept as they are inside it,
            // made absolute outside it.
            let path = Path::new(file);
            d.file = Some(if path.is_relative() && !file.starts_with("..") {
                file.to_string()
            } else {
                let abs = if path.is_absolute() { path.to_path_buf() } else { root.join(path) };
                std::fs::canonicalize(&abs).unwrap_or(abs).to_string_lossy().into_owned()
            });
        }
        out.push(d);
    }
    out
}

pub fn check(ws: &Workspace, project: Option<&Path>) -> Result<Report> {
    let t0 = Instant::now();
    let exe = tsc(ws)?;
    let (name, dirs) = includes(ws, project)?;
    let dir = ws.root.join("build").join("check").join(&name);
    toolchain::ensure_dir(&dir)?;
    let config = dir.join("tsconfig.json");
    // A project's own components (components.toml) as types beside the SDK's.
    let mut extra = vec![];
    for d in &dirs {
        if let Some(comps) = crate::gen::project_components(ws, d)? {
            let stem = d.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "project".into());
            let file = dir.join(format!("{stem}.components.d.ts"));
            std::fs::write(&file, crate::gen::project_components_ts(&comps)?)?;
            extra.push(file);
        }
    }
    std::fs::write(&config, serde_json::to_string_pretty(&tsconfig(ws, &dirs, &extra))?)?;
    let out = toolchain::command(exe.to_str().unwrap()).arg("-p").arg(&config).arg("--pretty").arg("false").current_dir(&ws.root).output().context("running tsc")?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let diagnostics = parse_tsc(&text, &ws.root);
    let errors = diagnostics.iter().filter(|d| d.severity == "error").count();
    let files: std::collections::BTreeSet<&str> = diagnostics.iter().filter_map(|d| d.file.as_deref()).collect();
    let mut rep = if out.status.success() && errors == 0 {
        Report::success("check", format!("{name}: no type errors"))
    } else if errors == 0 {
        Report::failure("check", format!("{name}: tsc failed: {}", text.lines().next().unwrap_or("").trim()))
    } else {
        Report::failure("check", format!("{name}: {errors} type error{} in {} file{}", if errors == 1 { "" } else { "s" }, files.len(), if files.len() == 1 { "" } else { "s" }))
    };
    rep.data = json!({ "scope": name, "errors": errors, "files": files, "tsconfig": config });
    rep.diagnostics = diagnostics;
    rep.elapsed_ms = t0.elapsed().as_millis();
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_places_codes_and_continuations() {
        let root = std::env::temp_dir();
        let text = "samples/a/main.ts(3,7): error TS2304: Cannot find name 'Vec3'.\nb.tsx(10,2): error TS2322: Type 'x' is not assignable to type 'y'.\n  Property 'key' does not exist on type 'z'.\nerror TS5023: Unknown compiler option 'foo'.\nFound 3 errors.\n";
        let d = parse_tsc(text, &root);
        assert_eq!(d.len(), 3);
        assert_eq!(d[0].file.as_deref(), Some("samples/a/main.ts"));
        assert_eq!((d[0].line, d[0].column), (Some(3), Some(7)));
        assert_eq!(d[0].message, "TS2304: Cannot find name 'Vec3'.");
        assert!(d[1].message.ends_with("\nProperty 'key' does not exist on type 'z'."));
        assert_eq!(d[2].file, None);
        assert!(d[2].message.starts_with("TS5023"));
    }
}
