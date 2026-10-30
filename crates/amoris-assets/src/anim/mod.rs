//! Character animation in the simulation (docs/spec/animation.md): skeletons and clips loaded with
//! the project (`AnimLibrary`), graphs bound to them (`Rig`), and the deterministic evaluation of a
//! graph's state into a pose, notifies and root motion (`graph::step`). The renderer draws the pose
//! the simulation evaluated.

pub mod clip;
pub mod graph;
#[cfg(feature = "import")]
pub mod load;
pub mod math;
pub mod state;

use std::collections::BTreeMap;
use std::sync::Arc;

use bevy_ecs::prelude::Resource;
use serde::{Deserialize, Serialize};

pub use clip::{Clip, ClipSet, Skeleton};
pub use graph::{GraphDef, Notice, Output, Rig};
pub use math::Xform;
pub use state::{
    AnimCommand, AnimGraph, AnimParam, AnimPose, AnimState, Attach, LayerState, SlotState, Socket,
};

/// A project's animation: its graphs and the files they name (skeletons and clip libraries), by
/// project path. The host builds it when it loads the project; it travels with the compiled
/// scripts, in the web package too.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AnimLibrary {
    pub sets: BTreeMap<String, ClipSet>,
    pub graphs: BTreeMap<String, GraphDef>,
}

/// The project's graphs bound to their skeletons and clips (built once from the library).
#[derive(Resource, Clone, Default)]
pub struct AnimRigs {
    pub sets: Arc<Vec<ClipSet>>,
    pub rigs: Arc<BTreeMap<String, Arc<Rig>>>,
}

impl AnimRigs {
    /// Binds every graph of `lib`; the first graph that does not bind is the error.
    pub fn build(lib: &AnimLibrary) -> Result<AnimRigs, String> {
        let named: Vec<(String, ClipSet)> = lib
            .sets
            .iter()
            .map(|(p, s)| (p.clone(), s.clone()))
            .collect();
        let mut rigs = BTreeMap::new();
        for (path, def) in &lib.graphs {
            let rig = Rig::new(def.clone(), &named).map_err(|e| format!("{path}: {e}"))?;
            rigs.insert(path.clone(), Arc::new(rig));
        }
        Ok(AnimRigs {
            sets: Arc::new(named.into_iter().map(|(_, s)| s).collect()),
            rigs: Arc::new(rigs),
        })
    }

    pub fn rig(&self, graph: &str) -> Option<&Arc<Rig>> {
        self.rigs.get(graph)
    }
}
