//! `pocket pack <project>`: a folder (or zip) that runs the project on another machine of the same
//! system without the repository: the runtime executable, the bundled scripts, the project config,
//! the project's scene and assets, the UI font, and a launcher script. Scripts stay out; the bundle
//! is what runs.
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

/// Copies a directory without its dot-entries (`.pocket`'s session state, `.git`), but for
/// `.imported/`: the store's Blender conversions and parsed OBJ meshes, which a packed game reads
/// instead of a Blender it does not have (docs/design/assets.md, Formats).
fn copy_tree(from: &Path, to: &Path, skip: &[&str]) -> Result<u64> {
    let mut bytes = 0;
    std::fs::create_dir_all(to)?;
    let mut entries: Vec<_> = std::fs::read_dir(from)?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if (name.starts_with('.') && name != ".imported") || skip.contains(&name.as_str()) {
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
    #[cfg(not(unix))]
    let _ = path;
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
    let windows = cfg!(windows);
    let exe_dst = dist.join("bin").join(exe.file_name().unwrap_or_default());
    std::fs::copy(&exe, &exe_dst)?;
    set_executable(&exe_dst)?;
    total += std::fs::metadata(&exe_dst)?.len();
    // Windows: the DLLs the build put beside the runtime (JavaScriptCore, the shader compiler,
    // AddressSanitizer's in a debug pack), not its debug database.
    let mut dlls: Vec<String> = vec![];
    if windows {
        if let Some(bin) = exe.parent() {
            for e in std::fs::read_dir(bin)?.filter_map(|e| e.ok()) {
                let p = e.path();
                if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dll")) {
                    let to = dist.join("bin").join(e.file_name());
                    std::fs::copy(&p, &to)?;
                    total += std::fs::metadata(&to)?.len();
                    dlls.push(e.file_name().to_string_lossy().into_owned());
                }
            }
        }
        dlls.sort();
    }
    total += write_game_data(ws, &project, &bundle.out, &dist)?;
    // Launcher.
    let launcher = if windows {
        let l = dist.join(format!("{name}.cmd"));
        std::fs::write(&l, format!("@rem Runs {name}. Extra arguments go to the runtime (--headless, --frames N, --json, --size WxH, --serve PORT ...).\r\n@\"%~dp0bin\\pocket_runtime.exe\" --project \"%~dp0project\" --bundle \"%~dp0project.js\" --project-config \"%~dp0project.json\" %*\r\n"))?;
        l
    } else {
        let l = dist.join(&name);
        std::fs::write(&l, format!("#!/bin/sh\n# Runs {name}. Extra arguments go to the runtime (--headless, --frames N, --json, --size WxH, --serve PORT ...).\nDIR=\"$(cd \"$(dirname \"$0\")\" && pwd)\"\nexec \"$DIR/bin/pocket_runtime\" --project \"$DIR/project\" --bundle \"$DIR/project.js\" --project-config \"$DIR/project.json\" \"$@\"\n"))?;
        set_executable(&l)?;
        l
    };
    let run = if windows { format!("{name}.cmd") } else { format!("./{name}") };
    let engine = if windows {
        format!("bin/pocket_runtime.exe (the engine) with {} beside it (pocket_jsc.dll is JavaScriptCore, LGPL-2.1+, replaceable; dxcompiler.dll is Microsoft's DirectX Shader Compiler; the game also needs the Visual C++ runtime, vcruntime140.dll and msvcp140.dll, which Windows usually has)", dlls.join(", "))
    } else {
        "bin/pocket_runtime (the engine)".to_string()
    };
    std::fs::write(dist.join("README.txt"), format!("{name} (packed by pocket, {config} configuration)\n\nRun {run} to open the game in a window.\n{run} --headless --frames 600 --json runs it without a window and prints a report.\n{run} --serve 4711 --paused opens the control server for agents (POST JSON-RPC to /rpc).\n\nContents: {engine}, project.js (the game's scripts, bundled), project.json (settings), project/ (scene and assets), fonts/ (UI font).\n"))?;
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

/// What a packed game reads beside the runtime, written into `dist`: the bundled scripts
/// (project.js), the settings with the UI fonts made relative (project.json, fonts/), and the
/// project's data (project/: the scene, the assets, project.toml; not the TypeScript sources).
fn write_game_data(ws: &Workspace, project: &Path, bundle: &Path, dist: &Path) -> Result<u64> {
    let mut total = 0u64;
    std::fs::copy(bundle, dist.join("project.js"))?;
    total += std::fs::metadata(dist.join("project.js"))?.len();
    let config_path = PathBuf::from(format!("{}.project.json", bundle.display()));
    let mut settings: Value = serde_json::from_str(&std::fs::read_to_string(&config_path)?)?;
    let settings_fallbacks = settings.get("font_fallbacks").cloned().unwrap_or(Value::Null);
    if let Value::Object(map) = &mut settings {
        map.remove("dir");
        map.remove("font");
        map.remove("font_fallbacks");
        if let Some(font) = ui_font(ws) {
            let file_name = font.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "font.otf".into());
            std::fs::create_dir_all(dist.join("fonts"))?;
            std::fs::copy(&font, dist.join("fonts").join(&file_name))?;
            total += std::fs::metadata(dist.join("fonts").join(&file_name))?.len();
            map.insert("font".into(), Value::String(format!("fonts/{file_name}")));
            let mut packed_fallbacks = vec![];
            for (fb, name) in fallback_paths(project, &settings_fallbacks, &file_name)? {
                std::fs::copy(&fb, dist.join("fonts").join(&name))?;
                total += std::fs::metadata(dist.join("fonts").join(&name))?.len();
                packed_fallbacks.push(Value::String(format!("fonts/{name}")));
            }
            map.insert("font_fallbacks".into(), Value::Array(packed_fallbacks));
        }
        map.insert("packed".into(), Value::String(chrono_free_timestamp()));
    }
    std::fs::write(dist.join("project.json"), serde_json::to_string_pretty(&settings)?)?;
    // The project's data: scene, assets, project.toml; not the TypeScript sources.
    total += copy_tree(project, &dist.join("project"), &["scripts", "node_modules", "build"])?;
    Ok(total)
}

/// The fallback font files a project config names, each with the name it is packed under in
/// fonts/. `pocket` writes the bundled ones as absolute paths; a project.toml's own are relative to
/// the project, as the runtime reads them. A name already taken (by the main font or another
/// fallback) gets a numbered prefix.
fn fallback_paths(project: &Path, v: &Value, main_font: &str) -> Result<Vec<(PathBuf, String)>> {
    let mut taken = std::collections::HashSet::from([main_font.to_string()]);
    let mut out = vec![];
    for (i, entry) in v.as_array().into_iter().flatten().filter_map(|x| x.as_str()).enumerate() {
        let path = PathBuf::from(entry);
        let path = if path.is_relative() { project.join(path) } else { path };
        if !path.is_file() {
            bail!("font_fallbacks: {} does not exist", path.display());
        }
        let base = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "fallback.ttf".into());
        let name = if taken.contains(&base) { format!("fallback-{i}-{base}") } else { base };
        taken.insert(name.clone());
        out.push((path, name));
    }
    Ok(out)
}

/// `[web]` in project.toml: what the browser pack does to the font.
pub struct WebSettings {
    /// Keep only the glyphs the project can show (default true; the full CJK font is 16 MB).
    pub subset_font: bool,
    /// Characters to keep besides those found in the project's sources.
    pub font_text: String,
    /// The installed game's name ([window] title, else the project's name).
    pub title: String,
    /// A PNG of the project's for the installed game's icon; a plain one is drawn without it.
    pub icon: Option<String>,
    /// How the installed game opens: fullscreen (default), standalone, minimal-ui or browser.
    pub display: String,
    /// landscape, portrait or any; by default from the [window] size.
    pub orientation: String,
    /// The splash screen's colour and the page's.
    pub background: String,
    /// The browser's bar around the game.
    pub theme: String,
    /// A service worker that keeps the game's files, so it starts without a network (default true).
    pub offline: bool,
}

impl WebSettings {
    pub fn from_project(project: &Path) -> Result<WebSettings> {
        let name = project.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "game".into());
        let mut out = WebSettings {
            subset_font: true,
            font_text: String::new(),
            title: name,
            icon: None,
            display: "fullscreen".into(),
            orientation: "any".into(),
            background: "#101014".into(),
            theme: "#101014".into(),
            offline: true,
        };
        let text = match std::fs::read_to_string(project.join("project.toml")) {
            Ok(t) => t,
            Err(_) => return Ok(out),
        };
        let pf: crate::ts::ProjectFile = toml::from_str(&text).context("parsing project.toml")?;
        if !pf.name.is_empty() {
            out.title = pf.name.clone();
        }
        if let Some(toml::Value::Table(window)) = &pf.window {
            if let Some(toml::Value::String(s)) = window.get("title") {
                out.title = s.clone();
            }
            let size = |k: &str| window.get(k).and_then(|v| v.as_integer());
            if let (Some(w), Some(h)) = (size("width"), size("height")) {
                out.orientation = if w > h { "landscape" } else if h > w { "portrait" } else { "any" }.into();
            }
        }
        if let Some(toml::Value::Table(web)) = pf.other.get("web") {
            if let Some(toml::Value::Boolean(b)) = web.get("subset_font") {
                out.subset_font = *b;
            }
            if let Some(toml::Value::String(s)) = web.get("font_text") {
                out.font_text = s.clone();
            }
            let text = |k: &str| web.get(k).and_then(|v| v.as_str()).map(str::to_string);
            if let Some(s) = text("title") {
                out.title = s;
            }
            out.icon = text("icon");
            for (key, slot) in [("display", &mut out.display), ("orientation", &mut out.orientation), ("background", &mut out.background), ("theme", &mut out.theme)] {
                if let Some(s) = text(key) {
                    *slot = s;
                }
            }
            if let Some(toml::Value::Boolean(b)) = web.get("offline") {
                out.offline = *b;
            }
        }
        if !["fullscreen", "standalone", "minimal-ui", "browser"].contains(&out.display.as_str()) {
            return Err(anyhow!("[web] display = \"{}\": fullscreen, standalone, minimal-ui or browser", out.display));
        }
        Ok(out)
    }
}

/// A PNG of RGBA rows, deflated (the manifest's icons when the project gives none).
fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut raw = Vec::with_capacity((width as usize * 4 + 1) * height as usize);
    for row in rgba.chunks(width as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    z.write_all(&raw).expect("deflating to memory");
    let idat = z.finish().expect("deflating to memory");
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |kind: &[u8], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut crc = flate2::Crc::new();
        crc.update(kind);
        crc.update(data);
        out.extend_from_slice(&crc.sum().to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &idat);
    chunk(b"IEND", &[]);
    out
}

/// A plain icon: a rounded square of the theme colour with a ring and a dot, edges smoothed.
fn default_icon(size: u32, color: &str) -> Vec<u8> {
    let hex = color.trim_start_matches('#');
    let c = |i: usize| u8::from_str_radix(hex.get(i..i + 2).unwrap_or("40"), 16).unwrap_or(0x40) as f32;
    let (r, g, b) = if hex.len() >= 6 { (c(0), c(2), c(4)) } else { (0x2b as f32, 0x6c as f32, 0xb0 as f32) };
    // A dark theme makes a dark icon: lift it so the icon shows on a home screen.
    let lift = if r + g + b < 150.0 { 1.0 } else { 0.0 };
    let base = [r + (0x3a as f32 - r) * lift, g + (0x7b as f32 - g) * lift, b + (0xd5 as f32 - b) * lift];
    let s = size as f32;
    let mut px = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // Coverage of the rounded square (corner radius a fifth of the side).
            let rad = s * 0.2;
            let qx = (fx - s / 2.0).abs() - (s / 2.0 - rad);
            let qy = (fy - s / 2.0).abs() - (s / 2.0 - rad);
            let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - rad;
            let square = (0.5 - outside).clamp(0.0, 1.0);
            let d = ((fx - s / 2.0).powi(2) + (fy - s / 2.0).powi(2)).sqrt();
            let ring = (0.5 - ((d - s * 0.27).abs() - s * 0.055)).clamp(0.0, 1.0);
            let dot = (0.5 - (d - s * 0.09)).clamp(0.0, 1.0);
            let white = ring.max(dot);
            let i = ((y * size + x) * 4) as usize;
            for k in 0..3 {
                px[i + k] = (base[k] + (255.0 - base[k]) * white * 0.92).round().clamp(0.0, 255.0) as u8;
            }
            px[i + 3] = (square * 255.0).round() as u8;
        }
    }
    encode_png(size, size, &px)
}

/// The width and height a PNG's header gives.
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return None;
    }
    Some((u32::from_be_bytes(bytes[16..20].try_into().ok()?), u32::from_be_bytes(bytes[20..24].try_into().ok()?)))
}

/// What makes a pack an installable app (docs/web.md, Installed and offline): the manifest, its
/// icons and, unless [web] offline = false, a service worker that keeps every file of the pack in a
/// cache named for their contents. Returns the files written and the lines for the page's head.
fn write_app_files(dist: &Path, project: &Path, name: &str, web: &WebSettings, files: &[String], page: &str) -> Result<(Vec<String>, String)> {
    use sha2::Digest;
    let mut written = Vec::new();
    let mut icons = Vec::new();
    if let Some(icon) = &web.icon {
        let bytes = std::fs::read(project.join(icon)).with_context(|| format!("[web] icon = \"{icon}\": reading it"))?;
        let (w, h) = png_size(&bytes).ok_or_else(|| anyhow!("[web] icon = \"{icon}\" is not a PNG"))?;
        std::fs::write(dist.join("icon.png"), &bytes)?;
        icons.push(json!({ "src": "icon.png", "sizes": format!("{w}x{h}"), "type": "image/png", "purpose": "any" }));
        written.push("icon.png".to_string());
    } else {
        for size in [192u32, 512] {
            let file = format!("icon-{size}.png");
            std::fs::write(dist.join(&file), default_icon(size, &web.theme))?;
            icons.push(json!({ "src": file, "sizes": format!("{size}x{size}"), "type": "image/png", "purpose": "any" }));
            written.push(file);
        }
    }
    let manifest = json!({
        "name": web.title,
        "short_name": if web.title.chars().count() <= 12 { web.title.clone() } else { name.to_string() },
        "start_url": "./",
        "scope": "./",
        "display": web.display,
        "orientation": web.orientation,
        "background_color": web.background,
        "theme_color": web.theme,
        "icons": icons,
    });
    std::fs::write(dist.join("manifest.webmanifest"), serde_json::to_string_pretty(&manifest)?)?;
    written.push("manifest.webmanifest".to_string());
    let touch_icon = if web.icon.is_some() { "icon.png" } else { "icon-192.png" };
    let mut head = format!(
        "<link rel=\"manifest\" href=\"manifest.webmanifest\">\n<meta name=\"theme-color\" content=\"{}\">\n<link rel=\"icon\" href=\"{touch_icon}\">\n<link rel=\"apple-touch-icon\" href=\"{touch_icon}\">\n<meta name=\"mobile-web-app-capable\" content=\"yes\">\n<meta name=\"apple-mobile-web-app-capable\" content=\"yes\">",
        web.theme
    );
    if web.offline {
        // The cache is named for the pack's contents, so a new pack replaces it on the next visit.
        let mut all: Vec<String> = files.iter().cloned().chain(written.iter().cloned()).collect();
        all.sort();
        let mut hash = sha2::Sha256::new();
        hash.update(page.as_bytes());
        for f in &all {
            hash.update(f.as_bytes());
            hash.update(std::fs::read(dist.join(f))?);
        }
        let version: String = hash.finalize().iter().take(6).map(|b| format!("{b:02x}")).collect();
        let mut cached = vec!["./".to_string(), "index.html".to_string()];
        cached.extend(all);
        let sw = SERVICE_WORKER.replace("__POCKET_VERSION__", &version).replace("__POCKET_FILES__", &serde_json::to_string(&cached)?);
        std::fs::write(dist.join("sw.js"), sw)?;
        written.push("sw.js".to_string());
        head.push_str("\n<script>if (\"serviceWorker\" in navigator && location.protocol.startsWith(\"http\")) navigator.serviceWorker.register(\"sw.js\").catch((e) => console.warn(\"service worker:\", e));</script>");
    }
    Ok((written, head))
}

const SERVICE_WORKER: &str = r#"// The service worker of a packed Pocket game (docs/web.md, Installed and offline): every file of the
// pack is kept in a cache named for their contents, so the game starts without a network once it has
// been opened; a new pack has another name, and its worker replaces the old cache on the next visit.
const CACHE = "pocket:" + self.registration.scope + ":__POCKET_VERSION__";
const FILES = __POCKET_FILES__;
self.addEventListener("install", (e) => {
  e.waitUntil(caches.open(CACHE).then((c) => c.addAll(FILES)).then(() => self.skipWaiting()));
});
self.addEventListener("activate", (e) => {
  const mine = "pocket:" + self.registration.scope + ":";
  e.waitUntil(caches.keys()
    .then((keys) => Promise.all(keys.filter((k) => k.startsWith(mine) && k !== CACHE).map((k) => caches.delete(k))))
    .then(() => self.clients.claim()));
});
self.addEventListener("fetch", (e) => {
  if (e.request.method !== "GET") return;
  e.respondWith(caches.open(CACHE)
    .then((c) => c.match(e.request, { ignoreSearch: true }))
    .then((hit) => hit || fetch(e.request)));
});
"#;

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
    let python = toolchain::python().context("Python 3 not found (font subsetting runs fontTools)")?;
    let output = toolchain::command(&python)
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
    let settings_fallbacks = settings.get("font_fallbacks").cloned().unwrap_or(Value::Null);
    if let Value::Object(map) = &mut settings {
        map.remove("dir");
        map.remove("font");
        map.remove("font_fallbacks");
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
            // The fallback fonts beside it, cut to the same characters.
            let mut packed_fallbacks = vec![];
            for (fb, name) in fallback_paths(project, &settings_fallbacks, &file_name)? {
                let dst = staging.join("fonts").join(&name);
                match web {
                    Some(w) if w.subset_font => {
                        let mut dirs = vec![project];
                        if editor {
                            dirs.push(&editor_dir);
                        }
                        subset_font(&fb, &font_text(&dirs, &w.font_text), &dst)?;
                    }
                    _ => {
                        std::fs::copy(&fb, &dst)?;
                    }
                }
                total += std::fs::metadata(&dst)?.len();
                packed_fallbacks.push(Value::String(format!("fonts/{name}")));
            }
            map.insert("font_fallbacks".into(), Value::Array(packed_fallbacks));
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
fn render_shell(name: &str, config: &str, editor: bool, head: &str, background: &str) -> String {
    let title = serde_json::to_string(name).unwrap_or_else(|_| format!("\"{name}\""));
    let extra_args = if editor { r#", "--editor", "/game/editor.js", "--paused""# } else { "" };
    WEB_SHELL
        .replace("__POCKET_NAME__", name)
        .replace("__POCKET_TITLE__", &title)
        .replace("__POCKET_CONFIG__", config)
        .replace("__POCKET_EXTRA_ARGS__", extra_args)
        .replace("__POCKET_EDITOR__", if editor { "true" } else { "false" })
        .replace("__POCKET_BACKGROUND__", background)
        .replace("__POCKET_HEAD__", head)
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
    let mut cmd = toolchain::command(&toolchain::python().context("Python 3 not found (the web pack runs Emscripten's file_packager)")?);
    toolchain::em_env(&mut cmd, &sdk);
    cmd.arg(&packager).arg(&data).arg("--preload").arg(format!("{}@/game", staging.display())).arg(format!("--js-output={}", data_js.display())).arg("--no-node");
    let out = cmd.output().context("running Emscripten's file_packager.py")?;
    if !out.status.success() {
        return Ok(Report::failure("pack", format!("file_packager failed: {}", String::from_utf8_lossy(&out.stderr))));
    }
    total += std::fs::metadata(&data)?.len() + std::fs::metadata(&data_js)?.len();
    // The page, and what makes it an installable app: a manifest, icons and the offline worker.
    let packed: Vec<String> = vec!["pocket_runtime.js".into(), "pocket_runtime.wasm".into(), format!("{name}.data"), format!("{name}.data.js")];
    let (app_files, head) = write_app_files(&dist, &project, &name, &web, &packed, &render_shell(&name, config, editor, "", &web.background))?;
    std::fs::write(dist.join("index.html"), render_shell(&name, config, editor, &head, &web.background))?;
    for f in &app_files {
        total += std::fs::metadata(dist.join(f))?.len();
    }
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
            let page = render_shell("my game", "wasm", editor, "<link rel=\"manifest\" href=\"manifest.webmanifest\">", "#223344");
            assert!(!page.contains("__POCKET_"), "a placeholder was left in the page");
            assert!(page.contains("<link rel=\"manifest\"") && page.contains("background: #223344"));
            assert!(page.contains("<title>my game</title>"));
            assert!(page.contains("pocket_command_async"));
            assert_eq!(page.contains("\"--editor\""), editor);
            assert!(page.contains(if editor { "const editor = true;" } else { "const editor = false;" }));
        }
    }

    #[test]
    fn app_files_make_a_manifest_icons_and_a_worker_named_for_the_contents() {
        let dir = std::env::temp_dir().join(format!("pocket-app-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("game.data"), b"one").unwrap();
        let web = WebSettings { subset_font: true, font_text: String::new(), title: "A Long Game Title".into(), icon: None, display: "fullscreen".into(), orientation: "landscape".into(), background: "#000000".into(), theme: "#102030".into(), offline: true };
        let files = vec!["game.data".to_string()];
        let (written, head) = write_app_files(&dir, &dir, "game", &web, &files, "page").unwrap();
        assert_eq!(written, ["icon-192.png", "icon-512.png", "manifest.webmanifest", "sw.js"]);
        assert_eq!(png_size(&std::fs::read(dir.join("icon-512.png")).unwrap()), Some((512, 512)));
        let manifest: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("manifest.webmanifest")).unwrap()).unwrap();
        assert_eq!(manifest["short_name"], "game");
        assert_eq!(manifest["orientation"], "landscape");
        assert_eq!(manifest["icons"].as_array().unwrap().len(), 2);
        assert!(head.contains("serviceWorker") && head.contains("theme-color"));
        let sw = std::fs::read_to_string(dir.join("sw.js")).unwrap();
        assert!(sw.contains("\"game.data\"") && sw.contains("\"index.html\"") && !sw.contains("__POCKET_"));
        // Other contents, another cache.
        std::fs::write(dir.join("game.data"), b"two").unwrap();
        write_app_files(&dir, &dir, "game", &web, &files, "page").unwrap();
        assert_ne!(sw, std::fs::read_to_string(dir.join("sw.js")).unwrap());
        // Offline off: no worker.
        let (written, head) = write_app_files(&dir, &dir, "game", &WebSettings { offline: false, ..web }, &files, "page").unwrap();
        assert!(!written.contains(&"sw.js".to_string()) && !head.contains("serviceWorker"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copy_tree_keeps_imported_and_leaves_other_dot_entries_out() {
        let dir = std::env::temp_dir().join(format!("pocket-copy-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let from = dir.join("project");
        for sub in [".imported/assets", ".pocket", "assets", "scripts"] {
            std::fs::create_dir_all(from.join(sub)).unwrap();
        }
        std::fs::write(from.join(".imported/assets/level.blend.glb"), b"glb").unwrap();
        std::fs::write(from.join(".pocket/session.json"), b"{}").unwrap();
        std::fs::write(from.join(".hidden"), b"x").unwrap();
        std::fs::write(from.join("assets/level.blend"), b"blend").unwrap();
        std::fs::write(from.join("scripts/main.ts"), b"").unwrap();
        let to = dir.join("out");
        let bytes = copy_tree(&from, &to, &["scripts"]).unwrap();
        assert_eq!(bytes, 8);
        assert!(to.join(".imported/assets/level.blend.glb").is_file());
        assert!(to.join("assets/level.blend").is_file());
        assert!(!to.join(".pocket").exists() && !to.join(".hidden").exists() && !to.join("scripts").exists());
        let _ = std::fs::remove_dir_all(&dir);
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

/// `pocket pack <project> --ios`: the project as an app for the iOS Simulator (docs/build-system.md,
/// iOS): `<name>.app` with the runtime as its executable, the game under `game/` as `pack` lays it
/// out beside the runtime, and an Info.plist; signed ad hoc, as the simulator asks of an arm64
/// app. The runtime finds `game/` in its bundle when it is started without a project.
pub fn pack_ios(ws: &Workspace, config: &str, target: &str, out: Option<&Path>) -> Result<Report> {
    let t0 = Instant::now();
    if ws.target_of(config)? != "ios-sim" {
        return Ok(Report::failure("pack", format!("--ios needs an ios-sim configuration; '{config}' targets {}", ws.target_of(config)?)));
    }
    let project = find_project(ws, target).ok_or_else(|| anyhow!("'{target}' is not a project with project.toml"))?;
    let outcome = build_targets(ws, config, &["pocket_runtime".to_string()], false)?;
    if !outcome.ok {
        let mut rep = Report::failure("pack", "iOS runtime build failed");
        rep.diagnostics = parse_compiler_diagnostics(&outcome.output);
        if rep.diagnostics.is_empty() {
            rep.summary = format!("iOS runtime build failed:\n{}", crate::commands::tail(&outcome.output, 40));
        }
        return Ok(rep);
    }
    let bundle = bundle_project(ws, &project, None)?;
    let name = project.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "game".into());
    let ident: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let exe_name: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>();
    let exe_name = if exe_name.is_empty() { "Game".to_string() } else { exe_name };
    let bundle_id = format!("dev.pocket.{ident}");
    let app = out.map(|p| p.to_path_buf()).unwrap_or_else(|| ws.root.join("dist").join("ios").join(format!("{name}.app")));
    if app.exists() {
        std::fs::remove_dir_all(&app).with_context(|| format!("clearing {}", app.display()))?;
    }
    std::fs::create_dir_all(app.join("game"))?;
    let exe_dst = app.join(&exe_name);
    std::fs::copy(exe_path(ws, config, "pocket_runtime")?, &exe_dst)?;
    set_executable(&exe_dst)?;
    let mut total = std::fs::metadata(&exe_dst)?.len();
    total += write_game_data(ws, &project, &bundle.out, &app.join("game"))?;
    // Held the way the window is shaped: wider than tall plays in landscape.
    let settings: Value = serde_json::from_str(&std::fs::read_to_string(app.join("game").join("project.json"))?)?;
    let window = settings.get("window").cloned().unwrap_or(Value::Null);
    let (w, h) = (window.get("width").and_then(|v| v.as_i64()).unwrap_or(960), window.get("height").and_then(|v| v.as_i64()).unwrap_or(540));
    let orientations = if w >= h {
        "<string>UIInterfaceOrientationLandscapeLeft</string><string>UIInterfaceOrientationLandscapeRight</string>"
    } else {
        "<string>UIInterfaceOrientationPortrait</string>"
    };
    let title = window.get("title").and_then(|v| v.as_str()).unwrap_or(&name).replace('&', "&amp;").replace('<', "&lt;");
    let sdk = toolchain::xcrun(&["--sdk", "iphonesimulator", "--show-sdk-version"]).unwrap_or_default();
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>{bundle_id}</string>
  <key>CFBundleExecutable</key><string>{exe_name}</string>
  <key>CFBundleName</key><string>{exe_name}</string>
  <key>CFBundleDisplayName</key><string>{title}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>CFBundleShortVersionString</key><string>{version}</string>
  <key>CFBundleSupportedPlatforms</key><array><string>iPhoneSimulator</string></array>
  <key>DTPlatformName</key><string>iphonesimulator</string>
  <key>DTSDKName</key><string>iphonesimulator{sdk}</string>
  <key>MinimumOSVersion</key><string>17.0</string>
  <key>LSRequiresIPhoneOS</key><true/>
  <key>UIDeviceFamily</key><array><integer>1</integer><integer>2</integer></array>
  <key>UIRequiredDeviceCapabilities</key><array><string>arm64</string><string>metal</string></array>
  <key>UISupportedInterfaceOrientations</key><array>{orientations}</array>
  <key>UISupportedInterfaceOrientations~ipad</key><array>{orientations}</array>
  <key>UILaunchScreen</key><dict/>
  <key>UIRequiresFullScreen</key><true/>
  <key>UIStatusBarHidden</key><true/>
  <key>UIApplicationSupportsIndirectInputEvents</key><true/>
</dict>
</plist>
"#,
        version = ws.file.workspace.version
    );
    std::fs::write(app.join("Info.plist"), plist)?;
    let sign = std::process::Command::new("codesign").env("DEVELOPER_DIR", toolchain::xcode_dir()?).args(["--force", "--sign", "-", "--timestamp=none"]).arg(&app).output().context("running codesign")?;
    if !sign.status.success() {
        bail!("codesign failed: {}", String::from_utf8_lossy(&sign.stderr).trim());
    }
    let mut rep = Report::success("pack", format!("packed {name} as {} for the iOS Simulator ({} MB)", app.display(), total / (1024 * 1024)));
    rep.data = json!({ "project": project, "app": app, "bundle_id": bundle_id, "config": config, "bytes": total });
    rep.elapsed_ms = t0.elapsed().as_millis();
    Ok(rep)
}
