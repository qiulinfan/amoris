//! The runtime's side of the scripts (docs/spec/hot-update.md 4 and 5; versions.md 7.4): compiling
//! a project's TypeScript, the bundle as a replay carries it, a compiled set rebuilt from the
//! modules a replay embeds, and `scripts.apply`'s parameters and report.

use std::collections::BTreeMap;

use pocket_contract::{Problem, detail};
use pocket_persist::EngineVersion;
use pocket_persist::replay::{BundleRecord, CompiledModuleRecord, CompiledRecord};
use pocket_script::{Bundle, CompiledModule, CompiledSet, ScriptError};
use pocket_sim::ContentHash;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The entry module of every slice 1 project (script-host.md 4.2's default).
pub const ENTRY: &str = pocket_script::source::DEFAULT_ENTRY;

/// `scripts.refused {stage, errors}` (hot-update.md 7).
pub fn refused(stage: &str, errors: &[ScriptError]) -> Problem {
    let list: Vec<Value> = errors
        .iter()
        .map(|e| serde_json::to_value(e.to_problem()).unwrap_or(Value::Null))
        .collect();
    let first = errors
        .first()
        .map_or_else(|| "no detail".to_owned(), |e| e.to_problem().message);
    Problem::new(
        "scripts.refused",
        format!("The scripts were not loaded ({stage}): {first}"),
        detail([("stage", json!(stage)), ("errors", Value::Array(list))]),
    )
}

/// Compiles TypeScript sources; the lint is on unless `lint_off` (the negative controls only,
/// script-sandbox.md 3).
#[cfg(feature = "transpile")]
pub fn compile(
    source: &pocket_script::ScriptSource,
    lint_off: bool,
) -> Result<CompiledSet, Problem> {
    let options = pocket_script::CompileOptions { lint_off };
    pocket_script::compile(source, &options).map_err(|e| refused("compile", &e))
}

/// The bundle as a replay records it: the TypeScript sources and the compiled modules beside them
/// (versions.md 7.4), so a build without the transpiler replays it.
pub fn bundle_record(set: &CompiledSet) -> BundleRecord {
    BundleRecord::new(
        set.bundle.files.clone(),
        Some(CompiledRecord {
            engine_source: EngineVersion::current().source,
            modules: set
                .modules
                .iter()
                .map(|m| CompiledModuleRecord {
                    path: m.path.clone(),
                    js: m.js.clone(),
                    map: m.map.clone(),
                })
                .collect(),
        }),
    )
}

/// A compiled set from a replay's bundle (replay.md 2.4): its embedded modules when the engine that
/// compiled them has this engine's source (`EngineVersion.source`), which run exactly as they did
/// when recorded; otherwise its TypeScript compiled again, which needs `transpile`, and without it
/// `replay.bundle_unavailable {hash, reason: "needs transpile"}`. Embedded modules carry no
/// systems' source sites (used only to locate errors), so those stay empty.
pub fn compiled_from_record(b: &BundleRecord) -> Result<CompiledSet, Problem> {
    let bundle = Bundle::new(b.files.clone());
    if bundle.hash != b.hash {
        return Err(pocket_persist::error::bundle_unavailable(
            &b.hash.to_hex(),
            "its sources do not give its hash",
        ));
    }
    match &b.compiled {
        Some(c) if c.engine_source == EngineVersion::current().source => Ok(embedded(b, c, bundle)),
        _ => recompiled(b),
    }
}

/// The record's TypeScript compiled by this engine. The bundle ran when it was recorded, so the
/// stateless lint, which decides what may be applied, is not asked again.
#[cfg(feature = "transpile")]
fn recompiled(b: &BundleRecord) -> Result<CompiledSet, Problem> {
    let source = pocket_script::ScriptSource {
        files: b.files.iter().cloned().collect(),
        entry: ENTRY.to_owned(),
    };
    let set = compile(&source, true)?;
    if set.bundle.hash != b.hash {
        return Err(pocket_persist::error::bundle_unavailable(
            &b.hash.to_hex(),
            "its sources compile to another bundle",
        ));
    }
    Ok(set)
}

#[cfg(not(feature = "transpile"))]
fn recompiled(b: &BundleRecord) -> Result<CompiledSet, Problem> {
    Err(pocket_persist::error::bundle_unavailable(
        &b.hash.to_hex(),
        "needs transpile",
    ))
}

fn embedded(b: &BundleRecord, compiled: &CompiledRecord, bundle: Bundle) -> CompiledSet {
    let sources: BTreeMap<&str, &[u8]> = b
        .files
        .iter()
        .map(|(p, s)| (p.as_str(), s.as_slice()))
        .collect();
    let mut modules: Vec<CompiledModule> = compiled
        .modules
        .iter()
        .map(|m| CompiledModule {
            path: m.path.clone(),
            js: m.js.clone(),
            map: m.map.clone(),
            source_hash: ContentHash::derive(
                pocket_script::source::BUNDLE_CONTEXT,
                sources.get(m.path.as_str()).copied().unwrap_or_default(),
            ),
            systems: Vec::new(),
        })
        .collect();
    modules.sort_by(|a, b| a.path.cmp(&b.path));
    CompiledSet {
        entry: ENTRY.to_owned(),
        modules,
        bundle,
        compiler: "embedded in a replay".to_owned(),
    }
}

/// `scripts.apply`'s parameters (hot-update.md 4.1, the slice 1 subset).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScriptsApplyParams {
    /// Module path to TypeScript text; default: the project's own scripts.
    #[serde(default)]
    pub files: Option<BTreeMap<String, String>>,
    /// Swap even when the bundle hash is unchanged (a reload, hot-update.md 9).
    #[serde(default)]
    pub force: bool,
    /// Stop before the swap.
    #[serde(default)]
    pub dry_run: bool,
    /// Also write the SDK's declarations of what was compiled (`scripts.types`, no
    /// `tsconfig.json`) and answer them as `types`; a dry run then loads the scripts too and is
    /// refused when they would not load. The server sets it for its type check.
    #[serde(default)]
    pub types: bool,
}

/// `scripts.swap`'s parameters: a Host write naming the prepared bundle by hash.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScriptsSwapParams {
    /// The bundle's hash, 64 hex digits.
    pub bundle: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> BundleRecord {
        let source = pocket_script::ScriptSource::new().with(
            ENTRY,
            "export function speed(x: number): number { return x * 2; }\n",
        );
        let options = pocket_script::CompileOptions { lint_off: true };
        let set = pocket_script::compile(&source, &options).unwrap_or_else(|e| panic!("{e:?}"));
        bundle_record(&set)
    }

    /// replay.md 2.4: modules this engine's source compiled are reused as they were recorded.
    #[test]
    fn modules_of_this_engine_are_reused() {
        let r = record();
        let set = compiled_from_record(&r).unwrap();
        assert_eq!(set.compiler, "embedded in a replay");
        assert_eq!(set.bundle.hash, r.hash);
    }

    /// Modules another engine's source compiled are not run: the TypeScript is compiled again, or
    /// the bundle is unavailable in a build without the transpiler (the web worker).
    #[test]
    fn modules_of_another_engine_are_not_reused() {
        let mut r = record();
        if let Some(c) = r.compiled.as_mut() {
            c.engine_source = ContentHash([7; 32]);
        }
        let got = compiled_from_record(&r);
        #[cfg(feature = "transpile")]
        {
            let set = got.unwrap();
            assert_ne!(set.compiler, "embedded in a replay");
            assert_eq!(set.bundle.hash, r.hash);
        }
        #[cfg(not(feature = "transpile"))]
        {
            let e = got.unwrap_err();
            assert_eq!(e.code, "replay.bundle_unavailable");
            assert_eq!(e.detail["reason"], json!("needs transpile"));
        }
    }
}
