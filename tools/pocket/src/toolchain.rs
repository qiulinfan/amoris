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
    // Some CMake projects (Draco) look for Emscripten by this variable, as emsdk_env.sh sets it.
    cmd.env("EMSCRIPTEN", &sdk.emscripten);
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![sdk.emscripten.clone()];
    paths.extend(std::env::split_paths(&path));
    if let Ok(joined) = std::env::join_paths(paths) {
        cmd.env("PATH", joined);
    }
}

/// A program on PATH. On Windows a name without an extension is tried with each of PATHEXT's
/// (`.exe`, `.cmd`, ...), and the Store's app-execution aliases under `WindowsApps` are passed
/// over: `python3.exe` there opens the Store instead of running Python.
pub fn which(name: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    let files: Vec<String> = if cfg!(windows) && Path::new(name).extension().is_none() {
        let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        exts.split(';').filter(|e| !e.is_empty()).map(|e| format!("{name}{}", e.to_ascii_lowercase())).collect()
    } else {
        vec![name.to_string()]
    };
    for dir in std::env::split_paths(&path) {
        if cfg!(windows) && dir.to_string_lossy().to_ascii_lowercase().contains("\\microsoft\\windowsapps") {
            continue;
        }
        for file in &files {
            let candidate = dir.join(file);
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }
    None
}

/// Python 3 for the scripts the tool runs: POCKET_PYTHON, else `python3`, else (Windows, where
/// the python.org installer makes no python3.exe) `python`.
pub fn python() -> Option<String> {
    if let Ok(p) = std::env::var("POCKET_PYTHON") {
        return Some(p);
    }
    which("python3").or_else(|| if cfg!(windows) { which("python") } else { None })
}

/// LLVM on Windows when clang++ is not on PATH: POCKET_LLVM, the newest `llvm-*` (or `llvm`)
/// under ~/.pocket-tools (where the release archive is unpacked without an installer), or the
/// official installer's directory.
fn windows_llvm_bin() -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = vec![];
    if let Ok(v) = std::env::var("POCKET_LLVM") {
        roots.push(PathBuf::from(v));
    }
    if let Some(home) = std::env::var_os("USERPROFILE") {
        let tools = PathBuf::from(home).join(".pocket-tools");
        let mut found: Vec<PathBuf> = std::fs::read_dir(&tools)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n == "llvm" || n.starts_with("llvm-")))
            .collect();
        // Newest version first: llvm-23.1.2 before llvm-22.1.8.
        found.sort_by_key(|p| std::cmp::Reverse(version_key(p)));
        roots.extend(found);
    }
    roots.push(PathBuf::from(r"C:\Program Files\LLVM"));
    roots.into_iter().map(|r| r.join("bin")).find(|b| b.join("clang++.exe").is_file())
}

fn version_key(p: &Path) -> Vec<u64> {
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name.trim_start_matches("llvm").trim_start_matches('-').split('.').map(|s| s.parse().unwrap_or(0)).collect()
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
    if cfg!(windows) {
        return detect_windows();
    }
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

/// Windows: LLVM's clang++ driving the MSVC ABI (the Visual Studio C++ library and Windows SDK,
/// which clang finds itself), lld to link and llvm-lib to archive. The GNU-style driver keeps
/// one set of flags across hosts; clang-cl would need its own.
fn detect_windows() -> Result<Toolchain> {
    let llvm = windows_llvm_bin();
    let beside = |name: &str| llvm.as_ref().map(|b| b.join(name).to_string_lossy().into_owned());
    let cxx = std::env::var("POCKET_CXX").ok().or_else(|| which("clang++")).or_else(|| beside("clang++.exe")).context(
        "clang++ not found: unpack an LLVM release (clang+llvm-<version>-x86_64-pc-windows-msvc) under ~/.pocket-tools/llvm-<version>, put its bin on PATH, or set POCKET_CXX or POCKET_LLVM",
    )?;
    let sibling = |name: &str| Path::new(&cxx).parent().map(|d| d.join(name)).filter(|p| p.is_file()).map(|p| p.to_string_lossy().into_owned());
    let cc = std::env::var("POCKET_CC").ok().or_else(|| sibling("clang.exe")).unwrap_or_else(|| cxx.replace("clang++", "clang"));
    let ar = std::env::var("POCKET_AR").ok().or_else(|| sibling("llvm-lib.exe")).or_else(|| which("llvm-lib")).context("llvm-lib not found beside clang++")?;
    let out = command(&cxx).arg("--version").output().context("running clang++ --version")?;
    if !out.status.success() {
        bail!("{} --version failed: {}", cxx, String::from_utf8_lossy(&out.stderr));
    }
    let cxx_version = String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or("").to_string();
    let machine = command(&cxx).arg("-dumpmachine").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    if !machine.ends_with("windows-msvc") {
        bail!("{cxx} targets {machine}; the engine builds for the MSVC ABI (x86_64-pc-windows-msvc): use an LLVM release for Windows, not a MinGW clang");
    }
    Ok(Toolchain {
        cxx,
        cc,
        ar,
        ninja: which("ninja"),
        cmake: which("cmake"),
        developer_dir: None,
        cxx_version,
        host_os: "windows".into(),
        target: "native".into(),
        triple: None,
        sysroot: None,
    })
}

/// File names on the host: an executable's (`.exe` on Windows) and a static library's
/// (`name.lib` on Windows, `libname.a` elsewhere).
pub fn exe_name(name: &str, host_os: &str) -> String {
    if host_os == "windows" { format!("{name}.exe") } else { name.to_string() }
}

pub fn static_lib_name(name: &str, host_os: &str) -> String {
    if host_os == "windows" { format!("{name}.lib") } else { format!("lib{name}.a") }
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
