//! The scripts a debugger shows (docs/spec/debugger.md 4). QuickJS-ng runs each module's
//! JavaScript under its module path (`scripts/rules.ts`); a CDP client sees that JavaScript as
//! `pocket:///scripts/rules.js` with a source map (a data URL) whose source is `rules.ts` beside it,
//! that is `pocket:///scripts/rules.ts`, with the TypeScript text embedded, so Chrome DevTools and
//! VS Code show and break in the TypeScript. Agents speak TypeScript positions directly; this module
//! maps both ways.
//!
//! Every position here is 0-based, as CDP counts; QuickJS-ng's are 1-based and are converted by
//! the caller.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use pocket_script::sourcemap::LineMap;
use pocket_script::{CompiledModule, CompiledSet};
use serde_json::{Value, json};

/// The URL scheme of the scripts CDP clients see.
pub const SCHEME: &str = "pocket:///";

/// How far a breakpoint on a line without code moves down to the next line with code, as V8 moves
/// one set on a blank line or a comment.
const SNAP_LINES: u32 = 40;

/// One module as the debugger shows it.
pub struct Script {
    /// The CDP `scriptId`, stable for a module whose JavaScript does not change.
    pub id: String,
    /// The module path (`scripts/rules.ts`): QuickJS-ng's module name and the agents' `file`.
    pub module: String,
    /// `pocket:///scripts/rules.js`.
    pub url: String,
    pub js: String,
    /// The TypeScript the module came from.
    pub ts: String,
    /// The source map as a `data:` URL (sources relative to `url`, TypeScript embedded).
    pub source_map_url: String,
    /// A hash of the JavaScript (CDP `hash`).
    pub hash: String,
    map: LineMap,
    /// TypeScript line -> the generated positions mapped from it, ascending.
    ts_to_js: BTreeMap<u32, Vec<(u32, u32)>>,
    /// Generated lines holding at least one mapping.
    js_code: BTreeSet<u32>,
    /// Per generated line with code, the first mapped column.
    js_first_col: BTreeMap<u32, u32>,
    js_lines: u32,
    js_last_line_len: u32,
}

impl std::fmt::Debug for Script {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Script")
            .field("id", &self.id)
            .field("module", &self.module)
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl Script {
    fn new(id: String, m: &CompiledModule, ts: &str, root: &SourceRoot) -> Script {
        let map = LineMap::parse(&m.map);
        let mut ts_to_js: BTreeMap<u32, Vec<(u32, u32)>> = BTreeMap::new();
        let mut js_code = BTreeSet::new();
        let mut js_first_col: BTreeMap<u32, u32> = BTreeMap::new();
        for (gl, gc, sl, _) in map.segments() {
            ts_to_js.entry(sl).or_default().push((gl, gc));
            js_code.insert(gl);
            let first = js_first_col.entry(gl).or_insert(gc);
            *first = (*first).min(gc);
        }
        for v in ts_to_js.values_mut() {
            v.sort_unstable();
            v.dedup();
        }
        let url = js_url(&m.path);
        let lines: Vec<&str> = m.js.split('\n').collect();
        Script {
            id,
            source_map_url: source_map_url(m, ts, root),
            hash: format!("{:016x}", fnv64(m.js.as_bytes())),
            module: m.path.clone(),
            url,
            js: m.js.clone(),
            ts: ts.to_owned(),
            map,
            ts_to_js,
            js_code,
            js_first_col,
            js_lines: u32::try_from(lines.len()).unwrap_or(u32::MAX),
            js_last_line_len: lines
                .last()
                .map_or(0, |l| u32::try_from(l.len()).unwrap_or(0)),
        }
    }

    /// The TypeScript position of a generated one, or `None` for generated code no TypeScript wrote
    /// (the harden epilogue).
    pub fn to_ts(&self, js_line: u32, js_col: u32) -> Option<(u32, u32)> {
        let (l, c) = self.map.lookup(js_line + 1, js_col + 1)?;
        Some((l - 1, c - 1))
    }

    /// Whether a generated line holds code.
    pub fn js_has_code(&self, js_line: u32) -> bool {
        self.js_code.contains(&js_line)
    }

    /// The first generated line at or after `js_line` holding code (within a few lines).
    pub fn snap_js(&self, js_line: u32) -> Option<u32> {
        self.js_code
            .range(js_line..js_line.saturating_add(SNAP_LINES))
            .next()
            .copied()
    }

    /// The first TypeScript line at or after `ts_line` holding code, and the first generated
    /// position mapped from it.
    pub fn snap_ts(&self, ts_line: u32) -> Option<(u32, (u32, u32))> {
        self.ts_to_js
            .range(ts_line..ts_line.saturating_add(SNAP_LINES))
            .find_map(|(l, v)| v.first().map(|p| (*l, *p)))
    }

    /// The generated lines mapped from a TypeScript line.
    pub fn js_lines_of(&self, ts_line: u32) -> Vec<u32> {
        let mut out: Vec<u32> = self
            .ts_to_js
            .get(&ts_line)
            .map(|v| v.iter().map(|p| p.0).collect())
            .unwrap_or_default();
        out.dedup();
        out
    }

    /// TypeScript lines holding code in `from..=to`.
    pub fn ts_code_lines(&self, from: u32, to: u32) -> Vec<u32> {
        self.ts_to_js.range(from..=to).map(|(l, _)| *l).collect()
    }

    /// Generated positions where statements of lines `from..=to` start (the first mapped column of
    /// each line with code).
    pub fn js_code_positions(&self, from: u32, to: u32) -> Vec<(u32, u32)> {
        self.js_first_col
            .range(from..=to)
            .map(|(l, c)| (*l, *c))
            .collect()
    }

    /// The first mapped column of a generated line (0 without code).
    pub fn js_first_col(&self, js_line: u32) -> u32 {
        self.js_first_col.get(&js_line).copied().unwrap_or(0)
    }

    /// `Debugger.scriptParsed`'s parameters.
    pub fn script_parsed(&self) -> Value {
        json!({
            "scriptId": self.id,
            "url": self.url,
            "startLine": 0,
            "startColumn": 0,
            "endLine": self.js_lines.saturating_sub(1),
            "endColumn": self.js_last_line_len,
            "executionContextId": 1,
            "hash": self.hash,
            "executionContextAuxData": {"isDefault": true},
            "isLiveEdit": false,
            "sourceMapURL": self.source_map_url,
            "hasSourceURL": false,
            "isModule": true,
            "length": self.js.len(),
            "scriptLanguage": "JavaScript",
            "embedderName": self.url,
        })
    }
}

/// Where the TypeScript sources of the source maps are.
#[derive(Clone, Debug, Default)]
pub enum SourceRoot {
    /// Beside the script: `pocket:///scripts/rules.ts` (clients map it to the project with a path
    /// override, editors/vscode/launch.json).
    #[default]
    Relative,
    /// Absolute `file://` URLs under a project directory, so an editor opens the files on disk
    /// without configuration.
    Project(std::path::PathBuf),
}

/// `scripts/rules.ts` -> `pocket:///scripts/rules.js`.
pub fn js_url(module: &str) -> String {
    let stem = module.strip_suffix(".ts").unwrap_or(module);
    format!("{SCHEME}{stem}.js")
}

fn file_url(root: &std::path::Path, module: &str) -> String {
    let abs = root.join(module);
    let s = abs.to_string_lossy().replace('\\', "/").replace(' ', "%20");
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
    }
}

fn source_map_url(m: &CompiledModule, ts: &str, root: &SourceRoot) -> String {
    let mut map: Value = serde_json::from_str(&m.map).unwrap_or_else(|_| json!({}));
    let base = m.path.rsplit('/').next().unwrap_or(&m.path);
    let source = match root {
        SourceRoot::Relative => base.to_owned(),
        SourceRoot::Project(dir) => file_url(dir, &m.path),
    };
    map["version"] = json!(3);
    map["file"] = json!(js_url(base).trim_start_matches(SCHEME));
    map["sourceRoot"] = json!("");
    map["sources"] = json!([source]);
    map["sourcesContent"] = json!([ts]);
    format!(
        "data:application/json;charset=utf-8;base64,{}",
        base64(map.to_string().as_bytes())
    )
}

/// Standard base64 with padding.
pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let idx = |shift: u32| char::from(T[((n >> shift) & 63) as usize]);
        out.push(idx(18));
        out.push(idx(12));
        out.push(if chunk.len() > 1 { idx(6) } else { '=' });
        out.push(if chunk.len() > 2 { idx(0) } else { '=' });
    }
    out
}

fn fnv64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Every script shown since the debugger attached: those of the running program, and those an
/// earlier program ran (a client may still hold their ids).
#[derive(Default)]
pub struct Registry {
    all: Vec<Arc<Script>>,
    /// The running program's scripts, by module path.
    current: BTreeMap<String, Arc<Script>>,
    root: SourceRoot,
}

impl Registry {
    pub fn new(root: SourceRoot) -> Registry {
        Registry {
            root,
            ..Registry::default()
        }
    }

    /// Takes a program's modules: a module whose JavaScript is unchanged keeps its script; the
    /// others get new ids. Returns the scripts not shown before.
    pub fn load(&mut self, set: &CompiledSet) -> Vec<Arc<Script>> {
        let sources: BTreeMap<&str, &[u8]> = set
            .bundle
            .files
            .iter()
            .map(|(p, s)| (p.as_str(), s.as_slice()))
            .collect();
        let mut fresh = Vec::new();
        let mut current = BTreeMap::new();
        for m in &set.modules {
            let hash = format!("{:016x}", fnv64(m.js.as_bytes()));
            let known = self
                .all
                .iter()
                .rev()
                .find(|s| s.module == m.path && s.hash == hash)
                .cloned();
            let script = match known {
                Some(s) => s,
                None => {
                    let ts = sources
                        .get(m.path.as_str())
                        .map(|b| String::from_utf8_lossy(b).into_owned())
                        .unwrap_or_default();
                    let s = Arc::new(Script::new(
                        (self.all.len() + 1).to_string(),
                        m,
                        &ts,
                        &self.root,
                    ));
                    self.all.push(s.clone());
                    fresh.push(s.clone());
                    s
                }
            };
            current.insert(m.path.clone(), script);
        }
        self.current = current;
        fresh
    }

    /// The running program's script of a module.
    pub fn module(&self, module: &str) -> Option<&Arc<Script>> {
        self.current.get(module)
    }

    /// Any script shown, by id.
    pub fn by_id(&self, id: &str) -> Option<&Arc<Script>> {
        self.all.iter().find(|s| s.id == id)
    }

    /// The running program's scripts, by module path.
    pub fn current(&self) -> impl Iterator<Item = &Arc<Script>> {
        self.current.values()
    }

    /// The running program's module paths.
    pub fn modules(&self) -> Vec<String> {
        self.current.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_pads() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn urls() {
        assert_eq!(js_url("scripts/rules.ts"), "pocket:///scripts/rules.js");
        assert_eq!(
            file_url(std::path::Path::new("/a b/p"), "scripts/x.ts"),
            "file:///a%20b/p/scripts/x.ts"
        );
    }
}
