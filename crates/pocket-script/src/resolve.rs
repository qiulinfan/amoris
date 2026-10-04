//! Module resolution (docs/spec/script-host.md 8.1), shared by `compile` (which computes the
//! reachable set and reports unresolvable imports) and the loader (which finds modules in a
//! `CompiledSet`). Nothing touches the file system.

use serde_json::json;

use crate::error::{ErrorPhase, ScriptError};
use crate::source::SCRIPTS_ROOT;

/// The prelude's module name.
pub const PRELUDE: &str = "pocket";
/// The native module's name (the prelude and the harden epilogue only).
pub const HOST: &str = "pocket:host";

/// What a specifier names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolved {
    Prelude,
    Host,
    Module(String),
}

/// `dir/name` with `.` and `..` resolved; `None` if it climbs above the project root.
fn join(dir: &str, name: &str) -> Option<String> {
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    for p in name.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(p),
        }
    }
    Some(parts.join("/"))
}

fn failed(code: &str, message: String, importer: &str, specifier: &str) -> ScriptError {
    ScriptError::new(code, message, ErrorPhase::Load)
        .with("importer", json!(importer))
        .with("import", json!(specifier))
}

/// Resolves `specifier` imported by `importer` (a module path, or the prelude's name) among the
/// module paths `known`.
pub fn resolve<'a>(
    importer: &str,
    specifier: &str,
    known: impl Iterator<Item = &'a str> + Clone,
) -> Result<Resolved, ScriptError> {
    if specifier == PRELUDE {
        return Ok(Resolved::Prelude);
    }
    if specifier == HOST {
        return Ok(Resolved::Host);
    }
    let relative = specifier.starts_with("./") || specifier.starts_with("../");
    if !relative {
        let hint = "npm packages and Node modules are not available in game scripts: import \
                    \"pocket\" or a ./relative path";
        return Err(failed(
            "script.module_not_found",
            format!("'{specifier}' is not a module of this project."),
            importer,
            specifier,
        )
        .hint(hint));
    }
    let dir = importer.rsplit_once('/').map_or("", |(d, _)| d);
    let joined = join(dir, specifier).unwrap_or_default();
    if !joined.starts_with(SCRIPTS_ROOT) {
        return Err(failed(
            "script.module_outside_root",
            format!("'{specifier}' from {importer} leaves the scripts/ directory."),
            importer,
            specifier,
        ));
    }
    let last = joined.rsplit('/').next().unwrap_or("");
    if let Some((_, ext)) = last.rsplit_once('.')
        && ext != "ts"
        && matches!(
            ext,
            "tsx" | "js" | "mjs" | "cjs" | "jsx" | "json" | "mts" | "cts"
        )
    {
        return Err(failed(
            "script.module_extension",
            format!("'{specifier}' is a .{ext} module; only .ts modules load."),
            importer,
            specifier,
        ));
    }
    let candidates = if joined.ends_with(".ts") {
        vec![joined.clone()]
    } else {
        vec![format!("{joined}.ts"), format!("{joined}/index.ts")]
    };
    for c in &candidates {
        if known.clone().any(|k| k == c) {
            return Ok(Resolved::Module(c.clone()));
        }
    }
    for c in &candidates {
        if let Some(k) = known.clone().find(|k| k.eq_ignore_ascii_case(c)) {
            return Err(failed(
                "script.module_case",
                format!("'{specifier}' differs only in case from the module {k}."),
                importer,
                specifier,
            )
            .with("existing", json!(k)));
        }
    }
    let suggestions = pocket_contract::suggest::suggest_names(&candidates[0], known);
    Err(failed(
        "script.module_not_found",
        format!("{importer} imports '{specifier}', which is not a module of this project."),
        importer,
        specifier,
    )
    .suggest(suggestions))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KNOWN: [&str; 3] = [
        "scripts/main.ts",
        "scripts/rules/damage.ts",
        "scripts/lib/index.ts",
    ];

    fn r(importer: &str, spec: &str) -> Result<Resolved, String> {
        resolve(importer, spec, KNOWN.iter().copied()).map_err(|e| e.code)
    }

    #[test]
    fn specifiers() {
        let m = |p: &str| Ok(Resolved::Module(p.to_owned()));
        assert_eq!(r("scripts/main.ts", "pocket"), Ok(Resolved::Prelude));
        assert_eq!(
            r("scripts/main.ts", "./rules/damage"),
            m("scripts/rules/damage.ts")
        );
        assert_eq!(
            r("scripts/main.ts", "./rules/damage.ts"),
            m("scripts/rules/damage.ts")
        );
        assert_eq!(
            r("scripts/rules/damage.ts", "../lib"),
            m("scripts/lib/index.ts")
        );
        assert_eq!(
            r("scripts/main.ts", "./Rules/Damage"),
            Err("script.module_case".into())
        );
        assert_eq!(
            r("scripts/main.ts", "./rules/dmg"),
            Err("script.module_not_found".into())
        );
        assert_eq!(
            r("scripts/main.ts", "lodash"),
            Err("script.module_not_found".into())
        );
        assert_eq!(
            r("scripts/main.ts", "node:fs"),
            Err("script.module_not_found".into())
        );
        assert_eq!(
            r("scripts/main.ts", "../x"),
            Err("script.module_outside_root".into())
        );
        assert_eq!(
            r("scripts/main.ts", "./data.json"),
            Err("script.module_extension".into())
        );
        assert_eq!(
            r("scripts/main.ts", "./ui.tsx"),
            Err("script.module_extension".into())
        );
    }
}
