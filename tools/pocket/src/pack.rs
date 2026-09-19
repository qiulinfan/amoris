//! `pocket pack <project>`: a folder (or zip) that runs the project on another Mac without the
//! repository: the runtime executable, the bundled scripts, the project config, the project's
//! scene and assets, the UI font, and a launcher script. Scripts stay out; the bundle is what runs.
//!
//! `pocket pack <project> --web`: the same project as a static web folder (docs/web.md): the wasm
//! runtime, the project packaged into a virtual file system by Emscripten's file packager, and an
//! HTML shell that starts the runtime on a canvas and exposes `pocket.command` to the page.
use crate::commands::{build_targets, bundle_project, exe_path, find_project, ui_font};
use crate::manifest::Workspace;
use crate::report::{parse_compiler_diagnostics, Report};
use crate::toolchain;
use anyhow::{anyhow, bail, Context, Result};
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

/// `[web]` in project.toml: what the browser pack does to the font.
pub struct WebSettings {
    /// Keep only the glyphs the project can show (default true; the full CJK font is 16 MB).
    pub subset_font: bool,
    /// Characters to keep besides those found in the project's sources.
    pub font_text: String,
}

impl WebSettings {
    pub fn from_project(project: &Path) -> Result<WebSettings> {
        let mut out = WebSettings { subset_font: true, font_text: String::new() };
        let text = match std::fs::read_to_string(project.join("project.toml")) {
            Ok(t) => t,
            Err(_) => return Ok(out),
        };
        let pf: crate::ts::ProjectFile = toml::from_str(&text).context("parsing project.toml")?;
        if let Some(toml::Value::Table(web)) = pf.other.get("web") {
            if let Some(toml::Value::Boolean(b)) = web.get("subset_font") {
                out.subset_font = *b;
            }
            if let Some(toml::Value::String(s)) = web.get("font_text") {
                out.font_text = s.clone();
            }
        }
        Ok(out)
    }
}

/// The characters a packed font must cover: printable ASCII, general and CJK punctuation, fullwidth
/// forms, every character in the sources, scenes, settings and text files under `dirs` (the project,
/// and the editor when it ships too), and `extra`. Sorted and unique, so the same input always
/// yields the same subset.
pub fn font_text(dirs: &[&Path], extra: &str) -> String {
    let mut chars: std::collections::BTreeSet<char> = (0x20u32..0x7f).filter_map(char::from_u32).collect();
    for range in [0x00A0u32..0x0100, 0x2000..0x2070, 0x3000..0x3040, 0xFF00..0xFF66] {
        chars.extend(range.filter_map(char::from_u32));
    }
    fn walk(dir: &Path, chars: &mut std::collections::BTreeSet<char>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name == "node_modules" || name == "build" {
                continue;
            }
            if path.is_dir() {
                walk(&path, chars);
                continue;
            }
            let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if !matches!(ext.as_str(), "ts" | "tsx" | "js" | "json" | "toml" | "txt" | "md" | "csv") {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path) {
                chars.extend(text.chars().filter(|c| !c.is_control()));
            }
        }
    }
    for dir in dirs {
        walk(dir, &mut chars);
    }
    chars.extend(extra.chars().filter(|c| !c.is_control()));
    chars.into_iter().collect()
}

/// Write `font` reduced to the glyphs `text` needs (plus .notdef with an outline, so a missing
/// character shows as a box) with fontTools; shaping tables for the kept glyphs stay.
pub fn subset_font(font: &Path, text: &str, out: &Path) -> Result<()> {
    let chars = PathBuf::from(format!("{}.chars.txt", out.display()));
    std::fs::write(&chars, text)?;
    let output = toolchain::command("python3")
        .arg("-m").arg("fontTools.subset").arg(font)
        .arg(format!("--text-file={}", chars.display()))
        .arg(format!("--output-file={}", out.display()))
        .arg("--notdef-outline").arg("--no-hinting")
        .output().context("running python3 for fontTools")?;
    let _ = std::fs::remove_file(&chars);
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        if err.contains("No module named") {
            bail!("font subsetting needs fontTools: python3 -m pip install fonttools (or set [web] subset_font = false in project.toml)");
        }
        bail!("fontTools.subset failed: {}", err.trim());
    }
    Ok(())
}

pub struct Staged {
    pub bytes: u64,
    pub font: Value,
}

/// The project's runtime files (bundle, settings, data, font) laid out as the runtime expects them,
/// ready to be packaged or copied. With `web`, the font is subset to what the project (and the
/// editor, when `editor` is set) can show, and the editor's bundle is staged as editor.js.
fn stage_project(ws: &Workspace, project: &Path, staging: &Path, config: &str, web: Option<&WebSettings>, editor: bool) -> Result<Staged> {
    if staging.exists() {
        std::fs::remove_dir_all(staging).with_context(|| format!("clearing {}", staging.display()))?;
    }
    std::fs::create_dir_all(staging)?;
    let mut total = 0u64;
    let mut font_info = Value::Null;
    let bundle = bundle_project(ws, project, None)?;
    std::fs::copy(&bundle.out, staging.join("project.js"))?;
    total += std::fs::metadata(staging.join("project.js"))?.len();
    let editor_dir = ws.root.join("editor");
    if editor {
        let eb = bundle_project(ws, &editor_dir, None)?;
        std::fs::copy(&eb.out, staging.join("editor.js"))?;
        total += std::fs::metadata(staging.join("editor.js"))?.len();
    }
    let config_path = PathBuf::from(format!("{}.project.json", bundle.out.display()));
    let mut settings: Value = serde_json::from_str(&std::fs::read_to_string(&config_path)?)?;
    if let Value::Object(map) = &mut settings {
        map.remove("dir");
        map.remove("font");
        if let Some(font) = ui_font(ws) {
            let file_name = font.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "font.otf".into());
            std::fs::create_dir_all(staging.join("fonts"))?;
            let dst = staging.join("fonts").join(&file_name);
            let full = std::fs::metadata(&font)?.len();
            match web {
                Some(w) if w.subset_font => {
                    let mut dirs = vec![project];
                    if editor {
                        dirs.push(&editor_dir);
                    }
                    let text = font_text(&dirs, &w.font_text);
                    subset_font(&font, &text, &dst)?;
                    let bytes = std::fs::metadata(&dst)?.len();
                    font_info = json!({ "file": file_name, "subset": true, "chars": text.chars().count(), "bytes": bytes, "full_bytes": full });
                }
                _ => {
                    std::fs::copy(&font, &dst)?;
                    font_info = json!({ "file": file_name, "subset": false, "bytes": full });
                }
            }
            total += std::fs::metadata(&dst)?.len();
            map.insert("font".into(), Value::String(format!("fonts/{file_name}")));
        }
        map.insert("packed".into(), Value::String(chrono_free_timestamp()));
        map.insert("config".into(), Value::String(config.to_string()));
    }
    std::fs::write(staging.join("project.json"), serde_json::to_string_pretty(&settings)?)?;
    // The project's data: scene, assets, project.toml; not the TypeScript sources.
    total += copy_tree(project, &staging.join("project"), &["scripts", "node_modules", "build"])?;
    Ok(Staged { bytes: total, font: font_info })
}

const WEB_SHELL: &str = include_str!("web_shell.html");

/// The page for a project: the shell with its placeholders filled (the name, the configuration,
/// the extra runtime arguments and the editor flag that shows the page's download button).
fn render_shell(name: &str, config: &str, editor: bool) -> String {
    let title = serde_json::to_string(name).unwrap_or_else(|_| format!("\"{name}\""));
    let extra_args = if editor { r#", "--editor", "/game/editor.js", "--paused""# } else { "" };
    WEB_SHELL.replace("__POCKET_NAME__", name).replace("__POCKET_TITLE__", &title).replace("__POCKET_CONFIG__", config).replace("__POCKET_EXTRA_ARGS__", extra_args).replace("__POCKET_EDITOR__", if editor { "true" } else { "false" })
}

pub fn pack_web(ws: &Workspace, config: &str, target: &str, out: Option<&Path>, make_zip: bool, editor: bool) -> Result<Report> {
    let t0 = Instant::now();
    if ws.target_of(config)? != "wasm" {
        return Ok(Report::failure("pack", format!("--web needs a wasm configuration; '{config}' targets {}", ws.target_of(config)?)));
    }
    let project = find_project(ws, target).ok_or_else(|| anyhow!("'{target}' is not a project with project.toml"))?;
    let runtime = "pocket_runtime";
    let outcome = build_targets(ws, config, &[runtime.to_string()], false)?;
    if !outcome.ok {
        let mut rep = Report::failure("pack", "wasm runtime build failed");
        rep.diagnostics = parse_compiler_diagnostics(&outcome.output);
        return Ok(rep);
    }
    let name = project.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "game".into());
    let dist = out.map(|p| p.to_path_buf()).unwrap_or_else(|| ws.root.join("dist").join("web").join(&name));
    if dist.exists() {
        std::fs::remove_dir_all(&dist).with_context(|| format!("clearing {}", dist.display()))?;
    }
    std::fs::create_dir_all(&dist)?;
    let mut total = 0u64;
    // The runtime: the loader script and the wasm module.
    let js = exe_path(ws, config, runtime)?;
    let js = PathBuf::from(format!("{}.js", js.display()));
    let wasm = js.with_extension("wasm");
    for f in [&js, &wasm] {
        let dst = dist.join(f.file_name().unwrap());
        std::fs::copy(f, &dst).with_context(|| format!("copying {}", f.display()))?;
        total += std::fs::metadata(&dst)?.len();
    }
    // The project, packaged into one .data file mounted at /game.
    let staging = ws.build_dir(config).join("pack").join(&name);
    let web = WebSettings::from_project(&project)?;
    let staged = stage_project(ws, &project, &staging, config, Some(&web), editor)?;
    let sdk = toolchain::emsdk()?;
    let packager = sdk.emscripten.join("tools").join("file_packager.py");
    let data = dist.join(format!("{name}.data"));
    let data_js = dist.join(format!("{name}.data.js"));
    let mut cmd = toolchain::command("python3");
    toolchain::em_env(&mut cmd, &sdk);
    cmd.arg(&packager).arg(&data).arg("--preload").arg(format!("{}@/game", staging.display())).arg(format!("--js-output={}", data_js.display())).arg("--no-node");
    let out = cmd.output().context("running Emscripten's file_packager.py")?;
    if !out.status.success() {
        return Ok(Report::failure("pack", format!("file_packager failed: {}", String::from_utf8_lossy(&out.stderr))));
    }
    total += std::fs::metadata(&data)?.len() + std::fs::metadata(&data_js)?.len();
    // The page.
    std::fs::write(dist.join("index.html"), render_shell(&name, config, editor))?;
    std::fs::write(dist.join("README.txt"), format!("{name} for the web (packed by pocket, {config} configuration)\n\nServe this folder from any static web server and open index.html; file:// does not work because the\nbrowser must fetch the wasm module. For a quick look: python3 -m http.server --directory . 8080\n\nContents: index.html (the page), pocket_runtime.js + pocket_runtime.wasm (the engine), {name}.data + {name}.data.js\n(the project: scripts bundled, settings, scene, assets, UI font). The page exposes window.pocket for tests and agents\n(pocket.command(name, params) runs any runtime command, pocket.download(path) saves a file of the page's file system,\nsuch as the scene the editor saved or a save slot, to the visitor's downloads; docs/web.md in the repository has the details).\n"))?;
    let mut rep_data = json!({ "project": project, "dist": dist, "config": config, "bytes": total, "staged_bytes": staged.bytes, "font": staged.font, "editor": editor, "index": dist.join("index.html") });
    if make_zip {
        let zip_path = dist.with_extension("zip");
        let zbytes = zip_dir(&dist, &zip_path)?;
        rep_data["zip"] = json!(zip_path);
        rep_data["zip_bytes"] = json!(zbytes);
    }
    let mut rep = Report::success("pack", format!("packed {name} for the web into {} ({} MB)", dist.display(), total / (1024 * 1024)));
    rep.data = rep_data;
    rep.elapsed_ms = t0.elapsed().as_millis();
    Ok(rep)
}

// A UTC timestamp without pulling in a date crate: seconds since the epoch is enough for provenance.
fn chrono_free_timestamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("unix:{secs}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_shell_fills_every_placeholder() {
        for editor in [false, true] {
            let page = render_shell("my game", "wasm", editor);
            assert!(!page.contains("__POCKET_"), "a placeholder was left in the page");
            assert!(page.contains("<title>my game</title>"));
            assert!(page.contains("pocket_command_async"));
            assert_eq!(page.contains("\"--editor\""), editor);
            assert!(page.contains(if editor { "const editor = true;" } else { "const editor = false;" }));
        }
    }

    #[test]
    fn font_text_covers_ascii_punctuation_and_the_project_sources() {
        let dir = std::env::temp_dir().join(format!("pocket-font-text-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        std::fs::write(dir.join("scripts").join("main.tsx"), "const s = `分数 ${score}`; // café\n").unwrap();
        std::fs::write(dir.join("scene.json"), "{\"name\": \"Ünïcode\"}").unwrap();
        std::fs::write(dir.join("node_modules").join("x.ts"), "ignored: 忽").unwrap();
        std::fs::write(dir.join("notes.bin"), "skipped: 跳").unwrap();
        let text = font_text(&[dir.as_path()], "★");
        for c in ['A', 'z', '0', ' ', '~', '分', '数', 'é', 'Ü', '★', '。', '，', '\u{2014}', '\u{FF01}'] {
            assert!(text.contains(c), "missing {c:?}");
        }
        assert!(!text.contains('忽') && !text.contains('跳'), "node_modules and binaries must not contribute");
        assert!(!text.chars().any(|c| c.is_control()));
        let again = font_text(&[dir.as_path()], "★");
        assert_eq!(text, again, "deterministic");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
