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

pub fn setup(ws: &Workspace, force: bool, target: &str) -> Result<Report> {
    let t0 = Instant::now();
    let mut log = vec![];
    let mut statuses = vec![];
    for d in &ws.file.dependencies {
        statuses.push(deps::setup_one(ws, d, force, &mut log, target)?);
    }
    let ready = statuses.iter().filter(|s| s.state == "ready" || s.state == "installed" || s.state == "system").count();
    let mut rep = Report::success("setup", format!("{} dependencies ready for {target}", ready));
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
    // Web readiness (docs/web.md): the Emscripten SDK and fontTools for font subsetting.
    let emsdk = toolchain::emsdk().ok().map(|s| s.root.to_string_lossy().into_owned());
    let fonttools = toolchain::command("python3").args(["-c", "import fontTools; print(fontTools.version)"]).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    rep.data = json!({ "toolchain": tc, "dependencies": statuses, "modules": ws.modules.keys().collect::<Vec<_>>(), "web": { "emsdk": emsdk, "fonttools": fonttools } });
    Ok(rep)
}

fn ensure_deps(ws: &Workspace, target: &str) -> Result<()> {
    let missing: Vec<String> = ws.file.dependencies.iter().filter(|d| !deps::is_ready_for(ws, d, target)).map(|d| d.name.clone()).collect();
    if !missing.is_empty() {
        let hint = if target == "native" { "pocket setup".to_string() } else { format!("pocket setup --target {target}") };
        bail!("dependencies not set up for {target}: {} (run `{hint}`)", missing.join(", "));
    }
    Ok(())
}

pub struct BuildOutcome {
    pub ok: bool,
    pub output: String,
    pub build_dir: PathBuf,
}

pub fn build_targets(ws: &Workspace, config: &str, targets: &[String], generate_only: bool) -> Result<BuildOutcome> {
    let target = ws.target_of(config)?;
    ensure_deps(ws, &target)?;
    crate::gen::generate(ws, false)?;
    let tc = toolchain::detect_for(&target)?;
    let graph = Graph::resolve_for(ws, &target)?;
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
    if target == "wasm" {
        toolchain::em_env(&mut cmd, &toolchain::emsdk()?);
    }
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
    let status = runtime_command(ws, &exe)
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
    dep_file(ws, "noto-sans-cjk")
}

/// The fonts the UI falls back to for what the main font lacks: the bundled Noto Sans Arabic
/// (Arabic script; right-to-left text needs a face that has it).
pub fn ui_fallback_fonts(ws: &Workspace) -> Vec<PathBuf> {
    dep_file(ws, "noto-sans-arabic").into_iter().collect()
}

fn dep_file(ws: &Workspace, name: &str) -> Option<PathBuf> {
    let dep = ws.file.dependencies.iter().find(|d| d.name == name)?;
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
    let status = runtime_command(ws, &exe)
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

/// `pocket new <name>`: a runnable project with a scene, a prefab and a script that already
/// exposes state, so `pocket run <name>` and `pocket editor <name>` work immediately.
fn check_project_name(name: &str) -> Result<()> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        bail!("project names use letters, digits, '-' and '_'");
    }
    Ok(())
}

/// The first line of a project's entry script without its comment marks: what a sample is about.
fn project_summary(project: &Path) -> String {
    let entry = std::fs::read_to_string(project.join("project.toml"))
        .ok()
        .and_then(|t| toml::from_str::<crate::ts::ProjectFile>(&t).ok())
        .and_then(|pf| pf.entry)
        .unwrap_or_else(|| "scripts/main.ts".into());
    std::fs::read_to_string(project.join(entry))
        .ok()
        .and_then(|s| s.lines().next().map(|l| l.trim_start_matches('/').trim().to_string()))
        .unwrap_or_default()
}

/// The samples a project can start from: `pocket new <name> --from <sample>`.
pub fn list_templates(ws: &Workspace) -> Result<Report> {
    let t0 = Instant::now();
    let samples = ws.root.join("samples");
    let mut names: Vec<String> = std::fs::read_dir(&samples)
        .with_context(|| format!("reading {}", samples.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("project.toml").exists())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let rows: Vec<serde_json::Value> = names.iter().map(|n| json!({ "name": n, "about": project_summary(&samples.join(n)) })).collect();
    let mut text = String::from("samples to start from (pocket new <name> --from <sample>):\n");
    for r in &rows {
        text.push_str(&format!("  {:<12}{}\n", r["name"].as_str().unwrap_or(""), r["about"].as_str().unwrap_or("")));
    }
    let mut rep = Report::success("new", text.trim_end().to_string());
    rep.data = json!({ "templates": rows });
    rep.elapsed_ms = t0.elapsed().as_millis();
    Ok(rep)
}

/// Copy a project directory as a new project: everything but the editor's `.pocket/` state and
/// build leftovers, with the name and the window title in project.toml replaced. Returns the
/// number of files copied.
pub fn copy_template(src: &Path, dst: &Path, name: &str) -> Result<usize> {
    fn walk(src: &Path, dst: &Path, count: &mut usize) -> Result<()> {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src).with_context(|| format!("reading {}", src.display()))? {
            let entry = entry?;
            let file_name = entry.file_name();
            let n = file_name.to_string_lossy();
            if n == ".pocket" || n == "build" || n == "dist" || n == ".DS_Store" {
                continue;
            }
            let from = entry.path();
            let to = dst.join(&file_name);
            if from.is_dir() {
                walk(&from, &to, count)?;
            } else {
                std::fs::copy(&from, &to).with_context(|| format!("copying {}", from.display()))?;
                *count += 1;
            }
        }
        Ok(())
    }
    let mut count = 0;
    walk(src, dst, &mut count)?;
    let toml_path = dst.join("project.toml");
    let text = std::fs::read_to_string(&toml_path).with_context(|| format!("reading {}", toml_path.display()))?;
    let mut out = String::new();
    let mut section = String::new();
    let mut named = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            section = trimmed.to_string();
        }
        if section.is_empty() && !named && trimmed.starts_with("name") && trimmed[4..].trim_start().starts_with('=') {
            out.push_str(&format!("name = \"{name}\"\n"));
            named = true;
            continue;
        }
        if section == "[window]" && trimmed.starts_with("title") && trimmed[5..].trim_start().starts_with('=') {
            out.push_str(&format!("title = \"{name}\"\n"));
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !named {
        out = format!("name = \"{name}\"\n{out}");
    }
    std::fs::write(&toml_path, out)?;
    Ok(count)
}

/// A new project that starts as a copy of a sample (`pocket new <name> --from sprites`).
pub fn new_from_template(ws: &Workspace, name: &str, dir: &Path, from: &str) -> Result<Report> {
    let t0 = Instant::now();
    check_project_name(name)?;
    let src = find_project(ws, from).ok_or_else(|| anyhow!("no sample or project named '{from}' (pocket new --list shows the samples)"))?;
    let base = if dir.is_absolute() { dir.to_path_buf() } else { ws.root.join(dir) };
    let project = base.join(name);
    if project.exists() {
        bail!("{} already exists", project.display());
    }
    let files = copy_template(&src, &project, name)?;
    let mut rep = Report::success("new", format!("created {} from {} ({files} files; run: pocket run {name}; editor: pocket editor {name})", project.display(), src.display()));
    rep.data = json!({ "project": project, "from": src, "files": files });
    rep.elapsed_ms = t0.elapsed().as_millis();
    Ok(rep)
}

pub fn new_project(ws: &Workspace, name: &str, dir: &Path) -> Result<Report> {
    let t0 = Instant::now();
    check_project_name(name)?;
    let base = if dir.is_absolute() { dir.to_path_buf() } else { ws.root.join(dir) };
    let project = base.join(name);
    if project.exists() {
        bail!("{} already exists", project.display());
    }
    std::fs::create_dir_all(project.join("scripts"))?;
    std::fs::create_dir_all(project.join("assets"))?;
    std::fs::create_dir_all(project.join("prefabs"))?;
    std::fs::write(project.join("project.toml"), format!("name = \"{name}\"\nentry = \"scripts/main.ts\"\nscene = \"scene.json\"\n\n[window]\nwidth = 960\nheight = 540\ntitle = \"{name}\"\n\n[physics]\ngravity = [0.0, -9.8, 0.0]\n\n# Actions instead of keys: keyboard and gamepad both work, and agents can hold an action by name.\n[input.actions]\nmove_x = {{ negative = [\"A\", \"Left\", \"pad:dpad_left\"], positive = [\"D\", \"Right\", \"pad:dpad_right\"], axis = [\"pad:leftx\"] }}\nmove_z = {{ negative = [\"W\", \"Up\", \"pad:dpad_up\"], positive = [\"S\", \"Down\", \"pad:dpad_down\"], axis = [\"pad:lefty\"] }}\ndrop = [\"Space\", \"pad:a\"]\n"))?;
    std::fs::write(project.join("scene.json"), r#"{
  "format": "pocket-scene",
  "entities": [
    { "name": "Ground", "components": { "Transform": { "position": { "x": 0, "y": -0.5, "z": 0 }, "scale": { "x": 20, "y": 1, "z": 20 } }, "MeshRenderer": { "mesh": "cube", "color": { "r": 0.35, "g": 0.4, "b": 0.32, "a": 1 } }, "RigidBody": { "kind": 0 }, "Collider": { "shape": 0 } } },
    { "name": "Player", "components": { "Transform": { "position": { "x": 0, "y": 0.5, "z": 0 } }, "MeshRenderer": { "mesh": "sphere", "color": { "r": 0.2, "g": 0.6, "b": 0.9, "a": 1 } }, "Health": { "current": 100, "max": 100 } } },
    { "name": "Sun", "components": { "Transform": { "rotation": { "x": -0.4, "y": 0.2, "z": 0.1, "w": 0.89 } }, "Light": { "kind": 0, "intensity": 1.2 } } },
    { "name": "Camera", "components": { "Transform": { "position": { "x": 0, "y": 6, "z": 10 }, "rotation": { "x": -0.26, "y": 0, "z": 0, "w": 0.97 } }, "Camera": {} } }
  ]
}
"#)?;
    std::fs::write(project.join("prefabs").join("crate.json"), r#"{
  "format": "pocket-scene",
  "entities": [
    { "name": "Crate", "components": { "Transform": { "position": { "x": 0, "y": 3, "z": 0 } }, "MeshRenderer": { "mesh": "cube", "color": { "r": 0.8, "g": 0.55, "b": 0.25, "a": 1 } }, "RigidBody": { "kind": 1, "mass": 1 }, "Collider": { "shape": 0 } } }
  ]
}
"#)?;
    std::fs::write(project.join("scripts").join("main.ts"), format!(r#"// {name}: move the player with WASD, drop crates with Space. Every value that matters is exposed,
// so `pocket run {name} -- --headless --frames 300 --json` reports it and an agent can read it.
import {{ events, expose, input, log, onStart, onTick, world }} from "pocket";

let player = 0;
let crates = 0;
let x = 0;
let z = 0;

onStart(() => {{
    player = world.find("Player") ?? 0;
    log("{name} started", {{ entities: world.summary().entities }});
}});

onTick((t) => {{
    const speed = 4;
    x += input.axis("move_x") * speed * t.dt;
    z += input.axis("move_z") * speed * t.dt;
    if (player) world.set(player, "Transform", {{ position: {{ x, y: 0.5, z }} }});
    if (input.pressed("drop")) {{
        const id = world.instantiate("prefabs/crate.json", {{ components: {{ Transform: {{ position: {{ x, y: 3, z }} }} }} }});
        crates++;
        events.emit("crate.dropped", {{ id, x, z }}, {{ subject: id }});
    }}
}});

expose("player.x", () => Number(x.toFixed(3)));
expose("player.z", () => Number(z.toFixed(3)));
expose("crates", () => crates);
"#))?;
    std::fs::write(project.join("assets").join(".gitkeep"), "")?;
    let mut rep = Report::success("new", format!("created {} (run: pocket run {name}; editor: pocket editor {name})", project.display()));
    rep.data = json!({ "project": project, "files": ["project.toml", "scene.json", "prefabs/crate.json", "scripts/main.ts", "assets/"] });
    rep.elapsed_ms = t0.elapsed().as_millis();
    Ok(rep)
}

pub struct BundleResult {
    pub out: PathBuf,
    pub modules: Vec<String>,
}

/// A runtime process as the tool starts it: it knows the workspace and this tool (POCKET_ROOT,
/// POCKET_TOOL), so `project.apply` can bundle and type-check the project it is running.
pub fn runtime_command(ws: &Workspace, exe: &Path) -> std::process::Command {
    let mut cmd = toolchain::command(exe.to_str().unwrap());
    cmd.env("POCKET_ROOT", &ws.root);
    if let Ok(me) = std::env::current_exe() {
        cmd.env("POCKET_TOOL", me);
    }
    cmd
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
        if !map.contains_key("font_fallbacks") {
            let fallbacks: Vec<serde_json::Value> = ui_fallback_fonts(ws).iter().map(|p| serde_json::Value::String(p.to_string_lossy().into_owned())).collect();
            map.insert("font_fallbacks".into(), serde_json::Value::Array(fallbacks));
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
        // Gameplay scenarios and perception benchmarks next to the project: build/ts/<sample>.<dir>.<file>.js each.
        for (file, out) in scenario_files(ws, &d).into_iter().chain(bench_files(ws, &d)) {
            outs.push(bundle_project(ws, &file, Some(&out))?.out);
        }
    }
    Ok(outs)
}

/// The scenario scripts of a project (`<project>/scenarios/*.ts`) with their bundle paths.
pub fn scenario_files(ws: &Workspace, project: &Path) -> Vec<(PathBuf, PathBuf)> {
    script_files(ws, project, "scenarios")
}

/// The perception benchmarks of a project (`<project>/benches/*.ts`) with their bundle paths.
pub fn bench_files(ws: &Workspace, project: &Path) -> Vec<(PathBuf, PathBuf)> {
    script_files(ws, project, "benches")
}

fn script_files(ws: &Workspace, project: &Path, kind: &str) -> Vec<(PathBuf, PathBuf)> {
    let dir = project.join(kind);
    let name = project.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "project".into());
    let mut files: Vec<PathBuf> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().map(|x| x == "ts" || x == "tsx").unwrap_or(false)).collect(),
        Err(_) => return vec![],
    };
    files.sort();
    files
        .into_iter()
        .map(|f| {
            let stem = f.file_stem().unwrap().to_string_lossy().to_string();
            let out = ws.root.join("build").join("ts").join(format!("{name}.{kind}.{stem}.js"));
            (f, out)
        })
        .collect()
}

/// Run every gameplay scenario of a project at several seeds and summarise (docs/design/scenarios.md).
pub fn scenario(ws: &Workspace, config: &str, target: &str, file: Option<&str>, seeds: u64, frames: i64, only: Option<&str>) -> Result<Report> {
    run_scripts(ws, config, target, file, seeds, frames, only, "scenarios")
}

/// Run a project's perception benchmarks (benches/*.ts) once each and report what every answer
/// cost through the instruments against frame-by-frame vision (docs/design/scenarios.md).
pub fn bench(ws: &Workspace, config: &str, target: &str, file: Option<&str>, frames: i64, only: Option<&str>) -> Result<Report> {
    let mut rep = run_scripts(ws, config, target, file, 1, frames, only, "benches")?;
    let mut rows = vec![];
    let (mut answered, mut correct, mut tokens_sum, mut frame_sum) = (0u64, 0u64, 0u64, 0u64);
    for r in rep.data.get("results").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        let Some(question) = r.get("scenario").and_then(|n| n.as_str()).map(String::from) else { continue };
        let run = r.get("runs").and_then(|v| v.as_array()).and_then(|a| a.first()).cloned().unwrap_or(json!({}));
        let ok = run.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        let b = run.get("state").and_then(|s| s.get("__bench")).cloned().unwrap_or(json!(null));
        let tokens = b.get("tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let frame_tokens = b.get("frame_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let ticks = b.get("ticks").and_then(|v| v.as_u64()).unwrap_or(0);
        let methods: Vec<String> = b.get("commands").and_then(|c| c.as_array()).map(|a| a.iter().filter_map(|c| c.get("method").and_then(|m| m.as_str()).map(String::from)).collect()).unwrap_or_default();
        let is_correct = b.get("correct").and_then(|v| v.as_bool()).unwrap_or(false);
        answered += 1;
        if ok && is_correct { correct += 1; tokens_sum += tokens; frame_sum += frame_tokens; }
        rows.push(json!({ "question": question, "ok": ok, "correct": is_correct, "answer": b.get("answer").cloned().unwrap_or(json!(null)), "truth": b.get("truth").cloned().unwrap_or(json!(null)), "tokens": tokens, "bytes_in": b.get("bytes_in").cloned().unwrap_or(json!(0)), "bytes_out": b.get("bytes_out").cloned().unwrap_or(json!(0)), "commands": methods, "ticks": ticks, "frame_tokens": frame_tokens, "image": b.get("image").cloned().unwrap_or(json!(null)), "ratio": b.get("ratio").cloned().unwrap_or(json!(null)), "error": run.get("error").cloned().unwrap_or(json!(null)), "step": run.get("step").cloned().unwrap_or(json!(null)) }));
    }
    let mut summary = format!("{correct} of {answered} perception benchmarks answered correctly; {tokens_sum} tokens through the instruments against {frame_sum} for frame-by-frame vision{}", if tokens_sum > 0 { format!(" ({:.0}x)", frame_sum as f64 / tokens_sum as f64) } else { String::new() });
    for row in &rows {
        let ok = row["ok"].as_bool().unwrap_or(false) && row["correct"].as_bool().unwrap_or(false);
        let methods = row["commands"].as_array().map(|a| a.iter().filter_map(|m| m.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
        if ok {
            summary.push_str(&format!("\n  ok   {}: {} tokens over {} ({} frames of play would cost {} as images, {}x)", row["question"].as_str().unwrap_or("?"), row["tokens"], methods, row["ticks"], row["frame_tokens"], row["ratio"]));
        } else {
            summary.push_str(&format!("\n  FAIL {}: {} at '{}' (answer {}, truth {})", row["question"].as_str().unwrap_or("?"), row["error"].as_str().map(String::from).unwrap_or_else(|| row["error"].to_string()), row["step"].as_str().unwrap_or("-"), row["answer"], row["truth"]));
        }
    }
    for r in rep.data.get("results").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        if r.get("scenario").is_none() { summary.push_str(&format!("\n  FAIL {}: {}", r.get("file").and_then(|f| f.as_str()).unwrap_or("?"), r.get("error").and_then(|e| e.as_str()).unwrap_or("?"))); }
    }
    let ok = rep.ok && correct == answered;
    rep.summary = summary;
    rep.ok = ok;
    rep.command = "bench".into();
    rep.data["benches"] = json!(rows);
    rep.data["totals"] = json!({ "answered": answered, "correct": correct, "tokens": tokens_sum, "frame_tokens": frame_sum });
    Ok(rep)
}

#[allow(clippy::too_many_arguments)]
fn run_scripts(ws: &Workspace, config: &str, target: &str, file: Option<&str>, seeds: u64, frames: i64, only: Option<&str>, kind: &str) -> Result<Report> {
    let t0 = Instant::now();
    let label = if kind == "benches" { "bench" } else { "scenario" };
    let project = find_project(ws, target).ok_or_else(|| anyhow!("'{target}' is not a project with project.toml"))?;
    let runtime = "pocket_runtime";
    let outcome = build_targets(ws, config, &[runtime.to_string()], false)?;
    if !outcome.ok {
        let mut rep = Report::failure(label, "runtime build failed");
        rep.diagnostics = parse_compiler_diagnostics(&outcome.output);
        return Ok(rep);
    }
    let exe = exe_path(ws, config, runtime)?;
    let project_bundle = bundle_project(ws, &project, None)?;
    let files: Vec<(PathBuf, PathBuf)> = match file {
        Some(f) => {
            let path = if Path::new(f).is_absolute() { PathBuf::from(f) } else if project.join(f).exists() { project.join(f) } else { ws.root.join(f) };
            let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "scenario".into());
            vec![(path, ws.root.join("build").join("ts").join(format!("{}.{kind}.{stem}.js", project.file_name().unwrap().to_string_lossy())))]
        }
        None => script_files(ws, &project, kind),
    };
    if files.is_empty() {
        return Ok(Report::failure(label, format!("{target} has no {kind} (put .ts files in {}/{kind}/)", project.display())));
    }
    let run_one = |bundle: &Path, name: Option<&str>, seed: u64, frames: i64| -> Result<serde_json::Value> {
        let mut cmd = toolchain::command(exe.to_str().unwrap());
        cmd.arg("--project").arg(&project).arg("--bundle").arg(&project_bundle.out).arg("--scenario").arg(bundle);
        cmd.args(["--headless", "--json", "--log-level", "warn", "--size", "320x180", "--no-tick-hash"]);
        cmd.arg("--frames").arg(frames.to_string()).arg("--seed").arg(seed.to_string());
        if let Some(n) = name { cmd.arg("--scenario-name").arg(n); }
        let output = cmd.current_dir(&ws.root).output()?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let mut report: serde_json::Value = serde_json::from_str(&stdout).unwrap_or(json!({ "raw": stdout }));
        if !output.status.success() { report["exit_code"] = json!(output.status.code().unwrap_or(-1)); }
        if let Some(obj) = report.as_object_mut() { obj.insert("stderr_tail".into(), json!(tail(&String::from_utf8_lossy(&output.stderr), 12))); }
        Ok(report)
    };
    let mut results = vec![];
    let mut all_ok = true;
    let mut total_runs = 0u64;
    let mut total_passed = 0u64;
    for (src, out) in &files {
        let bundle = match bundle_project(ws, src, Some(out)) {
            Ok(b) => b,
            Err(e) => {
                all_ok = false;
                results.push(json!({ "file": src, "ok": false, "error": format!("{e:#}") }));
                continue;
            }
        };
        // Discovery: one frame lists the scenarios the bundle defines.
        let probe = run_one(&bundle.out, None, 1, 1)?;
        let names: Vec<String> = probe.get("state").and_then(|s| s.get("__scenarios")).and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|n| n.as_str().map(String::from)).collect()).unwrap_or_default();
        if names.is_empty() {
            all_ok = false;
            results.push(json!({ "file": src, "ok": false, "error": "the bundle defines no scenarios (call scenario(name, build))", "probe": probe }));
            continue;
        }
        for name in names.iter().filter(|n| only.map(|o| n.contains(o)).unwrap_or(true)) {
            let mut runs = vec![];
            let mut passed = 0u64;
            let mut ticks_to_pass: Vec<u64> = vec![];
            for seed in 1..=seeds.max(1) {
                let started = Instant::now();
                let rep = run_one(&bundle.out, Some(name), seed, frames)?;
                let sc = rep.get("state").and_then(|s| s.get("__scenario")).cloned().unwrap_or(json!(null));
                let status = sc.get("status").and_then(|s| s.as_str()).unwrap_or("missing").to_string();
                let ok = status == "passed" && rep.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                let ticks = sc.get("ticks").and_then(|t| t.as_u64()).unwrap_or(0);
                let error = if ok { json!(null) } else if status == "running" { json!(format!("still running after {frames} frames (raise --frames or shorten the scenario)")) } else { sc.get("error").cloned().unwrap_or_else(|| json!(rep.get("errors").cloned().unwrap_or(json!(status)))) };
                if ok { passed += 1; ticks_to_pass.push(ticks); }
                total_runs += 1;
                if ok { total_passed += 1; }
                runs.push(json!({ "seed": seed, "ok": ok, "status": status, "ticks": ticks, "step": sc.get("label").cloned().unwrap_or(json!(null)), "error": error, "report": sc.get("report").cloned().unwrap_or(json!(null)), "bots": sc.get("bots").cloned().unwrap_or(json!(null)), "state": rep.get("state").cloned().unwrap_or(json!(null)), "elapsed_ms": started.elapsed().as_millis() }));
            }
            let n = runs.len() as u64;
            if passed < n { all_ok = false; }
            let (min, max, avg) = if ticks_to_pass.is_empty() { (0, 0, 0.0) } else { (*ticks_to_pass.iter().min().unwrap(), *ticks_to_pass.iter().max().unwrap(), ticks_to_pass.iter().sum::<u64>() as f64 / ticks_to_pass.len() as f64) };
            results.push(json!({ "file": src, "scenario": name, "seeds": n, "passed": passed, "failed": n - passed, "ticks": { "min": min, "max": max, "avg": avg }, "runs": runs }));
        }
    }
    let mut summary = format!("{total_passed} of {total_runs} scenario runs passed ({} scenarios, {} seeds each)", results.iter().filter(|r| r.get("scenario").is_some()).count(), seeds.max(1));
    for r in &results {
        match r.get("scenario").and_then(|n| n.as_str()) {
            Some(name) => {
                let passed = r.get("passed").and_then(|v| v.as_u64()).unwrap_or(0);
                let n = r.get("seeds").and_then(|v| v.as_u64()).unwrap_or(0);
                let ticks = r.get("ticks").cloned().unwrap_or(json!({}));
                summary.push_str(&format!("\n  {} {name}: {passed}/{n} passed", if passed == n { "ok " } else { "FAIL" }));
                if passed > 0 { summary.push_str(&format!(", ticks to pass {}..{} (avg {:.0})", ticks.get("min").and_then(|v| v.as_u64()).unwrap_or(0), ticks.get("max").and_then(|v| v.as_u64()).unwrap_or(0), ticks.get("avg").and_then(|v| v.as_f64()).unwrap_or(0.0))); }
                // The first passing seed's report (g.report entries) and its bots, one line each.
                if let Some(run) = r.get("runs").and_then(|v| v.as_array()).and_then(|a| a.iter().find(|run| run.get("ok").and_then(|v| v.as_bool()).unwrap_or(false))) {
                    let seed = run.get("seed").and_then(|v| v.as_u64()).unwrap_or(0);
                    if let Some(report) = run.get("report").and_then(|v| v.as_object()).filter(|o| !o.is_empty()) {
                        let parts: Vec<String> = report.iter().map(|(k, v)| format!("{k}={}", compact_value(v))).collect();
                        summary.push_str(&format!("\n      report (seed {seed}): {}", parts.join(", ")));
                    }
                    if let Some(bots) = run.get("bots").and_then(|v| v.as_array()).filter(|a| !a.is_empty()) {
                        let parts: Vec<String> = bots.iter().map(|b| format!("{} ({} ticks, {} holds, {} presses)", b.get("name").and_then(|v| v.as_str()).unwrap_or("?"), b.get("ticks").and_then(|v| v.as_u64()).unwrap_or(0), b.get("holds").and_then(|v| v.as_u64()).unwrap_or(0), b.get("presses").and_then(|v| v.as_u64()).unwrap_or(0))).collect();
                        summary.push_str(&format!("\n      bots (seed {seed}): {}", parts.join(", ")));
                    }
                }
                for run in r.get("runs").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
                    if run.get("ok").and_then(|o| o.as_bool()).unwrap_or(false) { continue; }
                    summary.push_str(&format!("\n      seed {}: {} at '{}': {}", run.get("seed").and_then(|v| v.as_u64()).unwrap_or(0), run.get("status").and_then(|v| v.as_str()).unwrap_or("?"), run.get("step").and_then(|v| v.as_str()).unwrap_or("-"), run.get("error").map(|e| if e.is_string() { e.as_str().unwrap().to_string() } else { e.to_string() }).unwrap_or_default()));
                }
            }
            None => summary.push_str(&format!("\n  FAIL {}: {}", r.get("file").and_then(|f| f.as_str()).unwrap_or("?"), r.get("error").and_then(|e| e.as_str()).unwrap_or("?"))),
        }
    }
    let mut rep = if all_ok { Report::success(label, summary) } else { Report::failure(label, summary) };
    rep.data = json!({ "project": project, "config": config, "seeds": seeds, "frames": frames, "kind": kind, "results": results });
    rep.elapsed_ms = t0.elapsed().as_millis();
    Ok(rep)
}

/// A report value on one line: numbers to three decimals, strings bare, the rest as JSON.
fn compact_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Number(n) => n.as_f64().map(|f| if f.fract() == 0.0 && f.abs() < 1e15 { format!("{}", f as i64) } else { format!("{f:.3}") }).unwrap_or_else(|| n.to_string()),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
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
    let output = runtime_command(ws, &exe).arg("--project").arg(&project).arg("--bundle").arg(&bundle.out).args(args).current_dir(&ws.root).output()?;
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
    // Types: the workspace's TypeScript read against the SDK's (`pocket check`).
    let mut types_result = json!(null);
    if filter.map(|f| "types".contains(f) || f == "check").unwrap_or(true) {
        let started = Instant::now();
        match crate::check::check(ws, None) {
            Ok(rep) => {
                if !rep.ok {
                    failed += 1;
                    eprintln!("--- types failed ---\n{}", rep.human());
                }
                types_result = json!({ "module": "types", "ok": rep.ok, "summary": rep.summary, "errors": rep.data.get("errors").cloned().unwrap_or(json!(0)), "diagnostics": rep.diagnostics, "elapsed_ms": started.elapsed().as_millis() });
            }
            Err(e) => {
                failed += 1;
                eprintln!("--- types could not run ---\n{e:#}");
                types_result = json!({ "module": "types", "ok": false, "error": format!("{e:#}"), "elapsed_ms": started.elapsed().as_millis() });
            }
        }
    }
    // Gameplay scenarios of every sample that has some, three seeds each.
    let mut scenario_results = vec![];
    if filter.map(|f| "scenarios".contains(f)).unwrap_or(true) {
        let samples = ws.root.join("samples");
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&samples).map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.join("project.toml").exists() && p.join("scenarios").is_dir()).collect()).unwrap_or_default();
        dirs.sort();
        for d in dirs {
            let name = d.file_name().unwrap().to_string_lossy().to_string();
            let started = Instant::now();
            let rep = scenario(ws, config, &name, None, 3, 900, None)?;
            if !rep.ok {
                failed += 1;
                eprintln!("--- scenarios of {name} failed ---\n{}", rep.summary);
                for r in rep.data.get("results").and_then(|r| r.as_array()).cloned().unwrap_or_default() {
                    for run in r.get("runs").and_then(|r| r.as_array()).cloned().unwrap_or_default() {
                        if !run.get("ok").and_then(|o| o.as_bool()).unwrap_or(false) {
                            eprintln!("  {} seed {}: {}", r.get("scenario").and_then(|s| s.as_str()).unwrap_or("?"), run.get("seed").and_then(|s| s.as_u64()).unwrap_or(0), run.get("error").map(|e| e.to_string()).unwrap_or_default());
                        }
                    }
                }
            }
            scenario_results.push(json!({ "module": format!("scenarios:{name}"), "ok": rep.ok, "summary": rep.summary, "elapsed_ms": started.elapsed().as_millis(), "results": rep.data.get("results").cloned().unwrap_or(json!([])) }));
        }
    }
    // Perception benchmarks of every sample that has some: the instruments must answer correctly.
    if filter.map(|f| "benches".contains(f)).unwrap_or(true) {
        let samples = ws.root.join("samples");
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&samples).map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.join("project.toml").exists() && p.join("benches").is_dir()).collect()).unwrap_or_default();
        dirs.sort();
        for d in dirs {
            let name = d.file_name().unwrap().to_string_lossy().to_string();
            let started = Instant::now();
            let rep = bench(ws, config, &name, None, 900, None)?;
            if !rep.ok {
                eprintln!("--- benchmarks of {name} failed ---\n{}", rep.summary);
            }
            scenario_results.push(json!({ "module": format!("benches:{name}"), "ok": rep.ok, "summary": rep.summary, "elapsed_ms": started.elapsed().as_millis(), "results": rep.data.get("benches").cloned().unwrap_or(json!([])) }));
        }
    }
    // The build tool's own unit tests (cargo test), when cargo is on PATH.
    let mut tool_result = json!(null);
    if filter.map(|f| "pocket_tool".contains(f)).unwrap_or(true) {
        if let Some(cargo) = toolchain::which("cargo") {
            let started = Instant::now();
            let out = toolchain::command(&cargo).args(["test", "--release", "--quiet", "--manifest-path"]).arg(ws.root.join("tools").join("pocket").join("Cargo.toml")).current_dir(&ws.root).output()?;
            let ok = out.status.success();
            if !ok {
                failed += 1;
                eprintln!("--- pocket_tool (cargo test) failed ---\n{}", tail(&format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)), 40));
            }
            tool_result = json!({ "module": "pocket_tool", "ok": ok, "exit_code": out.status.code(), "elapsed_ms": started.elapsed().as_millis() });
        }
    }
    // The Python environment client's tests (sdk/python, docs/design/environment.md), when
    // python3 is on PATH: they drive the built runtime over its JSON-RPC server.
    let mut python_result = json!(null);
    if filter.map(|f| "python".contains(f)).unwrap_or(true) {
        if let Some(python) = toolchain::which("python3") {
            let started = Instant::now();
            let out = toolchain::command(&python).arg(ws.root.join("sdk").join("python").join("test_pocket_env.py")).current_dir(&ws.root).env("POCKET_ROOT", &ws.root).env("POCKET_CONFIG", config).output()?;
            let ok = out.status.success();
            let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            if !ok {
                failed += 1;
                eprintln!("--- python (sdk/python/test_pocket_env.py) failed ---\n{}", tail(&text, 40));
            }
            python_result = json!({ "module": "python", "ok": ok, "exit_code": out.status.code(), "elapsed_ms": started.elapsed().as_millis(), "output_tail": tail(&text, 10) });
        }
    }
    let total = tests.len() + ts_results.len() + if tool_result.is_null() { 0 } else { 1 } + if python_result.is_null() { 0 } else { 1 } + if types_result.is_null() { 0 } else { 1 };
    let mut rep = if failed == 0 { Report::success("test", format!("{total} test modules passed")) } else { Report::failure("test", format!("{failed} of {total} test modules failed")) };
    rep.data = json!({ "config": config, "results": results, "scenarios": scenario_results, "ts": ts_results, "tool": tool_result, "python": python_result, "types": types_result, "bundles": bundles });
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

#[cfg(test)]
mod template_tests {
    use super::copy_template;
    use std::path::PathBuf;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pocket-template-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn copy_template_renames_and_skips_editor_state() {
        let dir = scratch("copy");
        let src = dir.join("sample");
        std::fs::create_dir_all(src.join("scripts")).unwrap();
        std::fs::create_dir_all(src.join(".pocket")).unwrap();
        std::fs::create_dir_all(src.join("assets").join("deep")).unwrap();
        std::fs::write(src.join("project.toml"), "name = \"sample\"\nentry = \"scripts/main.ts\"\n\n[window]\ntitle = \"Pocket: sample\"\nwidth = 960\n\n[input.actions]\nname = \"not the project name\"\n").unwrap();
        std::fs::write(src.join("scripts").join("main.ts"), "// A sample.\n").unwrap();
        std::fs::write(src.join(".pocket").join("editor.json"), "{}").unwrap();
        std::fs::write(src.join("assets").join("deep").join("tile.png"), b"png").unwrap();
        let dst = dir.join("mine");
        let files = copy_template(&src, &dst, "mine").unwrap();
        assert_eq!(files, 3);
        assert!(dst.join("scripts").join("main.ts").exists());
        assert!(dst.join("assets").join("deep").join("tile.png").exists());
        assert!(!dst.join(".pocket").exists());
        let toml = std::fs::read_to_string(dst.join("project.toml")).unwrap();
        assert!(toml.starts_with("name = \"mine\"\n"), "{toml}");
        assert!(toml.contains("title = \"mine\"\n"), "{toml}");
        assert!(toml.contains("name = \"not the project name\""), "{toml}");   // a key in a later section is left alone
        assert!(toml.contains("width = 960"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copy_template_adds_a_name_when_the_file_has_none() {
        let dir = scratch("noname");
        let src = dir.join("sample");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("project.toml"), "entry = \"scripts/main.ts\"\n").unwrap();
        let dst = dir.join("named");
        copy_template(&src, &dst, "named").unwrap();
        let toml = std::fs::read_to_string(dst.join("project.toml")).unwrap();
        assert_eq!(toml, "name = \"named\"\nentry = \"scripts/main.ts\"\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
