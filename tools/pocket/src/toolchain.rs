//! Toolchain discovery: compilers, ninja, cmake, and the Apple developer directory.

use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Serialize, Debug, Clone)]
pub struct Toolchain {
    pub cxx: String,
    pub cc: String,
    pub ar: String,
    pub ninja: Option<String>,
    pub cmake: Option<String>,
    pub developer_dir: Option<String>,
    pub cxx_version: String,
    pub host_os: String,
    /// "native", "wasm" or "ios-sim".
    pub target: String,
    /// A cross target's clang target triple and SDK (the iOS Simulator's), passed to every compile
    /// and link.
    pub triple: Option<String>,
    pub sysroot: Option<String>,
}

/// The iOS Simulator target: arm64, iOS 17 and later.
pub const IOS_SIM_TRIPLE: &str = "arm64-apple-ios17.0-simulator";

/// Xcode's developer directory (the iOS SDKs live there, not in the Command Line Tools):
/// POCKET_XCODE, else /Applications/Xcode.app.
pub fn xcode_dir() -> Result<String> {
    let dir = std::env::var("POCKET_XCODE").unwrap_or_else(|_| "/Applications/Xcode.app/Contents/Developer".into());
    if !Path::new(&dir).join("Platforms").join("iPhoneSimulator.platform").is_dir() {
        bail!("Xcode with the iOS Simulator platform not found at {dir} (install Xcode, or set POCKET_XCODE to its Contents/Developer)");
    }
    Ok(dir)
}

/// `xcrun` with Xcode's developer directory, for the iOS Simulator SDK.
pub fn xcrun(args: &[&str]) -> Result<String> {
    let out = Command::new("xcrun").env("DEVELOPER_DIR", xcode_dir()?).args(args).output().context("running xcrun")?;
    if !out.status.success() {
        bail!("xcrun {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The Emscripten SDK: POCKET_EMSDK, then EMSDK, then ~/.pocket-tools/emsdk.
#[derive(Serialize, Debug, Clone)]
pub struct Emsdk {
    pub root: PathBuf,
    pub emscripten: PathBuf,
    pub config_file: PathBuf,
    pub cmake_toolchain: PathBuf,
}

pub fn emsdk() -> Result<Emsdk> {
    let mut candidates: Vec<PathBuf> = vec![];
    for var in ["POCKET_EMSDK", "EMSDK"] {
        if let Ok(v) = std::env::var(var) {
            candidates.push(PathBuf::from(v));
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join(".pocket-tools").join("emsdk"));
    }
    for root in candidates {
        let emscripten = root.join("upstream").join("emscripten");
        if emscripten.join("em++").is_file() {
            return Ok(Emsdk { config_file: root.join(".emscripten"), cmake_toolchain: emscripten.join("cmake").join("Modules").join("Platform").join("Emscripten.cmake"), root, emscripten });
        }
    }
    bail!("Emscripten SDK not found: install it with `git clone https://github.com/emscripten-core/emsdk ~/.pocket-tools/emsdk && ~/.pocket-tools/emsdk/emsdk install latest && ~/.pocket-tools/emsdk/emsdk activate latest`, or set POCKET_EMSDK")
}

/// Environment for running Emscripten tools without sourcing emsdk_env.sh.
pub fn em_env(cmd: &mut Command, sdk: &Emsdk) {
    if sdk.config_file.is_file() {
        cmd.env("EM_CONFIG", &sdk.config_file);
    }
    cmd.env("EMSDK", &sdk.root);
    cmd.env("EMSDK_QUIET", "1");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![sdk.emscripten.clone()];
    paths.extend(std::env::split_paths(&path));
    if let Ok(joined) = std::env::join_paths(paths) {
        cmd.env("PATH", joined);
    }
}

pub fn which(name: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

/// Apple's shim tools (`clang`, `nm`, `lipo`) refuse to run while the Xcode license is
/// unaccepted. Pointing DEVELOPER_DIR at the Command Line Tools sidesteps that and is
/// harmless when Xcode is fine, so we do it whenever the directory exists.
pub fn developer_dir() -> Option<String> {
    if let Ok(d) = std::env::var("DEVELOPER_DIR") {
        return Some(d);
    }
    let clt = Path::new("/Library/Developer/CommandLineTools");
    if cfg!(target_os = "macos") && clt.exists() {
        return Some(clt.to_string_lossy().into_owned());
    }
    None
}

pub fn command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    if let Some(d) = developer_dir() {
        cmd.env("DEVELOPER_DIR", d);
    }
    cmd
}

pub fn detect_for(target: &str) -> Result<Toolchain> {
    if target == "wasm" {
        let sdk = emsdk()?;
        let cxx = sdk.emscripten.join("em++").to_string_lossy().into_owned();
        let mut probe = command(&cxx);
        em_env(&mut probe, &sdk);
        let out = probe.arg("--version").output().context("running em++ --version")?;
        if !out.status.success() {
            bail!("{} --version failed: {}", cxx, String::from_utf8_lossy(&out.stderr));
        }
        let cxx_version = String::from_utf8_lossy(&out.stdout).lines().find(|l| l.contains("emcc")).unwrap_or("").to_string();
        return Ok(Toolchain {
            cxx,
            cc: sdk.emscripten.join("emcc").to_string_lossy().into_owned(),
            ar: sdk.emscripten.join("emar").to_string_lossy().into_owned(),
            ninja: which("ninja"),
            cmake: which("cmake"),
            developer_dir: developer_dir(),
            cxx_version,
            host_os: std::env::consts::OS.to_string(),
            target: "wasm".into(),
            triple: None,
            sysroot: None,
        });
    }
    if target == "ios-sim" {
        let cxx = xcrun(&["--sdk", "iphonesimulator", "--find", "clang++"])?;
        let cc = xcrun(&["--sdk", "iphonesimulator", "--find", "clang"])?;
        let ar = xcrun(&["--sdk", "iphonesimulator", "--find", "ar"])?;
        let sysroot = xcrun(&["--sdk", "iphonesimulator", "--show-sdk-path"])?;
        let out = Command::new(&cxx).arg("--version").output().context("running clang++ --version")?;
        return Ok(Toolchain {
            cxx,
            cc,
            ar,
            ninja: which("ninja"),
            cmake: which("cmake"),
            developer_dir: Some(xcode_dir()?),
            cxx_version: String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or("").to_string(),
            host_os: std::env::consts::OS.to_string(),
            target: "ios-sim".into(),
            triple: Some(IOS_SIM_TRIPLE.into()),
            sysroot: Some(sysroot),
        });
    }
    detect()
}

pub fn detect() -> Result<Toolchain> {
    let cxx = std::env::var("POCKET_CXX").ok().or_else(|| which("clang++")).context("clang++ not found on PATH (set POCKET_CXX)")?;
    let cc = std::env::var("POCKET_CC").ok().or_else(|| which("clang")).unwrap_or_else(|| cxx.replace("clang++", "clang"));
    let ar = std::env::var("POCKET_AR").ok().or_else(|| which("ar")).unwrap_or_else(|| "ar".into());
    let out = command(&cxx).arg("--version").output().context("running clang++ --version")?;
    if !out.status.success() {
        bail!("{} --version failed: {}", cxx, String::from_utf8_lossy(&out.stderr));
    }
    let cxx_version = String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or("").to_string();
    Ok(Toolchain {
        cxx,
        cc,
        ar,
        ninja: which("ninja"),
        cmake: which("cmake"),
        developer_dir: developer_dir(),
        cxx_version,
        host_os: std::env::consts::OS.to_string(),
        target: "native".into(),
        triple: None,
        sysroot: None,
    })
}

pub fn ensure_dir(p: &Path) -> Result<()> {
    std::fs::create_dir_all(p).with_context(|| format!("creating {}", p.display()))
}

pub fn relative_to(path: &Path, base: &Path) -> PathBuf {
    pathdiff(path, base).unwrap_or_else(|| path.to_path_buf())
}

fn pathdiff(path: &Path, base: &Path) -> Option<PathBuf> {
    let path = path.components().collect::<Vec<_>>();
    let base = base.components().collect::<Vec<_>>();
    let mut i = 0;
    while i < path.len() && i < base.len() && path[i] == base[i] {
        i += 1;
    }
    let mut out = PathBuf::new();
    for _ in i..base.len() {
        out.push("..");
    }
    for c in &path[i..] {
        out.push(c.as_os_str());
    }
    Some(out)
}
