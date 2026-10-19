//! `cargo xtask export-web <project> [--out DIR] [--zip FILE]`: a game as a static web folder
//! (docs/spec/web-export.md). The folder holds the player page (`web/play.html` as `index.html`,
//! `web/player.js`), the game's module built as the web step builds it (`web_pack::module`, with
//! the `-O3` variant when `wasm-opt` is found) with its worker and page glue (`web/game.js`,
//! `web/amoris.js`), the committed WebGPU viewport (`web/viewport/`), and in `game/` the project's
//! web package (amoris-web's `pack` example, its scripts compiled natively) beside its asset files.
//! A `.gltf` is written as binary glTF at the same path, its buffers and images inside, since the
//! browser's importer reads one file per model and no data-URI image. Every path is relative and
//! nothing needs a server, so the
//! folder, or the zip with `index.html` at its root, plays from any static host (itch.io's HTML5
//! games among them).

use crate::report::{Problem, StepResult};
use crate::run::{self, Env};
use crate::steps::{web, web_pack};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The project files the viewport fetches, as `tools/build_web.sh` copies them.
const ASSETS: &[&str] = &[
    "glb", "gltf", "bin", "png", "jpg", "jpeg", "ktx2", "ply", "spz", "splat", "wav", "ogg", "mp3",
    "flac", "ttf",
];

/// The page's own files, from `web/`, at their published paths.
const PAGE: &[(&str, &str)] = &[
    ("player.js", "player.js"),
    ("amoris.js", "amoris.js"),
    ("game.js", "game.js"),
    ("viewport/viewport.js", "viewport/viewport.js"),
    (
        "viewport/pkg/amoris_viewport.js",
        "viewport/pkg/amoris_viewport.js",
    ),
    (
        "viewport/pkg/amoris_viewport_bg.wasm",
        "viewport/pkg/amoris_viewport_bg.wasm",
    ),
];

struct Options {
    project: PathBuf,
    out: Option<PathBuf>,
    zip: Option<PathBuf>,
}

fn usage() -> Problem {
    Problem::new(
        "check.usage",
        "cargo xtask export-web <project> [--out DIR] [--zip FILE]",
        json!({}),
    )
}

fn parse(args: &[String]) -> Result<Options, Problem> {
    let (mut project, mut out, mut zip) = (None, None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = Some(PathBuf::from(it.next().ok_or_else(usage)?)),
            "--zip" => zip = Some(PathBuf::from(it.next().ok_or_else(usage)?)),
            s if !s.starts_with("--") && project.is_none() => project = Some(PathBuf::from(s)),
            _ => return Err(usage()),
        }
    }
    Ok(Options {
        project: project.ok_or_else(usage)?,
        out,
        zip,
    })
}

fn failed(what: String) -> Problem {
    Problem::new("check.command_failed", what, json!({}))
}

/// Runs the export; prints what it wrote as JSON. False when a step failed (its problems printed).
pub fn main(env: &Env, args: &[String]) -> Result<bool, Problem> {
    let opts = parse(args)?;
    let cwd = std::env::current_dir().map_err(|e| failed(e.to_string()))?;
    let project = cwd.join(&opts.project);
    if !project.join("project.toml").is_file() {
        return Err(Problem::new(
            "check.usage",
            format!("{} has no project.toml", project.display()),
            json!({}),
        ));
    }
    let slug = web_pack::slug(
        &project
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "game".into()),
    );
    let out = opts
        .out
        .map_or_else(|| env.out.join("export-web").join(&slug), |o| cwd.join(o));
    let mut step = StepResult::new("export-web");
    let log = env.new_log("export-web");
    let ok = export(env, &mut step, &project, &out, &log);
    let zipped = match (&opts.zip, ok) {
        (Some(z), true) => Some(zip(&out, &cwd.join(z))),
        _ => None,
    };
    let zipped = match zipped.transpose() {
        Ok(z) => z,
        Err(p) => {
            step.error(p);
            None
        }
    };
    for p in &step.errors {
        eprintln!("{}: {}", p.code, p.message);
        if !p.detail.is_empty() {
            eprintln!(
                "{}",
                serde_json::to_string_pretty(&p.detail).unwrap_or_default()
            );
        }
    }
    for p in &step.warnings {
        eprintln!("warning: {}: {}", p.code, p.message);
    }
    eprintln!("log: {}", env.rel(&log));
    if !step.errors.is_empty() {
        return Ok(false);
    }
    let (files, bytes) = tree_size(&out);
    let mut report = json!({
        "out": out.display().to_string(),
        "files": files,
        "bytes": bytes,
        "measurements": step.measurements.iter().map(|m| json!({"name": m.name, "value": m.value, "unit": m.unit})).collect::<Vec<_>>(),
    });
    if let Some((path, size)) = zipped {
        report["zip"] = json!({"path": path.display().to_string(), "bytes": size});
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&report).unwrap_or_default()
    );
    Ok(true)
}

fn export(env: &Env, step: &mut StepResult, project: &Path, out: &Path, log: &Path) -> bool {
    let _ = std::fs::remove_dir_all(out);
    let opt = env.out.join("export-web-opt");
    let _ = std::fs::remove_dir_all(&opt);
    let target_dir =
        std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| env.root.join("target"), PathBuf::from);
    // The module into <out>/pkg, and its -O3 variant, which ships when it was built.
    let Some(opt_built) = web_pack::module(env, step, &target_dir, out, &opt, log) else {
        return false;
    };
    if opt_built {
        let wasm = "pkg/amoris_web_bg.wasm";
        if let Err(e) = std::fs::copy(opt.join(wasm), out.join(wasm)) {
            step.error(failed(format!("cannot copy the -O3 module: {e}")));
            return false;
        }
    }
    let _ = std::fs::remove_dir_all(&opt);
    let web = env.root.join(web::PAGE_DIR);
    for (from, to) in PAGE {
        let target = out.join(to);
        let copied = target
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::copy(web.join(from), &target));
        if let Err(e) = copied {
            step.error(failed(format!("cannot copy web/{from}: {e}")));
            return false;
        }
    }
    // The web package and the assets beside it.
    let game = out.join("game");
    if let Err(e) = std::fs::create_dir_all(&game) {
        step.error(failed(format!("cannot create {}: {e}", game.display())));
        return false;
    }
    let package = game.join("package.json");
    let mut pack = env.cargo([
        "run",
        "-q",
        "-p",
        "amoris-web",
        "--example",
        "pack",
        "--release",
        "--locked",
        "--",
    ]);
    pack.arg(project).arg(&package);
    if !web_pack::command(step, pack, log, "packing the project") {
        return false;
    }
    let mut assets = Vec::new();
    if let Err(e) = list_assets(project, Path::new(""), &mut assets) {
        step.error(failed(format!("cannot list {}: {e}", project.display())));
        return false;
    }
    for rel in &assets {
        let target = game.join(rel);
        let written = target
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                if rel.extension().is_some_and(|e| e == "gltf") {
                    gltf_to_glb(&project.join(rel)).and_then(|bytes| std::fs::write(&target, bytes))
                } else {
                    std::fs::copy(project.join(rel), &target).map(|_| ())
                }
            });
        if let Err(e) = written {
            step.error(failed(format!("cannot export {}: {e}", rel.display())));
            return false;
        }
    }
    // The page, named after the game, with the package's size for its progress bar.
    let name = std::fs::read(&package)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v["name"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "Amoris".into());
    let package_bytes = std::fs::metadata(&package).map_or(0, |m| m.len());
    let page = match std::fs::read_to_string(web.join("play.html")) {
        Ok(p) => p,
        Err(e) => {
            step.error(failed(format!("cannot read web/play.html: {e}")));
            return false;
        }
    };
    let page = page
        .replacen(
            "<html lang=\"en\">",
            &format!("<html lang=\"en\" data-package-bytes=\"{package_bytes}\">"),
            1,
        )
        .replacen(
            "<title>Amoris</title>",
            &format!("<title>{}</title>", html_escape(&name)),
            1,
        )
        .replacen(
            "<h1 id=\"name\">Amoris</h1>",
            &format!("<h1 id=\"name\">{}</h1>", html_escape(&name)),
            1,
        );
    if let Err(e) = std::fs::write(out.join("index.html"), page) {
        step.error(failed(format!("cannot write index.html: {e}")));
        return false;
    }
    step.measure("export.assets", assets.len() as f64, "files");
    step.measure("export.package", package_bytes as f64 / 1024.0, "KiB");
    true
}

/// The project's asset files, relative to it, in a stable order; hidden directories and the
/// project's own tooling (`.amoris/`, `node_modules/`, `scripts/`) are left out.
fn list_assets(root: &Path, rel: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(root.join(rel))?.collect::<Result<_, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = rel.join(&name);
        if entry.file_type()?.is_dir() {
            let skip = name.starts_with('.')
                || name == "node_modules"
                || (rel.as_os_str().is_empty() && name == "scripts");
            if !skip {
                list_assets(root, &path, out)?;
            }
        } else if Path::new(&name)
            .extension()
            .is_some_and(|e| ASSETS.contains(&e.to_string_lossy().to_ascii_lowercase().as_str()))
        {
            out.push(path);
        }
    }
    Ok(())
}

/// A `.gltf` as binary glTF (GLB): its buffers, whether separate files or data URIs, become the
/// one binary chunk, and each image named by a URI a buffer view in it. The browser's importer
/// (`import_glb_bytes`, the `gltf` crate's slice import) reads no file and no image data URI; it
/// tells GLB from JSON by the magic, so the file keeps its name and the scene's references hold.
fn gltf_to_glb(path: &Path) -> std::io::Result<Vec<u8>> {
    let bad = |e: String| std::io::Error::new(std::io::ErrorKind::InvalidData, e);
    let mut doc: Value =
        serde_json::from_slice(&std::fs::read(path)?).map_err(|e| bad(e.to_string()))?;
    let dir = path.parent().unwrap_or(Path::new("."));
    // A URI's bytes and its media type (a data URI's, or by the file's extension).
    let read = |uri: &str| -> std::io::Result<(Vec<u8>, Option<String>)> {
        if let Some(rest) = uri.strip_prefix("data:") {
            let (mime, data) = rest
                .split_once(";base64,")
                .ok_or_else(|| bad(format!("{}: a data URI that is not base64", path.display())))?;
            let bytes = unbase64(data).ok_or_else(|| {
                bad(format!(
                    "{}: a data URI that does not decode",
                    path.display()
                ))
            })?;
            return Ok((bytes, (!mime.is_empty()).then(|| mime.to_owned())));
        }
        let file = dir.join(percent_decode(uri));
        let bytes = std::fs::read(&file)
            .map_err(|e| bad(format!("{uri} (from {}): {e}", path.display())))?;
        let mime = match file
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
        {
            Some(e) if e == "png" => Some("image/png"),
            Some(e) if e == "jpg" || e == "jpeg" => Some("image/jpeg"),
            Some(e) if e == "ktx2" => Some("image/ktx2"),
            _ => None,
        };
        Ok((bytes, mime.map(str::to_owned)))
    };
    let mut bin: Vec<u8> = Vec::new();
    let append = |bin: &mut Vec<u8>, bytes: &[u8]| {
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        let at = bin.len();
        bin.extend_from_slice(bytes);
        at
    };
    let mut offsets = Vec::new();
    for buffer in doc["buffers"].as_array().cloned().unwrap_or_default() {
        let uri = buffer["uri"]
            .as_str()
            .ok_or_else(|| bad(format!("{}: a buffer without a URI", path.display())))?;
        let (bytes, _) = read(uri)?;
        offsets.push(append(&mut bin, &bytes));
    }
    if let Some(views) = doc.get_mut("bufferViews").and_then(Value::as_array_mut) {
        for view in views {
            let buffer = view["buffer"].as_u64().unwrap_or(0) as usize;
            let offset = view["byteOffset"].as_u64().unwrap_or(0);
            let base = *offsets
                .get(buffer)
                .ok_or_else(|| bad(format!("{}: a view of a missing buffer", path.display())))?;
            view["buffer"] = json!(0);
            view["byteOffset"] = json!(base as u64 + offset);
        }
    }
    let mut new_views = Vec::new();
    if let Some(images) = doc.get_mut("images").and_then(Value::as_array_mut) {
        for image in images {
            let Some(uri) = image["uri"].as_str().map(str::to_owned) else {
                continue;
            };
            let (bytes, mime) = read(&uri)?;
            let at = append(&mut bin, &bytes);
            new_views.push(json!({"buffer": 0, "byteOffset": at, "byteLength": bytes.len()}));
            let o = image.as_object_mut().expect("an image is an object");
            o.remove("uri");
            o.insert("_view".into(), json!(new_views.len() - 1));
            if let Some(m) =
                mime.or_else(|| o.get("mimeType").and_then(Value::as_str).map(str::to_owned))
            {
                o.insert("mimeType".into(), json!(m));
            }
        }
    }
    if !new_views.is_empty() {
        let views = doc
            .as_object_mut()
            .expect("a glTF document is an object")
            .entry("bufferViews")
            .or_insert_with(|| json!([]));
        if views.is_null() {
            *views = json!([]);
        }
        let first = views.as_array().map_or(0, Vec::len);
        views
            .as_array_mut()
            .expect("bufferViews is a list")
            .extend(new_views);
        for image in doc
            .get_mut("images")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            if let Some(i) = image.as_object_mut().and_then(|o| o.remove("_view")) {
                image["bufferView"] = json!(first + i.as_u64().unwrap_or(0) as usize);
            }
        }
    }
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    if bin.is_empty() {
        doc.as_object_mut().map(|o| o.remove("buffers"));
    } else {
        doc["buffers"] = json!([{"byteLength": bin.len()}]);
    }
    let mut text = serde_json::to_vec(&doc).map_err(|e| bad(e.to_string()))?;
    while !text.len().is_multiple_of(4) {
        text.push(b' ');
    }
    let total = 12 + 8 + text.len() + if bin.is_empty() { 0 } else { 8 + bin.len() };
    let u32le = |n: usize| (n as u32).to_le_bytes();
    let mut glb = Vec::with_capacity(total);
    glb.extend_from_slice(b"glTF");
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&u32le(total));
    glb.extend_from_slice(&u32le(text.len()));
    glb.extend_from_slice(b"JSON");
    glb.extend_from_slice(&text);
    if !bin.is_empty() {
        glb.extend_from_slice(&u32le(bin.len()));
        glb.extend_from_slice(b"BIN\0");
        glb.extend_from_slice(&bin);
    }
    Ok(glb)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Standard base64 (padding optional); `None` for any other character.
fn unbase64(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let text = text.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for &c in text {
        acc = (acc << 6) | u32::from(value(c)?);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn tree_size(dir: &Path) -> (u64, u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    entries.flatten().fold((0, 0), |(n, b), e| {
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            let (n2, b2) = tree_size(&e.path());
            (n + n2, b + b2)
        } else {
            (n + 1, b + e.metadata().map_or(0, |m| m.len()))
        }
    })
}

/// Zips the folder with its files at the zip's root (Python's `zipfile`, deflated), in a stable
/// order; the zip's path and size.
fn zip(dir: &Path, to: &Path) -> Result<(PathBuf, u64), Problem> {
    let python = run::python().ok_or_else(|| {
        Problem::new(
            "check.tool_missing",
            "no Python 3 to write the zip",
            json!({"tool": "python"}),
        )
    })?;
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| failed(format!("cannot create {}: {e}", parent.display())))?;
    }
    let script = "import os,sys,zipfile\n\
        root,to=sys.argv[1],sys.argv[2]\n\
        files=sorted(os.path.relpath(os.path.join(d,f),root).replace(os.sep,'/') for d,_,fs in os.walk(root) for f in fs)\n\
        with zipfile.ZipFile(to,'w',zipfile.ZIP_DEFLATED,compresslevel=9) as z:\n\
        \x20 for f in files: z.write(os.path.join(root,f),f)\n";
    let out = Command::new(python)
        .args(["-c", script])
        .arg(dir)
        .arg(to)
        .output()
        .map_err(|e| failed(format!("python could not start: {e}")))?;
    if !out.status.success() {
        return Err(failed(format!(
            "writing the zip failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    let size = std::fs::metadata(to).map_or(0, |m| m.len());
    Ok((to.to_path_buf(), size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_decodes_the_standard_alphabet() {
        assert_eq!(unbase64("").unwrap(), b"");
        assert_eq!(unbase64("Zg==").unwrap(), b"f");
        assert_eq!(unbase64("Zm8=").unwrap(), b"fo");
        assert_eq!(unbase64("Zm9vYmFy").unwrap(), b"foobar");
        assert_eq!(unbase64("+/8=").unwrap(), [0xfb, 0xff]);
        assert!(unbase64("Zm9v!").is_none());
    }

    /// The GLB's JSON and binary chunks.
    fn chunks(glb: &[u8]) -> (Value, Vec<u8>) {
        assert_eq!(&glb[..4], b"glTF");
        let n = |at: usize| u32::from_le_bytes(glb[at..at + 4].try_into().unwrap()) as usize;
        assert_eq!(n(8), glb.len());
        let json_len = n(12);
        assert_eq!(&glb[16..20], b"JSON");
        let doc = serde_json::from_slice(&glb[20..20 + json_len]).unwrap();
        let at = 20 + json_len;
        let bin = if at < glb.len() {
            assert_eq!(&glb[at + 4..at + 8], b"BIN\0");
            glb[at + 8..at + 8 + n(at)].to_vec()
        } else {
            Vec::new()
        };
        (doc, bin)
    }

    #[test]
    fn gltf_files_become_glb_with_their_images_and_buffers_inside() {
        let dir = std::env::temp_dir().join(format!("xtask-export-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("textures")).unwrap();
        std::fs::write(dir.join("textures/a b.png"), b"png").unwrap();
        std::fs::write(dir.join("m.bin"), b"binary").unwrap();
        std::fs::write(
            dir.join("m.gltf"),
            r#"{"asset":{"version":"2.0"},"images":[{"uri":"textures/a%20b.png"},{"uri":"data:image/jpeg;base64,AAE="}],
               "buffers":[{"uri":"m.bin","byteLength":6},{"uri":"data:application/octet-stream;base64,Zm9v","byteLength":3}],
               "bufferViews":[{"buffer":1,"byteOffset":1,"byteLength":2},{"buffer":0,"byteLength":6}]}"#,
        )
        .unwrap();
        let (doc, bin) = chunks(&gltf_to_glb(&dir.join("m.gltf")).unwrap());
        assert_eq!(doc["buffers"], json!([{"byteLength": bin.len()}]));
        let view = |i: usize| {
            let v = &doc["bufferViews"][i];
            assert_eq!(v["buffer"], 0);
            let at = v["byteOffset"].as_u64().unwrap_or(0) as usize;
            bin[at..at + v["byteLength"].as_u64().unwrap() as usize].to_vec()
        };
        assert_eq!(view(0), b"oo");
        assert_eq!(view(1), b"binary");
        for (i, (bytes, mime)) in [(&b"png"[..], "image/png"), (&[0u8, 1][..], "image/jpeg")]
            .iter()
            .enumerate()
        {
            let image = &doc["images"][i];
            assert!(image.get("uri").is_none());
            assert_eq!(image["mimeType"], *mime);
            assert_eq!(view(image["bufferView"].as_u64().unwrap() as usize), *bytes);
        }
        std::fs::write(
            dir.join("broken.gltf"),
            r#"{"images":[{"uri":"missing.png"}]}"#,
        )
        .unwrap();
        assert!(gltf_to_glb(&dir.join("broken.gltf")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn assets_leave_out_sources_and_tooling() {
        let dir = std::env::temp_dir().join(format!("xtask-assets-{}", std::process::id()));
        for d in ["scripts", ".amoris/types", "clouds", "node_modules/x"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        for f in [
            "project.toml",
            "scene.json",
            "a.glb",
            "scripts/main.ts",
            "scripts/icon.png",
            ".amoris/types/t.png",
            "clouds/c.gltf",
            "node_modules/x/y.png",
            "T.PNG",
        ] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        let mut found = Vec::new();
        list_assets(&dir, Path::new(""), &mut found).unwrap();
        let found: Vec<String> = found
            .iter()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(found, ["T.PNG", "a.glb", "clouds/c.gltf"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
