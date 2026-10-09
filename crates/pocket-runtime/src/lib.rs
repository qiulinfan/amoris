//! The game: the composition of the simulation, physics and scripts loaded from a project, the
//! command catalog and its application at tick boundaries, and the game thread (docs/spec/
//! architecture.md 4.8, threads.md).
//!
//! - [`Project`] reads a project directory (`project.toml`, `scene.json`, `scripts/`) and, with the
//!   feature `transpile`, compiles it into a [`GameSetup`].
//! - [`Game`] is the synchronous form every check and the headless batch form use: `apply` a
//!   command at the boundary the world is at, `step` a tick, `fork`, `snapshot`, record; it
//!   implements spec-persist's `Stepper`, so replays and lockstep drive it.
//! - With the feature `thread` (native), [`thread::GameThread`] runs a `Game` on its own thread,
//!   publishing snapshots and taking commands through `pocket-link`; it adds what needs a loop:
//!   pacing, Play (a fork of the edit world run in real time) and the kept snapshots.
//! - The catalog ([`CATALOG`], docs/spec/server.md) names every command with its kind and the JSON
//!   Schema of its parameters; `world.edit` keeps an undo history ([`history`]).

// A tick-code crate (numeric.md 5 and 8): its slot functions run inside ticks.
#![deny(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

mod catalog;
pub mod control;
mod edit;
mod engine;
pub mod files;
mod game;
pub mod history;
pub mod inspect;
pub mod present;
mod project;
mod scene;
mod scripts;
#[cfg(feature = "thread")]
pub mod thread;
pub mod types;
mod values;

pub use catalog::{CATALOG, Command, CommandDef, CommandFn, NoParams, catalog_json};
pub use control::{RestoreBundle, Sample, SnapshotsRestoreParams, StepParams};
pub use edit::{Edit, EditOp, WorldEditOps, WorldEditParams, WorldGetParams, label_of};
pub use engine::{engine_component, merge};
pub use game::{Extras, Game, GameBuilder, SystemFn, registry, run_config};
/// Presenter-side static walking queries; authoritative gameplay remains in the game world.
pub use pocket_physics::walk;
pub use present::Extractor;
pub use project::{GameSetup, Manifest, Project, read_file, toml_value};
pub use scene::{Prefab, SCENE_FORMAT, Scene, SceneEntity, prefab_components};
pub use scripts::{ENTRY, ScriptsApplyParams, bundle_record, compiled_from_record};
pub use values::{EntityRef, named, resolve};

#[cfg(feature = "transpile")]
pub use project::lint;
#[cfg(feature = "transpile")]
pub use scripts::compile;

use pocket_contract::{CheckOptions, Problem};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;

/// The stack of a thread that runs scripts: room for the script host's limit and the Rust frames
/// below the first call (script-sandbox.md 4.3). Every thread that builds a [`Game`] needs it.
pub const GAME_STACK_BYTES: usize = pocket_script::host::THREAD_STACK_BYTES;

/// Decodes `value` strictly (unknown fields refused with suggestions, charter 3.4); `owner` is what
/// it is, said as an agent would.
pub fn decode<T: DeserializeOwned + JsonSchema>(
    value: &serde_json::Value,
    owner: &str,
) -> Result<T, Problem> {
    pocket_contract::decode::<T>(value, &CheckOptions::new(owner)).map(|d| d.value)
}
