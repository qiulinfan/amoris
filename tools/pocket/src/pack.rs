//! `pocket pack <project>`: a folder (or zip) that runs the project on another Mac without the
//! repository: the runtime executable, the bundled scripts, the project config, the project's
//! scene and assets, the UI font, and a launcher script. Scripts stay out; the bundle is what runs.
use crate::commands::{build_targets, bundle_project, exe_path, find_project, ui_font};
use crate::manifest::Workspace;
use crate::report::{parse_compiler_diagnostics, Report};
use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn copy_tree(from: &Path, to: &Path, skip: &[&str]) -> Result<u64> {
    let mut bytes = 0;
    std::fs::create_dir_all(to)?;
    let mut entries: Vec<_> = std::fs::read_dir(from)?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || skip.contains(&name.as_str()) {
            continue;
        }
        let src = entry.path();
        let dst = to.join(&name);
        if src.is_dir() {
            bytes += copy_tree(&src, &dst, skip)?;
        } else {
            std::fs::copy(&src, &dst).with_context(|| format!("copying {}", src.display()))?;
            bytes += std::fs::metadata(&dst)?.len();
        }
    }
    Ok(bytes)
}

fn set_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms)?;
    }
    Ok(())
}

fn zip_dir(dir: &Path, out: &Path) -> Result<u64> {
    let file = std::fs::File::create(out)?;
    let mut zip = zip::ZipWriter::new(file);
    let base = dir.parent().unwrap_or(dir);
    fn walk(zip: &mut zip::ZipWriter<std::fs::File>, base: &Path, dir: &Path) -> Result<()> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            let rel = path.strip_prefix(base)?.to_string_lossy().replace('\\', "/");
            if path.is_dir() {
                zip.add_directory(format!("{rel}/"), zip::write::SimpleFileOptions::default())?;
                walk(zip, base, &path)?;
            } else {
                #[cfg(unix)]
                let mode = { use std::os::unix::fs::PermissionsExt; std::fs::metadata(&path)?.permissions().mode() };
                #[cfg(not(unix))]
                let mode = 0o644;
                let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).unix_permissions(mode);
                zip.start_file(rel, options)?;
                let data = std::fs::read(&path)?;
                zip.write_all(&data)?;
            }
        }
        Ok(())
    }
    walk(&mut zip, base, dir)?;
    zip.finish()?;
    Ok(std::fs::metadata(out)?.len())
}

pub fn pack(ws: &Workspace, config: &str, target: &str, out: Option<&Path>, make_zip: bool) -> Result<Report> {
    let t0 = Instant::now();
    let project = find_project(ws, target).ok_or_else(|| anyhow!("'{target}' is not a project with project.toml"))?;
    let runtime = "pocket_runtime";
    let outcome = build_targets(ws, config, &[runtime.to_string()], false)?;
    if !outcome.ok {
        let mut rep = Report::failure("pack", "runtime build failed");
        rep.diagnostics = parse_compiler_diagnostics(&outcome.output);
        return Ok(rep);
    }
    let bundle = bundle_project(ws, &project, None)?;
    let name = project.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "game".into());
    let dist = out.map(|p| p.to_path_buf()).unwrap_or_else(|| ws.root.join("dist").join(&name));
    if dist.exists() {
        std::fs::remove_dir_all(&dist).with_context(|| format!("clearing {}", dist.display()))?;
    }
    std::fs::create_dir_all(dist.join("bin"))?;
    let mut total = 0u64;
    // Runtime.
    let exe = exe_path(ws, config, runtime)?;
    let exe_dst = dist.join("bin").join("pocket_runtime");
    std::fs::copy(&exe, &exe_dst)?;
    set_executable(&exe_dst)?;
    total += std::fs::metadata(&exe_dst)?.len();
    // Scripts (bundled) and config.
    std::fs::copy(&bundle.out, dist.join("project.js"))?;
    total += std::fs::metadata(dist.join("project.js"))?.len();
    let config_path = PathBuf::from(format!("{}.project.json", bundle.out.display()));
    let mut settings: Value = serde_json::from_str(&std::fs::read_to_string(&config_path)?)?;
    if let Value::Object(map) = &mut settings {
        map.remove("dir");
        map.remove("font");
        if let Some(font) = ui_font(ws) {
            let file_name = font.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "font.otf".into());
            std::fs::create_dir_all(dist.join("fonts"))?;
            std::fs::copy(&font, dist.join("fonts").join(&file_name))?;
            total += std::fs::metadata(dist.join("fonts").join(&file_name))?.len();
            map.insert("font".into(), Value::String(format!("fonts/{file_name}")));
        }
        map.insert("packed".into(), Value::String(chrono_free_timestamp()));
    }
    std::fs::write(dist.join("project.json"), serde_json::to_string_pretty(&settings)?)?;
    // The project's data: scene, assets, project.toml; not the TypeScript sources.
    total += copy_tree(&project, &dist.join("project"), &["scripts", "node_modules", "build"])?;
    // Launcher.
    let launcher = dist.join(&name);
    std::fs::write(&launcher, format!("#!/bin/sh\n# Runs {name}. Extra arguments go to the runtime (--headless, --frames N, --json, --size WxH, --serve PORT ...).\nDIR=\"$(cd \"$(dirname \"$0\")\" && pwd)\"\nexec \"$DIR/bin/pocket_runtime\" --project \"$DIR/project\" --bundle \"$DIR/project.js\" --project-config \"$DIR/project.json\" \"$@\"\n"))?;
    set_executable(&launcher)?;
    std::fs::write(dist.join("README.txt"), format!("{name} (packed by pocket, {config} configuration)\n\nRun ./{name} to open the game in a window.\n./{name} --headless --frames 600 --json runs it without a window and prints a report.\n./{name} --serve 4711 --paused opens the control server for agents (POST JSON-RPC to /rpc).\n\nContents: bin/pocket_runtime (the engine), project.js (the game's scripts, bundled), project.json (settings), project/ (scene and assets), fonts/ (UI font).\n"))?;
    let mut data = json!({ "project": project, "dist": dist, "config": config, "bytes": total, "launcher": launcher });
    if make_zip {
        let zip_path = dist.with_extension("zip");
        let zbytes = zip_dir(&dist, &zip_path)?;
        data["zip"] = json!(zip_path);
        data["zip_bytes"] = json!(zbytes);
    }
    let mut rep = Report::success("pack", format!("packed {name} into {} ({} MB)", dist.display(), total / (1024 * 1024)));
    rep.data = data;
    rep.elapsed_ms = t0.elapsed().as_millis();
    Ok(rep)
}

// A UTC timestamp without pulling in a date crate: seconds since the epoch is enough for provenance.
fn chrono_free_timestamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("unix:{secs}")
}
