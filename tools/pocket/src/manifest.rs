//! Workspace (`pocket.toml`) and module (`module.toml`) manifests.

use anyhow::{anyhow, bail, Context, Result};
use indexmap::IndexMap;
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub fn find_root(start: &Path) -> Result<PathBuf> {
    let mut dir = Some(start.to_path_buf());
    while let Some(d) = dir {
        if d.join("pocket.toml").exists() {
            return Ok(d);
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    bail!("no pocket.toml found in {} or its parents", start.display())
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceSection {
    /// The workspace's name and version: part of the manifest's contract, read by people, not by the tool yet.
    #[allow(dead_code)]
    pub name: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub version: String,
    /// Directories scanned (recursively) for module.toml files.
    #[serde(default = "default_module_dirs")]
    pub module_dirs: Vec<String>,
}
fn default_module_dirs() -> Vec<String> { vec!["engine".into(), "third_party".into(), "tests".into(), "tools".into()] }

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct ToolchainSection {
    #[serde(default = "default_std")]
    pub cxx_standard: String,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub cxx_flags: Vec<String>,
    #[serde(default)]
    pub c_flags: Vec<String>,
    #[serde(default)]
    pub defines: Vec<String>,
    #[serde(default)]
    pub macos_deployment_target: Option<String>,
}
fn default_std() -> String { "c++26".into() }

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct ConfigSection {
    /// "native" (default), "wasm" (Emscripten: the web build) or "ios-sim" (the iOS Simulator on
    /// Apple silicon, with Xcode's clang and SDK).
    #[serde(default)]
    pub target: String,
    /// Replaces [toolchain] warnings for this config (Emscripten's driver turns -Werror on its own notes).
    #[serde(default)]
    pub warnings: Option<Vec<String>>,
    #[serde(default)]
    pub cxx_flags: Vec<String>,
    #[serde(default)]
    pub c_flags: Vec<String>,
    #[serde(default)]
    pub defines: Vec<String>,
    #[serde(default)]
    pub link_flags: Vec<String>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Dependency {
    pub name: String,
    #[serde(default)]
    pub version: String,
    /// "prebuilt" (archive with headers and libs), "cmake" (source archive built with CMake),
    /// "source" (an archive only unpacked, compiled by another dependency's setup step), "file" (a
    /// single file), "system" (frameworks/libs already present), "skip" (not used here).
    pub kind: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default = "one")]
    pub strip_components: u32,
    #[serde(default)]
    pub cmake_args: Vec<String>,
    #[serde(default)]
    pub include_dirs: Vec<String>,
    #[serde(default)]
    pub libs: Vec<String>,
    #[serde(default)]
    pub frameworks: Vec<String>,
    #[serde(default)]
    pub weak_frameworks: Vec<String>,
    #[serde(default)]
    pub link_flags: Vec<String>,
    #[serde(default)]
    pub defines: Vec<String>,
    /// Recorded for the dependency's provenance; the tool does not act on it.
    #[serde(default)]
    #[allow(dead_code)]
    pub license: String,
    /// Role on the wasm target: "" (cmake sources are rebuilt with Emscripten; prebuilt and system
    /// dependencies are skipped), "port" (an Emscripten port supplies it), "skip".
    #[serde(default)]
    pub wasm: String,
    /// System packages found with pkg-config (their include directories and libraries), for a
    /// "system" dependency on Linux.
    #[serde(default)]
    pub pkg_config: Vec<String>,
    /// Files under the prefix an executable needs beside it at run time (a DLL on Windows):
    /// `pocket build` copies them next to the executables, `pocket pack` into the game.
    #[serde(default)]
    pub runtime_files: Vec<String>,
    /// What changes on another host or target: keyed "<os>-<arch>" then "<os>" for the host
    /// ("linux-aarch64", "linux-x86_64", "linux", "windows-x86_64", "windows"), "ios-sim" then
    /// "ios" for the iOS Simulator target, each field given replacing the one above. The fields
    /// above describe macOS. A kind of "skip" leaves the dependency out there (a host tool on a
    /// device target). A prebuilt archive must be pinned for every other host it is used on.
    #[serde(default)]
    pub platforms: IndexMap<String, DependencyOverride>,
}
fn one() -> u32 { 1 }

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct DependencyOverride {
    pub kind: Option<String>,
    pub version: Option<String>,
    pub url: Option<String>,
    pub sha256: Option<String>,
    pub strip_components: Option<u32>,
    pub cmake_args: Option<Vec<String>>,
    pub include_dirs: Option<Vec<String>>,
    pub libs: Option<Vec<String>>,
    pub frameworks: Option<Vec<String>>,
    pub weak_frameworks: Option<Vec<String>>,
    pub link_flags: Option<Vec<String>>,
    pub defines: Option<Vec<String>>,
    pub pkg_config: Option<Vec<String>>,
    pub runtime_files: Option<Vec<String>>,
}

impl Dependency {
    /// The dependency as this host has it: its platform's fields over the shared ones, and its
    /// pkg-config packages' include directories and libraries added.
    fn for_host(self, os: &str, arch: &str) -> Result<Self> {
        self.resolved(&[format!("{os}-{arch}"), os.to_string()], os == "macos")
    }

    /// The dependency with the first of `keys` it has an override for laid over it; frameworks
    /// only where Apple's are (`apple`).
    fn resolved(mut self, keys: &[String], apple: bool) -> Result<Self> {
        let mut chosen = None;
        for k in keys {
            if let Some(o) = self.platforms.get(k) {
                chosen = Some(o.clone());
                break;
            }
        }
        if chosen.is_none() && !apple && self.kind == "prebuilt" {
            // The fields above are macOS's: its binaries would install here and fail at link time.
            bail!("dependency {}: no prebuilt archive pinned for {} (add [dependencies.platforms.{}] to pocket.toml)", self.name, keys.join(" or "), keys[0]);
        }
        if let Some(o) = chosen {
            if let Some(v) = o.kind { self.kind = v; }
            if let Some(v) = o.version { self.version = v; }
            if let Some(v) = o.url { self.url = v; }
            if let Some(v) = o.sha256 { self.sha256 = v; }
            if let Some(v) = o.strip_components { self.strip_components = v; }
            if let Some(v) = o.cmake_args { self.cmake_args = v; }
            if let Some(v) = o.include_dirs { self.include_dirs = v; }
            if let Some(v) = o.libs { self.libs = v; }
            if let Some(v) = o.frameworks { self.frameworks = v; }
            if let Some(v) = o.weak_frameworks { self.weak_frameworks = v; }
            if let Some(v) = o.link_flags { self.link_flags = v; }
            if let Some(v) = o.defines { self.defines = v; }
            if let Some(v) = o.pkg_config { self.pkg_config = v; }
            if let Some(v) = o.runtime_files { self.runtime_files = v; }
        }
        if !apple {
            // Frameworks are Apple's.
            self.frameworks.clear();
            self.weak_frameworks.clear();
        }
        if !self.pkg_config.is_empty() {
            let run = |flag: &str| -> Result<Vec<String>> {
                let out = std::process::Command::new("pkg-config").arg(flag).args(&self.pkg_config).output().with_context(|| format!("running pkg-config for {}", self.name))?;
                if !out.status.success() {
                    bail!("pkg-config {} {}: {}", flag, self.pkg_config.join(" "), String::from_utf8_lossy(&out.stderr).trim());
                }
                Ok(String::from_utf8_lossy(&out.stdout).split_whitespace().map(|s| s.to_string()).collect())
            };
            for f in run("--cflags-only-I")? {
                if let Some(dir) = f.strip_prefix("-I") { self.include_dirs.push(dir.to_string()); }
            }
            self.link_flags.extend(run("--libs")?);
        }
        Ok(self)
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceFile {
    pub workspace: WorkspaceSection,
    #[serde(default)]
    pub toolchain: ToolchainSection,
    #[serde(default)]
    pub configs: IndexMap<String, ConfigSection>,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ModuleFile {
    pub name: String,
    /// "static_library", "executable", "test"
    pub kind: String,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub public_include: Vec<String>,
    #[serde(default)]
    pub private_include: Vec<String>,
    #[serde(default)]
    pub public_deps: Vec<String>,
    #[serde(default)]
    pub private_deps: Vec<String>,
    #[serde(default)]
    pub public_defines: Vec<String>,
    #[serde(default)]
    pub defines: Vec<String>,
    #[serde(default)]
    pub cxx_flags: Vec<String>,
    #[serde(default)]
    pub c_flags: Vec<String>,
    #[serde(default)]
    pub frameworks: Vec<String>,
    #[serde(default)]
    pub link_flags: Vec<String>,
    /// Executable file name (defaults to the module name).
    #[serde(default)]
    pub output: Option<String>,
    /// A note for readers of the manifest; the tool does not print it.
    #[serde(default)]
    #[allow(dead_code)]
    pub description: String,
    /// Suppress warnings-as-errors for third-party code.
    #[serde(default)]
    pub third_party: bool,
    /// Targets this module is not built for ("wasm").
    #[serde(default)]
    pub exclude_targets: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Module {
    pub file: ModuleFile,
    pub dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Workspace {
    pub root: PathBuf,
    pub file: WorkspaceFile,
    pub modules: IndexMap<String, Module>,
    /// The dependencies as pocket.toml gives them, before the host's overrides: what another
    /// target resolves its own from.
    pub raw_dependencies: Vec<Dependency>,
}

impl Workspace {
    pub fn load(root: &Path) -> Result<Self> {
        // One spelling of the root (its case and separators as the file system has them), so paths
        // derived from it compare and strip whatever form --root or the working directory took.
        let canonical = dunce::canonicalize(root).unwrap_or(root.to_path_buf());
        let root = canonical.as_path();
        let text = std::fs::read_to_string(root.join("pocket.toml")).context("reading pocket.toml")?;
        let mut file: WorkspaceFile = toml::from_str(&text).context("parsing pocket.toml")?;
        let raw_dependencies = std::mem::take(&mut file.dependencies);
        for d in &raw_dependencies {
            file.dependencies.push(d.clone().for_host(std::env::consts::OS, std::env::consts::ARCH)?);
        }
        let mut modules = IndexMap::new();
        for dir in &file.workspace.module_dirs {
            let base = root.join(dir);
            if !base.exists() {
                continue;
            }
            for entry in walkdir::WalkDir::new(&base).sort_by_file_name().into_iter().filter_map(|e| e.ok()) {
                if entry.file_name() == "module.toml" {
                    let mtext = std::fs::read_to_string(entry.path()).with_context(|| format!("reading {}", entry.path().display()))?;
                    let mfile: ModuleFile = toml::from_str(&mtext).with_context(|| format!("parsing {}", entry.path().display()))?;
                    if !["static_library", "executable", "test"].contains(&mfile.kind.as_str()) {
                        bail!("{}: unknown module kind '{}'", entry.path().display(), mfile.kind);
                    }
                    let name = mfile.name.clone();
                    if modules.contains_key(&name) {
                        bail!("duplicate module name '{}' at {}", name, entry.path().display());
                    }
                    modules.insert(name, Module { file: mfile, dir: entry.path().parent().unwrap().to_path_buf() });
                }
            }
        }
        Ok(Workspace { root: root.to_path_buf(), file, modules, raw_dependencies })
    }

    /// The workspace as a target sees it: for the iOS Simulator its dependencies resolved again
    /// with their "ios-sim" and "ios" overrides; any other target builds on the host's.
    pub fn for_target(&self, target: &str) -> Result<Workspace> {
        // The web build's dependencies are the shared fields (or a "wasm" table), never the host's:
        // a Windows host's names its libraries `.lib`, while Emscripten builds `lib*.a`.
        let keys: Vec<String> = match target {
            "ios-sim" => vec!["ios-sim".to_string(), "ios".to_string()],
            "wasm" => vec!["wasm".to_string()],
            _ => return Ok(self.clone()),
        };
        let mut ws = self.clone();
        ws.file.dependencies.clear();
        for d in &self.raw_dependencies {
            // Frameworks and the prebuilt guard are a native host's concerns: kept as Apple's here
            // (the web build links neither frameworks nor prebuilt archives).
            ws.file.dependencies.push(d.clone().resolved(&keys, true)?);
        }
        Ok(ws)
    }

    pub fn dependency(&self, name: &str) -> Option<&Dependency> {
        self.file.dependencies.iter().find(|d| d.name == name)
    }

    pub fn config(&self, name: &str) -> Result<&ConfigSection> {
        self.file.configs.get(name).ok_or_else(|| anyhow!("unknown config '{}' (known: {})", name, self.file.configs.keys().cloned().collect::<Vec<_>>().join(", ")))
    }

    pub fn target_of(&self, config: &str) -> Result<String> {
        let t = &self.config(config)?.target;
        Ok(if t.is_empty() { "native".to_string() } else { t.clone() })
    }

    pub fn build_dir(&self, config: &str) -> PathBuf { self.root.join("build").join(config) }
    pub fn pocket_dir(&self) -> PathBuf { self.root.join(".pocket") }
    pub fn deps_dir(&self) -> PathBuf { self.pocket_dir().join("deps") }
}
