//! A project on disk: `project.toml` (its name, tick rate and default seed), `scene.json` (the
//! entities it starts with) and `scripts/` (its TypeScript rules), loaded into the data a [`Game`]
//! is built from. The web build receives the same data compiled, without the file system.
//!
//! [`Game`]: crate::Game

use std::path::{Path, PathBuf};

use pocket_contract::{Problem, detail};
use pocket_script::{CompiledSet, ScriptLimits, ScriptSource};
use pocket_sim::TickRate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::scene::Scene;

/// `project.toml`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub name: String,
    /// Ticks per second, 1 to 1000.
    #[serde(default = "default_rate")]
    pub rate: u32,
    /// The seed `pocket run` uses when none is given.
    #[serde(default = "default_seed")]
    pub seed: u64,
    /// Sounds the presenters play when an event is emitted: event name (or `prefix.*`) to clip
    /// (`sounds/ding.wav`, or a synthesized `sfx:coin`). Presentation only; never in the tick.
    #[serde(default)]
    pub sounds: std::collections::BTreeMap<String, String>,
}

fn default_rate() -> u32 {
    TickRate::DEFAULT.0
}

fn default_seed() -> u64 {
    1
}

/// What a game is built from: the project as data.
#[derive(Clone, Debug)]
pub struct GameSetup {
    pub name: String,
    pub rate: TickRate,
    pub scene: Scene,
    /// The scripts the game starts with.
    pub scripts: CompiledSet,
    /// Their TypeScript, for a reload that compiles them again (none on the web).
    pub source: Option<ScriptSource>,
    pub limits: ScriptLimits,
    /// Compile with the lint off: the negative controls only (script-sandbox.md 3).
    pub lint_off: bool,
}

/// A project read from disk.
#[derive(Clone, Debug)]
pub struct Project {
    pub root: PathBuf,
    pub manifest: Manifest,
    pub scene: Scene,
    pub source: ScriptSource,
}

/// `project.missing_file {path}` or `project.unreadable {path, error}`.
pub fn read_file(path: &Path) -> Result<String, Problem> {
    std::fs::read_to_string(path).map_err(|e| {
        let code = if e.kind() == std::io::ErrorKind::NotFound {
            "project.missing_file"
        } else {
            "project.unreadable"
        };
        Problem::new(
            code,
            format!("{} cannot be read: {e}.", path.display()),
            detail([
                ("path", json!(path.display().to_string())),
                ("error", json!(e.to_string())),
            ]),
        )
    })
}

/// A TOML file as a JSON value, for the strict decoder (`project.invalid` when it does not
/// parse).
pub fn toml_value(path: &Path, text: &str) -> Result<serde_json::Value, Problem> {
    toml::from_str::<serde_json::Value>(text).map_err(|e| {
        Problem::new(
            "project.invalid",
            format!("{} is not valid TOML: {}", path.display(), e.message()),
            detail([
                ("path", json!(path.display().to_string())),
                ("message", json!(e.message())),
            ]),
        )
    })
}

impl Project {
    /// Reads `root`'s manifest, scene and scripts.
    pub fn load(root: &Path) -> Result<Project, Problem> {
        let manifest_path = root.join("project.toml");
        let text = read_file(&manifest_path)?;
        let manifest: Manifest =
            crate::decode(&toml_value(&manifest_path, &text)?, "project.toml")?;
        TickRate::new(manifest.rate)?;
        let scene = Scene::from_json(&read_file(&root.join("scene.json"))?)?;
        let source = ScriptSource::read_dir(root).map_err(|e| {
            Problem::new(
                "project.unreadable",
                format!("{}/scripts cannot be read: {e}.", root.display()),
                detail([("path", json!(root.join("scripts").display().to_string()))]),
            )
        })?;
        Ok(Project {
            root: root.to_path_buf(),
            manifest,
            scene,
            source,
        })
    }

    /// The game's data: the scripts compiled, with the lint on unless `lint_off`.
    #[cfg(feature = "transpile")]
    pub fn setup(&self, lint_off: bool) -> Result<GameSetup, Problem> {
        Ok(GameSetup {
            name: self.manifest.name.clone(),
            rate: TickRate::new(self.manifest.rate)?,
            scene: self.scene.clone(),
            scripts: crate::scripts::compile(&self.source, lint_off)?,
            source: Some(self.source.clone()),
            limits: ScriptLimits::default(),
            lint_off,
        })
    }
}

/// Lints and compiles the scripts under `root` with the lint on (checks.md 6.4): the number of
/// modules, or every finding as its problem (`lint.*`, `script.*`).
#[cfg(feature = "transpile")]
pub fn lint(root: &Path) -> Result<usize, Vec<Problem>> {
    let source = ScriptSource::read_dir(root).map_err(|e| {
        vec![Problem::new(
            "project.unreadable",
            format!("{}/scripts cannot be read: {e}.", root.display()),
            detail([("path", json!(root.join("scripts").display().to_string()))]),
        )]
    })?;
    let modules = source.files.keys().filter(|p| p.ends_with(".ts")).count();
    pocket_script::compile(&source, &pocket_script::CompileOptions::default())
        .map(|_| modules)
        .map_err(|errors| {
            errors
                .iter()
                .map(pocket_script::ScriptError::to_problem)
                .collect()
        })
}
