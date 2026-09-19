//! Command implementations.

use crate::deps;
use crate::graph::Graph;
use crate::manifest::Workspace;
use crate::ninja;
use crate::report::{parse_compiler_diagnostics, Report};
use crate::toolchain;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub fn setup(ws: &Workspace, force: bool) -> Result<Report> {
    let t0 = Instant::now();
    let mut log = vec![];
    let mut statuses = vec![];
    for d in &ws.file.dependencies {
        statuses.push(deps::setup_one(ws, d, force, &mut log)?);
    }
    let mut rep = Report::success("setup", format!("{} dependencies ready", statuses.len()));
    rep.data = json!({ "dependencies": statuses, "log": log });
    rep.elapsed_ms = t0.elapsed().as_millis();
    Ok(rep)
}

pub fn doctor(ws: &Workspace) -> Result<Report> {
    let tc = toolchain::detect()?;
    let statuses = deps::status(ws);
    let missing: Vec<&str> = statuses.iter().filter(|s| s.state == "missing").map(|s| s.name.as_str()).collect();
    let mut rep = if tc.ninja.is_none() {
        Report::failure("doctor", "ninja not found on PATH")
    } else if !missing.is_empty() {
        Report::failure("doctor", format!("missing dependencies: {} (run `pocket setup`)", missing.join(", ")))
    } else {
        Report::success("doctor", "toolchain and dependencies ready")
    };
    rep.data = json!({ "toolchain": tc, "dependencies": statuses, "modules": ws.modules.keys().collect::<Vec<_>>() });
    Ok(rep)
}

fn ensure_deps(ws: &Workspace) -> Result<()> {
    let missing: Vec<String> = ws.file.dependencies.iter().filter(|d| !deps::is_ready(ws, d)).map(|d| d.name.clone()).collect();
    if !missing.is_empty() {
        bail!("dependencies not set up: {} (run `pocket setup`)", missing.join(", "));
    }
    Ok(())
}

pub struct BuildOutcome {
    pub ok: bool,
    pub output: String,
    pub build_dir: PathBuf,
}

pub fn build_targets(ws: &Workspace, config: &str, targets: &[String], generate_only: bool) -> Result<BuildOutcome> {
    ensure_deps(ws)?;
    crate::gen::generate(ws, false)?;
    let tc = toolchain::detect()?;
    let graph = Graph::resolve(ws)?;
    for t in targets {
        if !graph.modules.contains_key(t) {
            bail!("unknown target '{}' (known: {})", t, graph.modules.keys().cloned().collect::<Vec<_>>().join(", "));
        }
    }
    let gen = ninja::generate(ws, &graph, &tc, config)?;
    let ninja_bin = tc.ninja.clone().context("ninja not found on PATH")?;
    // compile_commands.json for clangd and every IDE.
    let compdb = toolchain::command(&ninja_bin).arg("-C").arg(&gen.build_dir).args(["-t", "compdb", "cxx", "objcxx", "cc"]).output()?;
    std::fs::write(gen.build_dir.join("compile_commands.json"), &compdb.stdout)?;
    let _ = std::fs::copy(gen.build_dir.join("compile_commands.json"), ws.root.join("compile_commands.json"));
    if generate_only {
        return Ok(BuildOutcome { ok: true, output: String::new(), build_dir: gen.build_dir });
    }
    let mut cmd = toolchain::command(&ninja_bin);
    cmd.arg("-C").arg(&gen.build_dir);
    if targets.is_empty() {
        cmd.arg("all");
    } else {
        cmd.args(targets);
    }
    let out = cmd.output().context("running ninja")?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    Ok(BuildOutcome { ok: out.status.success(), output: text, build_dir: gen.build_dir })
}

pub fn build(ws: &Workspace, config: &str, targets: &[String], generate_only: bool) -> Result<Report> {
    let t0 = Instant::now();
    let outcome = build_targets(ws, config, targets, generate_only)?;
    let diags = parse_compiler_diagnostics(&outcome.output);
    let mut rep = if outcome.ok {
        Report::success("build", if generate_only { "generated build.ninja and compile_commands.json".to_string() } else { format!("built {} ({})", if targets.is_empty() { "all".to_string() } else { targets.join(", ") }, config) })
    } else {
        Report::failure("build", "build failed")
    };
    rep.diagnostics = diags;
    if !outcome.ok && rep.diagnostics.is_empty() {
        rep.summary = format!("build failed:\n{}", outcome.output);
    }
    rep.data = json!({ "config": config, "build_dir": outcome.build_dir, "targets": targets, "ninja_output_tail": tail(&outcome.output, 40) });
    rep.elapsed_ms = t0.elapsed().as_millis();
    Ok(rep)
}

fn tail(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

pub fn exe_path(ws: &Workspace, config: &str, module: &str) -> Result<PathBuf> {
    let m = ws.modules.get(module).ok_or_else(|| anyhow!("unknown module {module}"))?;
    let out = m.file.output.clone().unwrap_or_else(|| module.to_string());
    Ok(ws.build_dir(config).join("bin").join(out))
}

/// Locate a sample or project directory by name.
pub fn find_project(ws: &Workspace, name: &str) -> Option<PathBuf> {
    let direct = PathBuf::from(name);
    if direct.join("project.toml").exists() {
        return Some(direct);
    }
    for base in ["samples", "projects", "benchmarks"] {
        let p = ws.root.join(base).join(name);
        if p.join("project.toml").exists() {
            return Some(p);
        }
    }
    None
}

pub fn run(ws: &Workspace, config: &str, target: &str, args: &[String]) -> Result<Report> {
    let t0 = Instant::now();
    // An executable module?
    if let Some(m) = ws.modules.get(target) {
        if m.file.kind != "executable" && m.file.kind != "test" {
            bail!("{target} is a {}; only executables and tests can be run", m.file.kind);
        }
        let outcome = build_targets(ws, config, &[target.to_string()], false)?;
        if !outcome.ok {
            let mut rep = Report::failure("run", "build failed");
            rep.diagnostics = parse_compiler_diagnostics(&outcome.output);
            return Ok(rep);
        }
        let exe = exe_path(ws, config, target)?;
        let status = toolchain::command(exe.to_str().unwrap()).args(args).current_dir(&ws.root).status()?;
        let mut rep = if status.success() { Report::success("run", format!("{target} exited 0")) } else { Report::failure("run", format!("{target} exited {}", status.code().unwrap_or(-1))) };
        rep.data = json!({ "exe": exe, "exit_code": status.code() });
        rep.elapsed_ms = t0.elapsed().as_millis();
        return Ok(rep);
    }
    // A sample project: bundle its TypeScript, then run it with the runtime module.
    let project = find_project(ws, target).ok_or_else(|| anyhow!("'{target}' is neither a module nor a project with project.toml"))?;
    let runtime = "pocket_runtime";
    let outcome = build_targets(ws, config, &[runtime.to_string()], false)?;
    if !outcome.ok {
        let mut rep = Report::failure("run", "runtime build failed");
        rep.diagnostics = parse_compiler_diagnostics(&outcome.output);
        return Ok(rep);
    }
    let bundle = bundle_project(ws, &project, None)?;
    let exe = exe_path(ws, config, runtime)?;
    let status = toolchain::command(exe.to_str().unwrap())
        .arg("--project")
        .arg(&project)
        .arg("--bundle")
        .arg(&bundle.out)
        .args(args)
        .current_dir(&ws.root)
        .status()?;
    let mut rep = if status.success() { Report::success("run", format!("{target} exited 0")) } else { Report::failure("run", format!("{target} exited {}", status.code().unwrap_or(-1))) };
    rep.data = json!({ "exe": exe, "project": project, "bundle": bundle.out, "modules": bundle.modules, "exit_code": status.code() });
    rep.elapsed_ms = t0.elapsed().as_millis();
    Ok(rep)
}

/// The UI font installed by `pocket setup` (the `file` dependency named noto-sans-cjk), if present.
pub fn ui_font(ws: &Workspace) -> Option<PathBuf> {
    let dep = ws.file.dependencies.iter().find(|d| d.name == "noto-sans-cjk")?;
    let file_name = dep.url.rsplit('/').next()?;
    let path = deps::prefix(ws, dep).join(file_name);
    if path.is_file() { Some(path) } else { None }
}

/// `pocket editor <project>`: bundle the editor (editor/) and the project, then run the runtime
/// with both bundles, paused, in a window.
pub fn editor(ws: &Workspace, config: &str, target: &str, args: &[String]) -> Result<Report> {
    let t0 = Instant::now();
    let project = find_project(ws, target).ok_or_else(|| anyhow!("'{target}' is not a project with project.toml"))?;
    let editor_dir = ws.root.join("editor");
    if !editor_dir.join("project.toml").exists() {
        bail!("editor/project.toml is missing");
    }
    let runtime = "pocket_runtime";
    let outcome = build_targets(ws, config, &[runtime.to_string()], false)?;
    if !outcome.ok {
        let mut rep = Report::failure("editor", "runtime build failed");
        rep.diagnostics = parse_compiler_diagnostics(&outcome.output);
        return Ok(rep);
    }
    let editor_bundle = bundle_project(ws, &editor_dir, None)?;
    let bundle = bundle_project(ws, &project, None)?;
    let exe = exe_path(ws, config, runtime)?;
    let status = toolchain::command(exe.to_str().unwrap())
        .arg("--project")
        .arg(&project)
        .arg("--bundle")
        .arg(&bundle.out)
        .arg("--editor")
        .arg(&editor_bundle.out)
        .arg("--paused")
        .arg("--title")
        .arg(format!("Pocket Editor - {}", project.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()))
        .args(args)
        .current_dir(&ws.root)
        .status()?;
    let mut rep = if status.success() { Report::success("editor", format!("editor exited 0 ({target})")) } else { Report::failure("editor", format!("editor exited {}", status.code().unwrap_or(-1))) };
    rep.data = json!({ "exe": exe, "project": project, "bundle": bundle.out, "editor_bundle": editor_bundle.out, "exit_code": status.code() });
    rep.elapsed_ms = t0.elapsed().as_millis();
    Ok(rep)
}

pub struct BundleResult {
    pub out: PathBuf,
    pub modules: Vec<String>,
}

pub fn bundle_project(ws: &Workspace, project: &Path, out: Option<&Path>) -> Result<BundleResult> {
    let (entry, project_file, name) = if project.is_file() {
        let name = project.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "bundle".into());
        (project.to_path_buf(), crate::ts::ProjectFile::default(), name)
    } else {
        let text = std::fs::read_to_string(project.join("project.toml")).with_context(|| format!("reading {}", project.join("project.toml").display()))?;
        let pf: crate::ts::ProjectFile = toml::from_str(&text).context("parsing project.toml")?;
        let entry = project.join(pf.entry.clone().unwrap_or_else(|| "scripts/main.ts".into()));
        let name = if pf.name.is_empty() { project.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "bundle".into()) } else { pf.name.clone() };
        (entry, pf, name)
    };
    let out = out.map(|p| p.to_path_buf()).unwrap_or_else(|| ws.root.join("build").join("ts").join(format!("{name}.js")));
    toolchain::ensure_dir(out.parent().unwrap())?;
    let sdk_dir = ws.root.join("sdk").join("runtime");
    let root = std::fs::canonicalize(&ws.root).unwrap_or(ws.root.clone());
    let result = crate::ts::bundle(&entry, &sdk_dir, &root, &out)?;
    // Hand the parsed project settings to the runtime as JSON next to the bundle.
    let mut settings = serde_json::to_value(&project_file)?;
    if let serde_json::Value::Object(map) = &mut settings {
        map.insert("name".into(), serde_json::Value::String(name.clone()));
        map.insert("dir".into(), serde_json::Value::String(std::fs::canonicalize(project).unwrap_or(project.to_path_buf()).to_string_lossy().into_owned()));
        if let Some(font) = ui_font(ws) {
            map.insert("font".into(), serde_json::Value::String(font.to_string_lossy().into_owned()));
        }
    }
    let settings_path = PathBuf::from(format!("{}.project.json", out.display()));
    std::fs::write(&settings_path, serde_json::to_string_pretty(&settings)?)?;
    Ok(BundleResult { out, modules: result.modules })
}

/// Bundle every project under samples/ (used by `pocket test` so end-to-end tests have bundles).
pub fn bundle_all_samples(ws: &Workspace) -> Result<Vec<PathBuf>> {
    let mut outs = vec![];
    let samples = ws.root.join("samples");
    if !samples.is_dir() {
        return Ok(outs);
    }
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&samples)?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.join("project.toml").exists()).collect();
    dirs.sort();
    for d in dirs {
        outs.push(bundle_project(ws, &d, None)?.out);
    }
    Ok(outs)
}

/// Run a project with the runtime and capture its JSON report (the runtime must be given --json).
pub fn run_captured(ws: &Workspace, config: &str, target: &str, args: &[String]) -> Result<Report> {
    let t0 = Instant::now();
    let project = find_project(ws, target).ok_or_else(|| anyhow!("'{target}' is not a project with project.toml"))?;
    let runtime = "pocket_runtime";
    let outcome = build_targets(ws, config, &[runtime.to_string()], false)?;
    if !outcome.ok {
        let mut rep = Report::failure("run", "runtime build failed");
        rep.diagnostics = parse_compiler_diagnostics(&outcome.output);
        return Ok(rep);
    }
    let bundle = bundle_project(ws, &project, None)?;
    let exe = exe_path(ws, config, runtime)?;
    let output = toolchain::command(exe.to_str().unwrap()).arg("--project").arg(&project).arg("--bundle").arg(&bundle.out).args(args).current_dir(&ws.root).output()?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap_or(json!({ "raw": stdout }));
    let ok = output.status.success() && report.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    let mut rep = if ok { Report::success("run", format!("{target} finished")) } else { Report::failure("run", format!("{target} exited {}", output.status.code().unwrap_or(-1))) };
    rep.data = json!({ "project": project, "report": report, "stderr_tail": tail(&String::from_utf8_lossy(&output.stderr), 20) });
    rep.elapsed_ms = t0.elapsed().as_millis();
    Ok(rep)
}

pub fn ts_bundle(ws: &Workspace, project: &Path, out: Option<&Path>) -> Result<Report> {
    let t0 = Instant::now();
    let b = bundle_project(ws, project, out)?;
    let mut rep = Report::success("ts", format!("bundled {} modules -> {}", b.modules.len(), b.out.display()));
    rep.data = json!({ "out": b.out, "modules": b.modules });
    rep.elapsed_ms = t0.elapsed().as_millis();
    Ok(rep)
}

pub fn test(ws: &Workspace, config: &str, filter: Option<&str>) -> Result<Report> {
    let t0 = Instant::now();
    let tests: Vec<String> = ws.modules.iter().filter(|(n, m)| m.file.kind == "test" && filter.map(|f| n.contains(f)).unwrap_or(true)).map(|(n, _)| n.clone()).collect();
    if tests.is_empty() {
        return Ok(Report::failure("test", "no test modules matched"));
    }
    let outcome = build_targets(ws, config, &tests, false)?;
    if !outcome.ok {
        let mut rep = Report::failure("test", "build failed");
        rep.diagnostics = parse_compiler_diagnostics(&outcome.output);
        return Ok(rep);
    }
    let mut bundles = bundle_all_samples(ws)?;
    // The editor is a TypeScript project too; runtime tests open it headless.
    if ws.root.join("editor").join("project.toml").exists() {
        bundles.push(bundle_project(ws, &ws.root.join("editor"), None)?.out);
    }
    let mut results = vec![];
    let mut failed = 0;
    for t in &tests {
        let exe = exe_path(ws, config, t)?;
        let started = Instant::now();
        let out = toolchain::command(exe.to_str().unwrap()).current_dir(&ws.root).env("POCKET_ROOT", &ws.root).output()?;
        let ok = out.status.success();
        if !ok {
            failed += 1;
        }
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        results.push(json!({ "module": t, "ok": ok, "exit_code": out.status.code(), "elapsed_ms": started.elapsed().as_millis(), "output_tail": tail(&text, 30) }));
        if !ok {
            eprintln!("--- {t} failed ---\n{}", tail(&text, 60));
        }
    }
    // TypeScript tests: every tests/ts/*.test.ts(x) is bundled and run headless for one frame; the
    // runner exposes its results as state.__tests.
    let ts_dir = ws.root.join("tests").join("ts");
    let mut ts_results = vec![];
    if ts_dir.is_dir() && filter.map(|f| "ts".contains(f) || "typescript".contains(f) || f == "ts").unwrap_or(true) {
        let runtime = "pocket_runtime";
        let outcome = build_targets(ws, config, &[runtime.to_string()], false)?;
        if !outcome.ok {
            let mut rep = Report::failure("test", "runtime build failed");
            rep.diagnostics = parse_compiler_diagnostics(&outcome.output);
            return Ok(rep);
        }
        let exe = exe_path(ws, config, runtime)?;
        let mut files: Vec<PathBuf> = std::fs::read_dir(&ts_dir)?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| { let n = p.to_string_lossy(); n.ends_with(".test.ts") || n.ends_with(".test.tsx") }).collect();
        files.sort();
        for f in files {
            let stem = f.file_name().unwrap().to_string_lossy().replace(".test.tsx", "").replace(".test.ts", "");
            let out = ws.root.join("build").join("ts").join("tests").join(format!("{stem}.js"));
            let started = Instant::now();
            let bundle = match bundle_project(ws, &f, Some(&out)) {
                Ok(b) => b,
                Err(e) => {
                    failed += 1;
                    ts_results.push(json!({ "file": f, "ok": false, "error": format!("{e:#}") }));
                    eprintln!("--- {} failed to bundle ---\n{e:#}", f.display());
                    continue;
                }
            };
            let output = toolchain::command(exe.to_str().unwrap())
                .args(["--headless", "--frames", "1", "--json", "--size", "320x240", "--log-level", "warn", "--bundle"])
                .arg(&bundle.out)
                .current_dir(&ws.root)
                .env("POCKET_ROOT", &ws.root)
                .output()?;
            let text = String::from_utf8_lossy(&output.stdout).to_string();
            let report: serde_json::Value = serde_json::from_str(&text).unwrap_or(json!({}));
            let tests_json = report.get("state").and_then(|s| s.get("__tests")).cloned().unwrap_or(json!(null));
            let passed = tests_json.get("passed").and_then(|v| v.as_u64()).unwrap_or(0);
            let failed_n = tests_json.get("failed").and_then(|v| v.as_u64()).unwrap_or(0);
            let ok = output.status.success() && report.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) && !tests_json.is_null() && failed_n == 0;
            if !ok {
                failed += 1;
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                eprintln!("--- {} failed ---\n{}\n{}", f.display(), serde_json::to_string_pretty(&tests_json).unwrap_or_default(), tail(&stderr, 20));
                if let Some(errs) = report.get("errors") {
                    eprintln!("{}", serde_json::to_string_pretty(errs).unwrap_or_default());
                }
            }
            ts_results.push(json!({ "file": f, "ok": ok, "passed": passed, "failed": failed_n, "elapsed_ms": started.elapsed().as_millis(), "results": tests_json.get("results").cloned().unwrap_or(json!([])) }));
        }
    }
    let total = tests.len() + ts_results.len();
    let mut rep = if failed == 0 { Report::success("test", format!("{total} test modules passed")) } else { Report::failure("test", format!("{failed} of {total} test modules failed")) };
    rep.data = json!({ "config": config, "results": results, "ts": ts_results, "bundles": bundles });
    rep.elapsed_ms = t0.elapsed().as_millis();
    Ok(rep)
}

pub fn gen(ws: &Workspace, check: bool) -> Result<Report> {
    let t0 = Instant::now();
    let g = crate::gen::generate(ws, check)?;
    let mut rep = if check && !g.changed.is_empty() {
        Report::failure("gen", format!("{} generated files are out of date", g.changed.len()))
    } else {
        Report::success("gen", format!("{} components from {} metadata files; {} files {}", g.components, g.inputs.len(), g.changed.len(), if check { "would change" } else { "rewritten" }))
    };
    rep.data = json!({ "inputs": g.inputs, "outputs": g.outputs, "changed": g.changed, "components": g.components });
    rep.elapsed_ms = t0.elapsed().as_millis();
    Ok(rep)
}

pub fn clean(ws: &Workspace) -> Result<Report> {
    let b = ws.root.join("build");
    if b.exists() {
        std::fs::remove_dir_all(&b)?;
    }
    let _ = std::fs::remove_file(ws.root.join("compile_commands.json"));
    Ok(Report::success("clean", "removed build/"))
}

pub fn graph(ws: &Workspace) -> Result<Report> {
    let g = Graph::resolve(ws)?;
    let mut text = String::new();
    let mut data = vec![];
    for (name, m) in &g.modules {
        text.push_str(&format!("{name} [{}] {} sources; links {}; deps {}\n", m.kind, m.sources.len(), m.link_modules.join(","), m.link_deps.join(",")));
        data.push(json!({ "name": name, "kind": m.kind, "dir": m.dir, "sources": m.sources.len(), "link_modules": m.link_modules, "link_deps": m.link_deps }));
    }
    let mut rep = Report::success("graph", format!("{} modules\n{}", g.modules.len(), text));
    rep.data = json!({ "modules": data });
    Ok(rep)
}
