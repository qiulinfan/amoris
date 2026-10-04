//! `scripts.types` (docs/sdk.md; script-host.md 7.4): the SDK's declarations for the project,
//! written into `<project>/.pocket/types/`, the engine's generated (ignored) directory. `pocket.d.ts`
//! is the prelude's API; `components.d.ts` fills its `Components` and `ComponentColumns` with every
//! component scripts can use: the engine's from the live registry, and the game's own from the
//! scripts as they are on disk (the running program's when they are the running bundle, else
//! compiled and instantiated in a throwaway host, so they are current before any swap), else from
//! the registry; `tsconfig.json` beside them is what `tsc` runs with when the project has none. Only
//! an explicit `scripts.types` (`tsconfig`, the default) creates the project's own `tsconfig.json`;
//! `scripts.apply {types}`, the server's checks, `pocket check` and the editor never write into the
//! source tree. Generating is a read of the registry and at most one compile and instantiation on
//! the game thread; the type check that reads the files (`tsc`) runs in the server, never here.

use std::path::Path;
use std::sync::Arc;

use pocket_contract::{Problem, detail};
use pocket_script::types::{
    TSCONFIG, TYPES_DIR, TYPES_TSCONFIG, components_dts, pocket_dts, usable_components,
};
use pocket_sim::registry::{ComponentOrigin, ComponentSchema};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// `scripts.types`'s parameters.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScriptsTypesParams {
    /// Also return the files' text (`text: {"pocket.d.ts": ..., "components.d.ts": ...}`), as the
    /// editor loads them.
    #[serde(default)]
    pub text: bool,
    /// Create the project's `tsconfig.json` when it has none (default true); `false` writes only
    /// under `.pocket/types/`, as the checks and the editor do.
    #[serde(default = "yes")]
    pub tsconfig: bool,
}

fn yes() -> bool {
    true
}

impl Default for ScriptsTypesParams {
    fn default() -> ScriptsTypesParams {
        ScriptsTypesParams {
            text: false,
            tsconfig: true,
        }
    }
}

impl ScriptsTypesParams {
    /// What a check writes: the declarations only, nothing in the source tree.
    pub fn for_check() -> ScriptsTypesParams {
        ScriptsTypesParams {
            text: false,
            tsconfig: false,
        }
    }
}

/// Where the game's components came from.
pub enum ProjectSource {
    /// The scripts on disk, compiled and instantiated.
    Scripts,
    /// The live registry: the scripts did not load (their diagnostics say why) or this game has no
    /// project directory.
    Registry(Vec<Value>),
}

/// The declarations' text and what they hold.
pub struct Declarations {
    pub pocket: String,
    pub components: String,
    pub engine: Vec<String>,
    pub project: Vec<String>,
    pub unavailable: Vec<String>,
    pub source: ProjectSource,
}

/// The declarations for `world`, with `candidate` as the game's components when the scripts on
/// disk loaded (else the registry's).
pub fn declarations(
    world: &bevy_ecs::prelude::World,
    candidate: Result<Vec<Arc<ComponentSchema>>, Vec<Value>>,
) -> Declarations {
    let (usable, unavailable) = usable_components(world);
    let engine: Vec<Arc<ComponentSchema>> = usable
        .iter()
        .filter(|c| c.origin == ComponentOrigin::Engine)
        .cloned()
        .collect();
    let (project, source) = match candidate {
        Ok(p) => (p, ProjectSource::Scripts),
        Err(diagnostics) => (
            usable
                .iter()
                .filter(|c| c.origin == ComponentOrigin::Project)
                .cloned()
                .collect(),
            ProjectSource::Registry(diagnostics),
        ),
    };
    let all: Vec<&ComponentSchema> = engine.iter().chain(&project).map(|c| &**c).collect();
    let unavailable_names: Vec<&str> = unavailable.iter().map(String::as_str).collect();
    let names = |v: &[Arc<ComponentSchema>]| v.iter().map(|c| c.name.to_string()).collect();
    Declarations {
        pocket: pocket_dts(),
        components: components_dts(&all, &unavailable_names),
        engine: names(&engine),
        project: names(&project),
        unavailable,
        source,
    }
}

fn io_problem(path: &Path, e: &std::io::Error) -> Problem {
    Problem::new(
        "project.unwritable",
        format!("{} cannot be written: {e}.", path.display()),
        detail([("path", json!(path.display().to_string()))]),
    )
}

/// Writes `text` unless the file already holds it (editors and watchers see no change).
fn write_if_changed(path: &Path, text: &str) -> Result<bool, Problem> {
    if std::fs::read_to_string(path).is_ok_and(|t| t == text) {
        return Ok(false);
    }
    std::fs::write(path, text).map_err(|e| io_problem(path, &e))?;
    Ok(true)
}

/// Writes the declarations under `root` (and its `tsconfig.json` when it has none and `p` asks for
/// it) and answers what `scripts.types` reports.
pub fn write(
    root: Option<&Path>,
    d: &Declarations,
    p: &ScriptsTypesParams,
) -> Result<Value, Problem> {
    let mut files = Vec::new();
    let mut tsconfig = Value::Null;
    if let Some(root) = root {
        let dir = root.join(TYPES_DIR);
        std::fs::create_dir_all(&dir).map_err(|e| io_problem(&dir, &e))?;
        for (name, text) in [
            ("pocket.d.ts", d.pocket.as_str()),
            ("components.d.ts", d.components.as_str()),
            ("tsconfig.json", TYPES_TSCONFIG),
        ] {
            let changed = write_if_changed(&dir.join(name), text)?;
            files.push(
                json!({"path": format!("{TYPES_DIR}/{name}"), "bytes": text.len(),
                              "changed": changed}),
            );
        }
        let config = root.join("tsconfig.json");
        tsconfig = if config.is_file() {
            json!("kept")
        } else if p.tsconfig {
            std::fs::write(&config, TSCONFIG).map_err(|e| io_problem(&config, &e))?;
            json!("created")
        } else {
            json!("none")
        };
    }
    let (from, diagnostics) = match &d.source {
        ProjectSource::Scripts => ("scripts", Vec::new()),
        ProjectSource::Registry(diags) => ("registry", diags.clone()),
    };
    let mut out = json!({
        "dir": root.map(|_| TYPES_DIR),
        "files": files,
        "tsconfig": tsconfig,
        "components": {"engine": d.engine, "project": d.project, "unavailable": d.unavailable},
        "project_from": from,
        "diagnostics": diagnostics,
    });
    if p.text {
        out["text"] = json!({"pocket.d.ts": d.pocket, "components.d.ts": d.components});
    }
    Ok(out)
}
