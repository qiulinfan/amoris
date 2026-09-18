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
    pub name: String,
    #[serde(default)]
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
    /// "prebuilt" (archive with headers and libs), "cmake" (source archive built with CMake), "system" (frameworks/libs already present).
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
    #[serde(default)]
    pub license: String,
}
fn one() -> u32 { 1 }

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
    /// Extra description shown by `pocket graph`.
    #[serde(default)]
    pub description: String,
    /// Suppress warnings-as-errors for third-party code.
    #[serde(default)]
    pub third_party: bool,
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
}

impl Workspace {
    pub fn load(root: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(root.join("pocket.toml")).context("reading pocket.toml")?;
        let file: WorkspaceFile = toml::from_str(&text).context("parsing pocket.toml")?;
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
        Ok(Workspace { root: root.to_path_buf(), file, modules })
    }

    pub fn dependency(&self, name: &str) -> Option<&Dependency> {
        self.file.dependencies.iter().find(|d| d.name == name)
    }

    pub fn config(&self, name: &str) -> Result<&ConfigSection> {
        self.file.configs.get(name).ok_or_else(|| anyhow!("unknown config '{}' (known: {})", name, self.file.configs.keys().cloned().collect::<Vec<_>>().join(", ")))
    }

    pub fn build_dir(&self, config: &str) -> PathBuf { self.root.join("build").join(config) }
    pub fn pocket_dir(&self) -> PathBuf { self.root.join(".pocket") }
    pub fn deps_dir(&self) -> PathBuf { self.pocket_dir().join("deps") }
}
