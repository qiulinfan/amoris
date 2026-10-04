//! Assets and what things look like (charter 4.4, 5.1): the visual components (`Model`, `Light`,
//! `Camera`, `Environment`, `Splat`), CPU mesh data with the built-in primitives, glTF import
//! (feature `import`), and the render feed that carries visual changes from the game thread to the
//! renderers. Game side: no GPU, no threads, builds for `wasm32`.

pub mod frame;
#[cfg(feature = "import")]
pub mod import;
pub mod mesh;
pub mod primitives;
pub mod visual;

use pocket_contract::Problem;
use pocket_sim::{ComponentRegistry, Persisted, RegisterPersisted, Sim};
use serde_reflection::{Samples, Tracer};

pub use frame::{Feed, Mailbox, RenderFrame};
pub use mesh::{MeshData, ModelAsset, Vertex};
pub use visual::{Camera, Environment, Light, LightKind, Model, SkyKind, Splat};

impl Persisted for Model {
    const NAME: &'static str = "Model";
    const VERSION: u32 = 1;
}

impl Persisted for Light {
    const NAME: &'static str = "Light";
    const VERSION: u32 = 1;

    fn trace(t: &mut Tracer, s: &Samples) -> serde_reflection::Result<()> {
        t.trace_type::<LightKind>(s)?;
        t.trace_type::<Self>(s).map(|_| ())
    }
}

impl Persisted for Camera {
    const NAME: &'static str = "Camera";
    const VERSION: u32 = 1;
}

impl Persisted for Environment {
    const NAME: &'static str = "Environment";
    const VERSION: u32 = 1;

    fn trace(t: &mut Tracer, s: &Samples) -> serde_reflection::Result<()> {
        t.trace_type::<SkyKind>(s)?;
        t.trace_type::<Self>(s).map(|_| ())
    }
}

impl Persisted for Splat {
    const NAME: &'static str = "Splat";
    const VERSION: u32 = 1;
}

/// Registers the visual components with the world's component registry.
pub fn plugin(sim: &mut Sim) -> Result<(), Problem> {
    let w = sim.world_mut();
    ComponentRegistry::register::<Model>(w, None)?;
    ComponentRegistry::register::<Light>(w, None)?;
    ComponentRegistry::register::<Camera>(w, None)?;
    ComponentRegistry::register::<Environment>(w, None)?;
    ComponentRegistry::register::<Splat>(w, None)?;
    Ok(())
}

/// Declares the visual components to persistence: snapshotted, hashed and forked like the rest.
pub fn declare<R: RegisterPersisted>(r: &mut R) {
    r.component::<Model>()
        .component::<Light>()
        .component::<Camera>()
        .component::<Environment>()
        .component::<Splat>();
}
