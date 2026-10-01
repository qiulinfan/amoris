//! `pocket run --watch` / `pocket editor --watch`: keep the runtime open, rebundle the project's
//! TypeScript when a source file changes, and hot reload it through the control server. This is
//! the loop an agent (or a person) iterates in: edit a script, see the result, no restart.
use crate::commands::{build_targets, bundle_project, exe_path, find_project};
use crate::manifest::Workspace;
use crate::report::{parse_compiler_diagnostics, Report};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

fn free_port() -> Result<u16> {
    let l = TcpListener::bind("127.0.0.1:0")?;
    Ok(l.local_addr()?.port())
}

/// Newest modification time under `dir` for the file kinds the bundler reads.
fn newest_source(dir: &Path) -> SystemTime {
    let mut newest = SystemTime::UNIX_EPOCH;
    fn visit(dir: &Path, newest: &mut SystemTime, depth: usize) {
        if depth > 8 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for entry in rd.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name == "node_modules" || name == "build" {
                continue;
            }
            if path.is_dir() {
                visit(&path, newest, depth + 1);
                continue;
            }
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if matches!(ext, "ts" | "tsx" | "json" | "toml") {
                if let Ok(m) = entry.metadata().and_then(|m| m.modified()) {
                    if m > *newest {
                        *newest = m;
                    }
                }
            }
        }
    }
    visit(dir, &mut newest, 0);
    newest
}

/// The newest file under the project's assets/ (pictures, maps, sounds, meshes): a change there
/// needs no bundle, only the runtime forgetting what it decoded.
fn newest_asset(project: &Path) -> SystemTime {
    let mut newest = SystemTime::UNIX_EPOCH;
    fn visit(dir: &Path, newest: &mut SystemTime, depth: usize) {
        if depth > 8 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for entry in rd.flatten() {
            let path = entry.path();
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if path.is_dir() {
                visit(&path, newest, depth + 1);
                continue;
            }
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            // Pictures, maps, sounds and models, the ones Blender converts too (.blend saved from Blender
            // reaches the running game: docs/design/assets.md, Live models).
            if matches!(
                ext.as_str(),
                "png" | "jpg" | "jpeg" | "bmp" | "tga" | "tmj" | "wav" | "ogg" | "mp3" | "glb" | "gltf" | "bin" | "obj" | "stl" | "ply" | "vox" | "voxels" | "blend" | "fbx" | "dae" | "usd" | "usda" | "usdc" | "usdz" | "abc" | "3ds" | "x3d" | "wrl"
            ) {
                if let Ok(m) = entry.metadata().and_then(|m| m.modified()) {
                    if m > *newest {
                        *newest = m;
                    }
                }
            }
        }
    }
    visit(&project.join("assets"), &mut newest, 0);
    newest
}

/// Type-check the project and hand the errors to the runtime (the editor's Script tab and the
/// Console show them; agents read `script.diagnostics`), with file names relative to the project.
fn send_types(ws: &Workspace, url: &str, project: &Path) {
    let rep = match crate::check::check(ws, Some(project)) {
        Ok(rep) => rep,
        Err(e) => {
            eprintln!("[watch] types not checked: {e:#}");
            return;
        }
    };
    let root = std::fs::canonicalize(&ws.root).unwrap_or(ws.root.clone());
    let dir = std::fs::canonicalize(project).unwrap_or(project.to_path_buf());
    let prefix = dir.strip_prefix(&root).map(|p| format!("{}/", p.display())).unwrap_or_else(|_| format!("{}/", dir.display()));
    let diagnostics: Vec<Value> = rep.diagnostics.iter().map(|d| {
        let mut v = serde_json::to_value(d).unwrap_or(Value::Null);
        if let Some(f) = d.file.as_deref().and_then(|f| f.strip_prefix(prefix.as_str())) {
            v["file"] = json!(f);
        }
        v
    }).collect();
    eprintln!("[watch] types: {}", rep.summary);
    if let Err(e) = rpc(url, "script.diagnostics", json!({ "diagnostics": diagnostics })) {
        eprintln!("[watch] type errors not delivered: {e:#}");
    }
}

fn rpc(url: &str, method: &str, params: Value) -> Result<Value> {
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }).to_string();
    let resp = crate::mcp::http_post(url, "/rpc", &body)?;
    let v: Value = serde_json::from_str(&resp).context("runtime returned invalid JSON")?;
    if let Some(err) = v.get("error") {
        bail!("{}", err.get("message").and_then(|m| m.as_str()).unwrap_or("runtime error"));
    }
    Ok(v.get("result").cloned().unwrap_or(Value::Null))
}

pub fn watch(ws: &Workspace, config: &str, target: &str, args: &[String], editor: bool) -> Result<Report> {
    let t0 = Instant::now();
    let project = find_project(ws, target).ok_or_else(|| anyhow!("'{target}' is not a project with project.toml"))?;
    let runtime = "pocket_runtime";
    let outcome = build_targets(ws, config, &[runtime.to_string()], false)?;
    if !outcome.ok {
        let mut rep = Report::failure("watch", "runtime build failed");
        rep.diagnostics = parse_compiler_diagnostics(&outcome.output);
        return Ok(rep);
    }
    let bundle = bundle_project(ws, &project, None)?;
    let editor_bundle: Option<PathBuf> = if editor {
        let dir = ws.root.join("editor");
        if !dir.join("project.toml").exists() {
            bail!("editor/project.toml is missing");
        }
        Some(bundle_project(ws, &dir, None)?.out)
    } else {
        None
    };
    let port = free_port()?;
    let exe = exe_path(ws, config, runtime)?;
    let mut cmd = crate::commands::runtime_command(ws, &exe);
    cmd.arg("--project").arg(&project).arg("--bundle").arg(&bundle.out).arg("--serve").arg(port.to_string());
    if let Some(eb) = &editor_bundle {
        cmd.arg("--editor").arg(eb).arg("--paused").arg("--title").arg(format!("Pocket Editor - {target}"));
    }
    cmd.args(args).current_dir(&ws.root);
    let mut child = cmd.spawn().context("spawning pocket_runtime")?;
    let url = format!("http://127.0.0.1:{port}");
    // Wait for the control server.
    let started = Instant::now();
    loop {
        if rpc(&url, "state", json!({})).is_ok() {
            break;
        }
        if let Some(status) = child.try_wait()? {
            bail!("runtime exited before serving ({status})");
        }
        if started.elapsed() > Duration::from_secs(20) {
            let _ = child.kill();
            bail!("runtime did not start serving within 20s");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    eprintln!("[watch] {target} running at {url}; editing {} reloads it", project.display());
    send_types(ws, &url, &project);
    let sdk_dir = ws.root.join("sdk").join("runtime");
    let editor_dir = ws.root.join("editor");
    let mut last = newest_source(&project).max(newest_source(&sdk_dir)).max(if editor { newest_source(&editor_dir) } else { SystemTime::UNIX_EPOCH });
    let mut last_asset = newest_asset(&project);
    let mut reloads = 0u32;
    let mut failures = 0u32;
    let exit_code = loop {
        if let Some(status) = child.try_wait()? {
            break status.code();
        }
        std::thread::sleep(Duration::from_millis(250));
        // A changed asset: the runtime forgets its decoded copy and draws the file as it is now,
        // without a bundle or a reload of the scripts.
        let now_asset = newest_asset(&project);
        if now_asset > last_asset {
            last_asset = now_asset;
            match rpc(&url, "assets.reload", json!({})) {
                Ok(_) => {
                    reloads += 1;
                    eprintln!("[watch] assets reloaded");
                }
                Err(e) => {
                    failures += 1;
                    eprintln!("[watch] assets reload failed: {e:#}");
                }
            }
        }
        let now = newest_source(&project).max(newest_source(&sdk_dir)).max(if editor { newest_source(&editor_dir) } else { SystemTime::UNIX_EPOCH });
        if now <= last {
            continue;
        }
        // Debounce: editors write files in bursts.
        std::thread::sleep(Duration::from_millis(150));
        last = newest_source(&project).max(newest_source(&sdk_dir)).max(if editor { newest_source(&editor_dir) } else { SystemTime::UNIX_EPOCH });
        let t = Instant::now();
        match bundle_project(ws, &project, None) {
            Ok(b) => {
                let editor_ok = if editor {
                    match bundle_project(ws, &editor_dir, None) {
                        Ok(_) => true,
                        Err(e) => {
                            failures += 1;
                            eprintln!("[watch] editor bundle failed: {e:#}");
                            false
                        }
                    }
                } else {
                    true
                };
                if editor && editor_ok {
                    match rpc(&url, "script.reload", json!({ "name": "editor" })) {
                        Ok(_) => eprintln!("[watch] editor reloaded"),
                        Err(e) => eprintln!("[watch] editor reload failed: {e:#}"),
                    }
                }
                match rpc(&url, "project.reload", json!({ "scene": !editor, "scripts": true })) {
                    Ok(v) => {
                        reloads += 1;
                        eprintln!("[watch] reloaded {} modules in {} ms (started: {})", b.modules.len(), t.elapsed().as_millis(), v.get("started").and_then(|s| s.as_bool()).unwrap_or(false));
                    }
                    Err(e) => {
                        failures += 1;
                        eprintln!("[watch] reload failed: {e:#}");
                    }
                }
            }
            Err(e) => {
                failures += 1;
                eprintln!("[watch] bundle failed, keeping the previous scripts:\n{e:#}");
            }
        }
        send_types(ws, &url, &project);
    };
    let mut rep = if exit_code == Some(0) { Report::success("watch", format!("{target} exited 0 after {reloads} reloads")) } else { Report::failure("watch", format!("{target} exited {}", exit_code.unwrap_or(-1))) };
    rep.data = json!({ "project": project, "reloads": reloads, "failures": failures, "exit_code": exit_code });
    rep.elapsed_ms = t0.elapsed().as_millis();
    Ok(rep)
}
