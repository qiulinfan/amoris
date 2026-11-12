//! Loading a scene file at run time (docs/spec/simulation.md 7.6; server.md 3): `scene.load
//! {path, mode}` reads another scene of the project and spawns it at a boundary, replacing the
//! world's entities or adding to them; a script asks for one with `ctx.scene.load`, which the
//! game turns into the same write after the tick.
//!
//! The write is recorded as `scene_load {path, mode, scene}`, `scene` the content hash of the
//! scene as it was read (its cells files resolved into it), whose bytes the recorder keeps as
//! data: a replay spawns those bytes and never reads the file again, so a scene edited on disk
//! after the run cannot change it (replay.md, Data).

use std::path::Path;

use amoris_contract::{Problem, detail};
use amoris_sim::event::{EventKind, NewEvent};
use amoris_sim::{Boundary, ContentHash, EntityId, EntityIndex, PlainData};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::scene::Scene;

/// The context of a loaded scene's content hash.
pub const DATA_CONTEXT: &str = "Amoris loaded scene v1";

/// What a loaded scene does to the entities already there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SceneMode {
    /// Every entity of the world goes, then the scene's come: a new level.
    #[default]
    Replace,
    /// The scene's entities join the world's: a room, a wave, a set piece.
    Add,
}

impl SceneMode {
    fn name(self) -> &'static str {
        match self {
            SceneMode::Replace => "replace",
            SceneMode::Add => "add",
        }
    }
}

/// `scene.load`'s parameters.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SceneLoadParams {
    /// The scene file, relative to the project (`scenes/level2.json`); a `.json` file inside it.
    pub path: String,
    /// `replace` (the default) or `add`.
    #[serde(default)]
    pub mode: SceneMode,
}

/// `scene_load`'s parameters: the recorded form of a `scene.load`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SceneLoadRecorded {
    pub path: String,
    pub mode: SceneMode,
    /// The content hash of the scene's bytes, which the recording holds as data.
    pub scene: String,
}

fn bad_path(path: &str, why: &str) -> Problem {
    Problem::new(
        "request.invalid_value",
        format!("'{path}' is not a scene file of the project: {why}."),
        detail([("path", json!("/path")), ("got", json!(path))]),
    )
}

/// The scene file `path` names under `root`, once checked: relative, inside the project, `.json`.
pub fn file(root: &Path, path: &str) -> Result<std::path::PathBuf, Problem> {
    let p = Path::new(path);
    if path.is_empty()
        || !p
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
    {
        return Err(bad_path(path, "it must be a path inside the project"));
    }
    if p.extension().and_then(|e| e.to_str()) != Some("json") {
        return Err(bad_path(path, "a scene is a .json file"));
    }
    Ok(root.join(p))
}

/// The scene at `path` under `root`, read and checked, its cells files resolved into it, with its
/// canonical bytes and their content hash.
pub fn read(root: &Path, path: &str) -> Result<(Scene, Vec<u8>, ContentHash), Problem> {
    let f = file(root, path)?;
    let text = crate::project::read_file(&f)?;
    let mut scene = Scene::from_json(&text)?;
    crate::voxel_file::resolve_under(&mut scene, root)?;
    let bytes = serde_json::to_vec(&scene).map_err(|e| {
        Problem::new(
            "sim.internal",
            format!("A read scene did not serialize: {e}."),
            detail([]),
        )
    })?;
    let hash = ContentHash::derive(DATA_CONTEXT, &bytes);
    Ok((scene, bytes, hash))
}

/// The scene of a recording's data bytes.
pub fn from_bytes(bytes: &[u8]) -> Result<Scene, Problem> {
    Scene::from_json(std::str::from_utf8(bytes).unwrap_or(""))
}

/// Spawns `scene` at the boundary: its entities first, checked whole (a refusal changes nothing),
/// then, in `replace` mode, every entity that was there before is despawned, in id order. Entity
/// numbers inside its components are places in its file ([`Scene::spawn_placed`]). Appends
/// `scene.loaded {path, mode, entities}`. Answers `{path, mode, spawned, first, despawned}`.
pub fn apply(
    b: &mut Boundary<'_>,
    scene: &Scene,
    path: &str,
    mode: SceneMode,
) -> Result<Value, Problem> {
    let before: Vec<EntityId> = match mode {
        SceneMode::Replace => b.world().resource::<EntityIndex>().ids().collect(),
        SceneMode::Add => Vec::new(),
    };
    let ids = scene.spawn_placed(b)?;
    let mut despawned = 0u64;
    for id in before {
        despawned += u64::try_from(b.despawn_tree(id)?.len()).unwrap_or(u64::MAX);
    }
    let data = PlainData::object(vec![
        ("path".into(), PlainData::String(path.into())),
        ("mode".into(), PlainData::String(mode.name().into())),
        (
            "entities".into(),
            PlainData::Number(f64::from(u32::try_from(ids.len()).unwrap_or(u32::MAX))),
        ),
    ])?;
    let kind = EventKind::new("scene.loaded")?;
    b.emit(NewEvent::new(kind).data(data));
    Ok(json!({
        "path": path,
        "mode": mode.name(),
        "spawned": ids.len(),
        "first": ids.first().map(|i| i.get()),
        "despawned": despawned,
    }))
}

/// A script's requests of the tick (`ctx.scene.load`): its `scene.load_requested` events, in
/// order, as the parameters of the `scene.load` each asks for.
pub fn requested(events: &[amoris_sim::Event]) -> Vec<Value> {
    events
        .iter()
        .filter(|e| e.kind.as_str() == "scene.load_requested")
        .map(|e| e.data.to_json())
        .collect()
}
