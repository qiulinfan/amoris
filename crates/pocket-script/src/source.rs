//! What scripts arrive as (docs/spec/script-host.md 4.1, 8.1): the TypeScript sources keyed by
//! module path, the compiled modules a host instantiates, and the bundle that names them.

use std::collections::BTreeMap;

use pocket_sim::ContentHash;
use serde::{Deserialize, Serialize};

use crate::error::{ErrorPhase, ScriptError};

/// The context string of the bundle hash (versions.md 3.2).
pub const BUNDLE_CONTEXT: &str = "Pocket3D 2026-10-03 script bundle v1";

/// The entry module unless the project manifest names another.
pub const DEFAULT_ENTRY: &str = "scripts/main.ts";

/// The directory every project module lives under.
pub const SCRIPTS_ROOT: &str = "scripts/";

/// Whether `path` is a module path: relative to the project root, `/`-separated, normalized (no
/// empty, `.` or `..` segment), under `scripts/`, case kept.
pub fn valid_module_path(path: &str) -> bool {
    path.starts_with(SCRIPTS_ROOT)
        && !path.contains('\\')
        && path
            .split('/')
            .all(|s| !s.is_empty() && s != "." && s != "..")
}

/// A project's scripts as bytes, keyed by module path, and the entry module. Whoever submits
/// scripts builds the whole set; the host never reads the file system.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptSource {
    pub files: BTreeMap<String, Vec<u8>>,
    pub entry: String,
}

impl ScriptSource {
    /// A source with the default entry.
    pub fn new() -> ScriptSource {
        ScriptSource {
            files: BTreeMap::new(),
            entry: DEFAULT_ENTRY.to_owned(),
        }
    }

    /// Adds a module (a builder for tests and tools).
    pub fn with(mut self, path: &str, text: &str) -> ScriptSource {
        self.files.insert(path.to_owned(), text.as_bytes().to_vec());
        self
    }

    /// Reads every file under `<root>/scripts/` (any extension, so the loader can refuse the
    /// wrong ones by name), keyed by its path relative to `root`.
    pub fn read_dir(root: &std::path::Path) -> std::io::Result<ScriptSource> {
        fn walk(
            dir: &std::path::Path,
            base: &std::path::Path,
            out: &mut ScriptSource,
        ) -> std::io::Result<()> {
            let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
            entries.sort_by_key(std::fs::DirEntry::file_name);
            for e in entries {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, base, out)?;
                } else if let Ok(rel) = p.strip_prefix(base) {
                    let key = rel
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .join("/");
                    out.files.insert(key, std::fs::read(&p)?);
                }
            }
            Ok(())
        }
        let mut out = ScriptSource::new();
        walk(&root.join("scripts"), root, &mut out)?;
        Ok(out)
    }
}

/// Where a system's `run` is written (from `system({ name: "...", run })` with a literal name).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemSite {
    pub name: String,
    pub line: u32,
    pub column: u32,
}

/// One module after transpiling: its JavaScript with the harden epilogue, its source map (JSON,
/// version 3), the hash of the TypeScript it came from, and where its systems' `run`s are.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledModule {
    pub path: String,
    pub js: String,
    pub map: String,
    pub source_hash: ContentHash,
    #[serde(default)]
    pub systems: Vec<SystemSite>,
}

/// A bundle (versions.md 3.2): the reachable TypeScript modules as written, and their hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bundle {
    pub hash: ContentHash,
    pub files: Vec<(String, Vec<u8>)>,
}

impl Bundle {
    /// The bundle of `files` (any order; hashed in path order).
    pub fn new(mut files: Vec<(String, Vec<u8>)>) -> Bundle {
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let mut h = ContentHash::hasher(BUNDLE_CONTEXT);
        for (path, source) in &files {
            h.update(&uleb128(path.len()));
            h.update(path.as_bytes());
            h.update(&uleb128(source.len()));
            h.update(source);
        }
        Bundle {
            hash: h.finish(),
            files,
        }
    }
}

/// What `compile` produces and `instantiate` takes: the reachable modules, compiled, in path
/// order; the entry; the bundle of their sources; and the compiler that made them. A build
/// without the transpiler (the shipped web build) runs one sent to it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledSet {
    pub entry: String,
    pub modules: Vec<CompiledModule>,
    pub bundle: Bundle,
    /// The transpiler and its options' version: a cached module is reused only under the same.
    pub compiler: String,
}

impl CompiledSet {
    /// The module at `path`.
    pub fn module(&self, path: &str) -> Option<&CompiledModule> {
        self.modules
            .binary_search_by(|m| m.path.as_str().cmp(path))
            .ok()
            .map(|i| &self.modules[i])
    }

    /// The set as JSON, the form the web worker and replays carry.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// A set from JSON (`script.compiled_invalid` when it does not decode or is inconsistent:
    /// modules out of order, or the bundle's hash not its files').
    pub fn from_json(text: &str) -> Result<CompiledSet, ScriptError> {
        let set: CompiledSet = serde_json::from_str(text).map_err(|e| {
            ScriptError::new(
                "script.compiled_invalid",
                format!("The compiled scripts do not decode: {e}."),
                ErrorPhase::Load,
            )
        })?;
        let ordered = set.modules.windows(2).all(|w| w[0].path < w[1].path);
        if !ordered || Bundle::new(set.bundle.files.clone()).hash != set.bundle.hash {
            return Err(ScriptError::new(
                "script.compiled_invalid",
                "The compiled scripts are inconsistent: modules out of order or a bundle hash \
                 that is not its files'.",
                ErrorPhase::Load,
            ));
        }
        Ok(set)
    }
}

/// Unsigned LEB128.
pub fn uleb128(mut n: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(4);
    loop {
        let byte = u8::try_from(n & 0x7f).unwrap_or(0);
        n >>= 7;
        if n == 0 {
            out.push(byte);
            return out;
        }
        out.push(byte | 0x80);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert!(valid_module_path("scripts/main.ts"));
        assert!(valid_module_path("scripts/rules/a.ts"));
        for bad in [
            "main.ts",
            "scripts//a.ts",
            "scripts/./a.ts",
            "scripts/../a.ts",
            "scripts\\a.ts",
        ] {
            assert!(!valid_module_path(bad), "{bad}");
        }
    }

    #[test]
    fn bundle_hash_is_order_free_and_pinned() {
        let a = Bundle::new(vec![
            ("scripts/b.ts".into(), b"b".to_vec()),
            ("scripts/a.ts".into(), b"a".to_vec()),
        ]);
        let b = Bundle::new(vec![
            ("scripts/a.ts".into(), b"a".to_vec()),
            ("scripts/b.ts".into(), b"b".to_vec()),
        ]);
        assert_eq!(a, b);
        assert_eq!(uleb128(300), vec![0xac, 0x02]);
        assert_eq!(uleb128(5), vec![5]);
    }
}
