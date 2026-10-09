//! The web package (docs/spec/threads.md 7.2, `init`'s `project`): a project as the shipped module
//! receives it, its TypeScript already compiled natively, since the shipped module has no
//! transpiler (architecture.md 8.3). One JSON file: the manifest's name, rate and seed, the scene,
//! and the compiled scripts in the form `CompiledSet::to_json` writes. `examples/pack.rs` writes it
//! from a project directory, until `pocket pack <project> --web` (architecture.md 4.13) is built
//! (threads-slice1.md 14).

use pocket_contract::{Problem, detail};
use pocket_runtime::{GameSetup, Scene};
use pocket_sim::TickRate;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The value of [`Package::format`].
pub const PACKAGE_FORMAT: &str = "pocket-web-package";

/// The version of the layout below.
pub const PACKAGE_VERSION: u32 = 1;

/// A project for the browser.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    /// Always [`PACKAGE_FORMAT`].
    pub format: String,
    /// [`PACKAGE_VERSION`].
    pub version: u32,
    pub name: String,
    /// Ticks per second.
    pub rate: u32,
    /// The seed a run uses when none is given.
    pub seed: u64,
    pub scene: Scene,
    /// The compiled scripts (`pocket_script::CompiledSet` as JSON: modules, entry, bundle).
    pub scripts: Value,
    /// The player declarations (pocket-runtime's `PlayerSpec`), when the game has players.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub player: Option<pocket_runtime::player::PlayerSpec>,
}

fn invalid(message: String) -> Problem {
    Problem::new(
        "project.invalid",
        message.clone(),
        detail([("path", json!("package.json")), ("message", json!(message))]),
    )
}

impl Package {
    /// The package of a game's setup (natively, from a project read and compiled from disk).
    pub fn from_setup(setup: &GameSetup, seed: u64) -> Package {
        Package {
            format: PACKAGE_FORMAT.to_owned(),
            version: PACKAGE_VERSION,
            name: setup.name.clone(),
            rate: setup.rate.0,
            seed,
            scene: setup.scene.clone(),
            scripts: serde_json::to_value(&setup.scripts).unwrap_or(Value::Null),
            player: setup.player.clone(),
        }
    }

    /// Reads a package (`project.invalid {path, message}` when it does not decode).
    pub fn from_bytes(bytes: &[u8]) -> Result<Package, Problem> {
        let p: Package = serde_json::from_slice(bytes)
            .map_err(|e| invalid(format!("The web package does not decode: {e}.")))?;
        if p.format != PACKAGE_FORMAT || p.version != PACKAGE_VERSION {
            return Err(invalid(format!(
                "The web package is {} version {}; this build reads {PACKAGE_FORMAT} version {PACKAGE_VERSION}.",
                p.format, p.version
            )));
        }
        Ok(p)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// What a game is built from. The scripts run exactly as compiled; a package whose scripts were
    /// not what the native build compiled shows as a different world hash in the cross-target
    /// comparison (checks.md 7.2).
    pub fn setup(&self) -> Result<GameSetup, Problem> {
        Ok(GameSetup {
            name: self.name.clone(),
            rate: TickRate::new(self.rate)?,
            scene: self.scene.clone(),
            scripts: serde_json::from_value(self.scripts.clone())
                .map_err(|e| invalid(format!("The package's scripts do not decode: {e}.")))?,
            source: None,
            limits: Default::default(),
            lint_off: false,
            player: self.player.clone(),
        })
    }
}
