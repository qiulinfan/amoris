//! Third-party dependencies: prebuilt archives, foreign CMake builds and system frameworks,
//! all installed under `.pocket/deps/<name>-<version>/` and identified by a stamp file.

use crate::manifest::{Dependency, Workspace};
use crate::toolchain;
use anyhow::{bail, Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Serialize, Debug, Clone)]
pub struct DepStatus {
    pub name: String,
    pub version: String,
    pub kind: String,
    pub state: String,
    pub prefix: String,
}

pub fn prefix(ws: &Workspace, dep: &Dependency) -> PathBuf {
    prefix_for(ws, dep, "native")
}

/// Where a dependency is installed for a target; cmake sources get their own wasm build, and the
/// iOS Simulator its own of everything built or prebuilt for it.
pub fn prefix_for(ws: &Workspace, dep: &Dependency, target: &str) -> PathBuf {
    let v = if dep.version.is_empty() { "system".to_string() } else { dep.version.clone() };
    if target == "wasm" && dep.kind == "cmake" {
        return ws.deps_dir().join(format!("{}-{}-wasm", dep.name, v));
    }
    if target == "ios-sim" && matches!(dep.kind.as_str(), "cmake" | "prebuilt") {
        return ws.deps_dir().join(format!("{}-{}-ios-sim", dep.name, v));
    }
    ws.deps_dir().join(format!("{}-{}", dep.name, v))
}

/// Whether a dependency contributes headers and libraries on a target. On wasm, Emscripten
/// ports and prebuilt native archives do not; cmake sources are rebuilt and files are shared.
pub fn applies(dep: &Dependency, target: &str) -> bool {
    if dep.kind == "skip" {
        return false;
    }
    if target != "wasm" {
        return true;
    }
    if dep.wasm == "skip" || dep.wasm == "port" {
        return false;
    }
    matches!(dep.kind.as_str(), "cmake" | "file")
}

fn stamp_key(dep: &Dependency, target: &str) -> String {
    let mut h = Sha256::new();
    h.update(dep.sha256.as_bytes());
    h.update(dep.url.as_bytes());
    for a in &dep.cmake_args {
        h.update(a.as_bytes());
    }
    // Only cmake dependencies are built per target (into their own prefix); files and prebuilt
    // archives are shared, so their stamp must not depend on the target.
    if target != "native" && (dep.kind == "cmake" || (target == "ios-sim" && dep.kind == "prebuilt")) {
        h.update(target.as_bytes());
    }
    hex(&h.finalize())
}

pub fn is_ready(ws: &Workspace, dep: &Dependency) -> bool {
    is_ready_for(ws, dep, "native")
}

pub fn is_ready_for(ws: &Workspace, dep: &Dependency, target: &str) -> bool {
    if dep.kind == "system" || !applies(dep, target) {
        return true;
    }
    let stamp = prefix_for(ws, dep, target).join(".pocket-stamp");
    std::fs::read_to_string(stamp).map(|s| s.trim() == stamp_key(dep, target)).unwrap_or(false)
}

pub fn status(ws: &Workspace) -> Vec<DepStatus> {
    ws.file
        .dependencies
        .iter()
        .map(|d| DepStatus {
            name: d.name.clone(),
            version: d.version.clone(),
            kind: d.kind.clone(),
            state: if is_ready(ws, d) { "ready".into() } else { "missing".into() },
            prefix: if d.kind == "system" { String::new() } else { prefix(ws, d).to_string_lossy().into_owned() },
        })
        .collect()
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

fn download(url: &str, dest: &Path, expected_sha: &str) -> Result<()> {
    if dest.exists() && sha256_file(dest)? == expected_sha {
        return Ok(());
    }
    toolchain::ensure_dir(dest.parent().unwrap())?;
    let status = std::process::Command::new("curl")
        .args(["-L", "--fail", "--silent", "--show-error", "-o"])
        .arg(dest)
        .arg(url)
        .status()
        .context("running curl")?;
    if !status.success() {
        bail!("download failed: {url}");
    }
    let actual = sha256_file(dest)?;
    if actual != expected_sha {
        let _ = std::fs::remove_file(dest);
        bail!("sha256 mismatch for {url}: expected {expected_sha}, got {actual}");
    }
    Ok(())
}

fn strip(path: &Path, n: u32) -> Option<PathBuf> {
    let comps: Vec<_> = path.components().collect();
    if comps.len() <= n as usize {
        return None;
    }
    Some(comps[n as usize..].iter().collect())
}

fn extract(archive: &Path, dest: &Path, strip_components: u32) -> Result<()> {
    let name = archive.file_name().unwrap().to_string_lossy().to_string();
    toolchain::ensure_dir(dest)?;
    if name.ends_with(".zip") {
        let f = std::fs::File::open(archive)?;
        let mut z = zip::ZipArchive::new(f)?;
        for i in 0..z.len() {
            let mut entry = z.by_index(i)?;
            let Some(inner) = entry.enclosed_name() else { continue };
            let Some(rel) = strip(&inner, strip_components) else { continue };
            let out = dest.join(rel);
            if entry.is_dir() {
                toolchain::ensure_dir(&out)?;
            } else {
                toolchain::ensure_dir(out.parent().unwrap())?;
                let mut w = std::fs::File::create(&out)?;
                std::io::copy(&mut entry, &mut w)?;
                #[cfg(unix)]
                if let Some(mode) = entry.unix_mode() {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode))?;
                }
            }
        }
        return Ok(());
    }
    let raw: Box<dyn Read> = if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Box::new(flate2::read::GzDecoder::new(std::fs::File::open(archive)?))
    } else if name.ends_with(".tar.xz") {
        let mut input = std::io::BufReader::new(std::fs::File::open(archive)?);
        let mut decoded = Vec::new();
        lzma_rs::xz_decompress(&mut input, &mut decoded).context("xz decompress")?;
        Box::new(std::io::Cursor::new(decoded))
    } else {
        bail!("unsupported archive format: {name}");
    };
    let mut tar = tar::Archive::new(raw);
    for entry in tar.entries()? {
        let mut entry = entry?;
        let inner = entry.path()?.to_path_buf();
        let Some(rel) = strip(&inner, strip_components) else { continue };
        let out = dest.join(rel);
        if let Some(parent) = out.parent() {
            toolchain::ensure_dir(parent)?;
        }
        entry.unpack(&out)?;
    }
    Ok(())
}

pub fn setup_one(ws: &Workspace, dep: &Dependency, force: bool, log: &mut Vec<String>, target: &str) -> Result<DepStatus> {
    let pfx = prefix_for(ws, dep, target);
    if dep.kind == "system" {
        return Ok(DepStatus { name: dep.name.clone(), version: dep.version.clone(), kind: dep.kind.clone(), state: "system".into(), prefix: String::new() });
    }
    if !applies(dep, target) {
        return Ok(DepStatus { name: dep.name.clone(), version: dep.version.clone(), kind: dep.kind.clone(), state: format!("not used on {target}"), prefix: String::new() });
    }
    if !force && is_ready_for(ws, dep, target) {
        return Ok(DepStatus { name: dep.name.clone(), version: dep.version.clone(), kind: dep.kind.clone(), state: "ready".into(), prefix: pfx.to_string_lossy().into_owned() });
    }
    if dep.url.is_empty() || dep.sha256.is_empty() {
        bail!("dependency {} needs url and sha256", dep.name);
    }
    let file_name = dep.url.rsplit('/').next().unwrap().to_string();
    let cache = ws.pocket_dir().join("cache").join(&file_name);
    log.push(format!("fetch {} -> {}", dep.url, cache.display()));
    download(&dep.url, &cache, &dep.sha256)?;
    if pfx.exists() {
        std::fs::remove_dir_all(&pfx)?;
    }
    match dep.kind.as_str() {
        "prebuilt" => {
            log.push(format!("extract {} -> {}", file_name, pfx.display()));
            extract(&cache, &pfx, dep.strip_components)?;
        }
        "file" => {
            // A single file (a font, a data blob) installed as <prefix>/<file name>.
            toolchain::ensure_dir(&pfx)?;
            let dest = pfx.join(&file_name);
            log.push(format!("install {} -> {}", file_name, dest.display()));
            std::fs::copy(&cache, &dest).with_context(|| format!("copying {}", file_name))?;
        }
        "cmake" => {
            let src = ws.pocket_dir().join("src").join(format!("{}-{}", dep.name, dep.version));
            let bld = ws.pocket_dir().join("build").join(format!("{}-{}", dep.name, dep.version));
            if src.exists() {
                std::fs::remove_dir_all(&src)?;
            }
            log.push(format!("extract {} -> {}", file_name, src.display()));
            extract(&cache, &src, dep.strip_components)?;
            let cmake = toolchain::detect()?.cmake.context("cmake not found on PATH; needed for foreign builds")?;
            let sdk = if target == "wasm" { Some(toolchain::emsdk()?) } else { None };
            let bld = if target != "native" { bld.with_file_name(format!("{}-{}-{target}", dep.name, dep.version)) } else { bld };
            if bld.exists() {
                // A stale CMake cache would keep the previous configuration's flags.
                std::fs::remove_dir_all(&bld)?;
            }
            log.push(format!("cmake configure {} ({target})", dep.name));
            let mut cfg = toolchain::command(&cmake);
            cfg.arg("-S").arg(&src).arg("-B").arg(&bld).arg("-G").arg("Ninja").arg("-DCMAKE_BUILD_TYPE=Release").arg(format!("-DCMAKE_INSTALL_PREFIX={}", pfx.display()));
            if let Some(sdk) = &sdk {
                cfg.arg(format!("-DCMAKE_TOOLCHAIN_FILE={}", sdk.cmake_toolchain.display()));
                toolchain::em_env(&mut cfg, sdk);
            }
            let ios = if target == "ios-sim" { Some(toolchain::detect_for("ios-sim")?) } else { None };
            if let Some(tc) = &ios {
                // CMake's own iOS support, pointed at the simulator SDK with Xcode's compilers.
                cfg.env("DEVELOPER_DIR", tc.developer_dir.clone().unwrap_or_default());
                cfg.arg("-DCMAKE_SYSTEM_NAME=iOS").arg(format!("-DCMAKE_OSX_SYSROOT={}", tc.sysroot.clone().unwrap_or_default())).arg("-DCMAKE_OSX_ARCHITECTURES=arm64").arg("-DCMAKE_OSX_DEPLOYMENT_TARGET=17.0");
                cfg.arg(format!("-DCMAKE_C_COMPILER={}", tc.cc)).arg(format!("-DCMAKE_CXX_COMPILER={}", tc.cxx));
            }
            for a in &dep.cmake_args {
                cfg.arg(a);
            }
            run_logged(cfg, &format!("cmake configure {}", dep.name))?;
            log.push(format!("cmake build {}", dep.name));
            let mut b = toolchain::command(&cmake);
            if let Some(sdk) = &sdk {
                toolchain::em_env(&mut b, sdk);
            }
            if let Some(tc) = &ios {
                b.env("DEVELOPER_DIR", tc.developer_dir.clone().unwrap_or_default());
            }
            b.arg("--build").arg(&bld).arg("-j").arg(std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).to_string());
            run_logged(b, &format!("cmake build {}", dep.name))?;
            let mut i = toolchain::command(&cmake);
            if let Some(sdk) = &sdk {
                toolchain::em_env(&mut i, sdk);
            }
            if let Some(tc) = &ios {
                i.env("DEVELOPER_DIR", tc.developer_dir.clone().unwrap_or_default());
            }
            i.arg("--install").arg(&bld);
            run_logged(i, &format!("cmake install {}", dep.name))?;
        }
        other => bail!("dependency {}: unknown kind '{}'", dep.name, other),
    }
    for lib in &dep.libs {
        if !pfx.join(lib).exists() {
            bail!("dependency {}: expected library {} missing after setup", dep.name, lib);
        }
    }
    std::fs::write(pfx.join(".pocket-stamp"), stamp_key(dep, target))?;
    Ok(DepStatus { name: dep.name.clone(), version: dep.version.clone(), kind: dep.kind.clone(), state: "installed".into(), prefix: pfx.to_string_lossy().into_owned() })
}

fn run_logged(mut cmd: std::process::Command, what: &str) -> Result<()> {
    let out = cmd.output().with_context(|| what.to_string())?;
    if !out.status.success() {
        bail!("{what} failed:\n{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
