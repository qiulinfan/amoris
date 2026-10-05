//! `compile` (docs/spec/script-host.md 8; feature `transpile`): a project's TypeScript to a
//! `CompiledSet`, pure Rust, on any thread. Every module under `scripts/` is compiled and linted
//! (a broken file should not hide); those the entry does not reach are reported as
//! `script.module_unused` warnings and left out of the bundle.

mod lint;
mod sites;
mod transpile;

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use pocket_sim::ContentHash;
use serde_json::json;

pub use transpile::{ModuleOptions, Transpiled, epilogue, transpile};

use crate::error::{ErrorPhase, ScriptError};
use crate::resolve::{Resolved, resolve};
use crate::source::{Bundle, CompiledModule, CompiledSet, ScriptSource, valid_module_path};

/// The transpiler and its options: compiled modules from another version are compiled again.
pub const COMPILER: &str = concat!(
    "oxc 0.152.0; pocket-script ",
    env!("CARGO_PKG_VERSION"),
    "; v1"
);

/// The context of a module's source hash.
const SOURCE_CONTEXT: &str = "Amoris 2026-10-03 script module source v1";

/// Options of `compile`.
#[derive(Clone, Copy, Debug, Default)]
pub struct CompileOptions {
    /// The lint is off: only the check command's `module-state` negative control sets it
    /// (script-sandbox.md 3); sessions, the editor and the apply path cannot.
    pub lint_off: bool,
}

/// What `compile` gives besides the set.
#[derive(Clone, Debug, Default)]
pub struct CompileReport {
    pub warnings: Vec<ScriptError>,
}

/// Compiles `source` into the set `instantiate` takes, or every error found.
pub fn compile(
    source: &ScriptSource,
    options: &CompileOptions,
) -> Result<CompiledSet, Vec<ScriptError>> {
    compile_with_report(source, options).map(|(set, _)| set)
}

/// [`compile`], with its warnings.
pub fn compile_with_report(
    source: &ScriptSource,
    options: &CompileOptions,
) -> Result<(CompiledSet, CompileReport), Vec<ScriptError>> {
    let mut errors = Vec::new();
    let mut compiled: BTreeMap<String, (Transpiled, ContentHash)> = BTreeMap::new();
    for (path, bytes) in &source.files {
        if !path.ends_with(".ts") {
            continue;
        }
        if !valid_module_path(path) {
            errors.push(
                ScriptError::new(
                    "script.module_outside_root",
                    format!("{path} is not a module path under scripts/."),
                    ErrorPhase::Load,
                )
                .with("path", json!(path)),
            );
            continue;
        }
        let Ok(text) = std::str::from_utf8(bytes) else {
            errors.push(
                ScriptError::new(
                    "script.source_encoding",
                    format!("{path} is not UTF-8 text."),
                    ErrorPhase::Compile,
                )
                .at(path, 1, 1),
            );
            continue;
        };
        let module = ModuleOptions {
            lint_off: options.lint_off,
            prelude: false,
        };
        // A panic in oxc must not take the caller down: it becomes this module's error.
        match std::panic::catch_unwind(|| transpile(path, text, module)) {
            Ok(Ok(t)) => {
                compiled.insert(
                    path.clone(),
                    (t, ContentHash::derive(SOURCE_CONTEXT, bytes)),
                );
            }
            Ok(Err(mut e)) => errors.append(&mut e),
            Err(_) => errors.push(
                ScriptError::new(
                    "script.syntax",
                    format!("The transpiler failed on {path}."),
                    ErrorPhase::Compile,
                )
                .at(path, 1, 1),
            ),
        }
    }
    if !source.files.contains_key(&source.entry) {
        errors.push(
            ScriptError::new(
                "script.module_not_found",
                format!("The entry module {} does not exist.", source.entry),
                ErrorPhase::Load,
            )
            .with("import", json!(source.entry))
            .suggest(pocket_contract::suggest::suggest_names(
                &source.entry,
                source.files.keys().map(String::as_str),
            )),
        );
    }
    // Reachability from the entry, resolving every import (unresolvable ones are errors).
    let known: Vec<&str> = source
        .files
        .keys()
        .map(String::as_str)
        .filter(|p| p.ends_with(".ts"))
        .collect();
    let mut reached = BTreeSet::new();
    let mut queue = VecDeque::from([source.entry.clone()]);
    for (path, (t, _)) in &compiled {
        for spec in &t.imports {
            match resolve(path, spec, known.iter().copied()) {
                Err(e) => errors.push(locate(e, path, spec, &source.files[path])),
                Ok(Resolved::Host) => {}
                Ok(_) => {}
            }
        }
    }
    while let Some(path) = queue.pop_front() {
        if !reached.insert(path.clone()) {
            continue;
        }
        if let Some((t, _)) = compiled.get(&path) {
            for spec in &t.imports {
                if let Ok(Resolved::Module(m)) = resolve(&path, spec, known.iter().copied()) {
                    queue.push_back(m);
                }
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let warnings = compiled
        .keys()
        .filter(|p| !reached.contains(*p))
        .map(|p| {
            ScriptError::new(
                "script.module_unused",
                format!(
                    "{p} is not imported from the entry {}, so it does not run.",
                    source.entry
                ),
                ErrorPhase::Load,
            )
            .at(p, 1, 1)
        })
        .collect();
    let mut modules = Vec::new();
    let mut files = Vec::new();
    for (path, (t, hash)) in compiled {
        if reached.contains(&path) {
            files.push((path.clone(), source.files[&path].clone()));
            modules.push(CompiledModule {
                path,
                js: t.js,
                map: t.map,
                source_hash: hash,
                systems: t.systems,
            });
        }
    }
    Ok((
        CompiledSet {
            entry: source.entry.clone(),
            modules,
            bundle: Bundle::new(files),
            compiler: COMPILER.to_owned(),
        },
        CompileReport { warnings },
    ))
}

/// Places a resolution error at the import's line in the TypeScript.
fn locate(e: ScriptError, path: &str, spec: &str, bytes: &[u8]) -> ScriptError {
    let text = String::from_utf8_lossy(bytes);
    let needle = [format!("\"{spec}\""), format!("'{spec}'")];
    let offset = needle
        .iter()
        .filter_map(|n| text.find(n.as_str()))
        .min()
        .unwrap_or(0);
    let (line, column) = crate::error::line_col(&text, u32::try_from(offset + 1).unwrap_or(0));
    e.at(path, line, column)
}

/// Compiles the prelude's TypeScript (`src/prelude/pocket.ts`): the same pipeline, held to the
/// same lint, allowed to import `pocket:host`.
pub fn compile_prelude(text: &str) -> Result<Transpiled, Vec<ScriptError>> {
    transpile(
        crate::resolve::PRELUDE,
        text,
        ModuleOptions {
            lint_off: false,
            prelude: true,
        },
    )
}
