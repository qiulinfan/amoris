//! Assets and what things look like (charter 4.4, 5.1): the visual components (`Model`, `Light`,
//! `Camera`, `Environment`, `Splat`), CPU mesh data with the built-in primitives, glTF import
//! (feature `import`), and the render feed that carries visual changes from the game thread to the
//! renderers. Game side: no GPU, no threads, builds for `wasm32`.

pub mod animation;
pub mod frame;
pub mod gi;
#[cfg(feature = "import")]
pub mod import;
pub mod mesh;
pub mod primitives;
pub mod visual;

use pocket_contract::Problem;
use bevy_ecs::prelude::{Query, Res};
use pocket_sim::{ComponentRegistry, Persisted, RegisterPersisted, RunCondition, Sim, SimClock, TickPhase};
use serde_reflection::{Samples, Tracer};

pub use frame::{Feed, Mailbox, RenderFrame};
pub use mesh::{MeshData, ModelAsset, Vertex};
pub use visual::{
    Animator, AudioSource, Camera, Environment, Light, LightKind, Model, ParticleEmitter, SkyKind, Splat, UiAnchor, UiBar, UiText,
};

impl Persisted for Model {
    const NAME: &'static str = "Model";
    const VERSION: u32 = 2;
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
    const VERSION: u32 = 3;

    fn trace(t: &mut Tracer, s: &Samples) -> serde_reflection::Result<()> {
        t.trace_type::<SkyKind>(s)?;
        t.trace_type::<Self>(s).map(|_| ())
    }
}

impl Persisted for Splat {
    const NAME: &'static str = "Splat";
    const VERSION: u32 = 1;
}

impl Persisted for UiText {
    const NAME: &'static str = "UiText";
    const VERSION: u32 = 1;

    fn trace(t: &mut Tracer, s: &Samples) -> serde_reflection::Result<()> {
        t.trace_type::<UiAnchor>(s)?;
        t.trace_type::<Self>(s).map(|_| ())
    }
}

impl Persisted for UiBar {
    const NAME: &'static str = "UiBar";
    const VERSION: u32 = 1;

    fn trace(t: &mut Tracer, s: &Samples) -> serde_reflection::Result<()> {
        t.trace_type::<UiAnchor>(s)?;
        t.trace_type::<Self>(s).map(|_| ())
    }
}

impl Persisted for ParticleEmitter {
    const NAME: &'static str = "ParticleEmitter";
    const VERSION: u32 = 1;
}

impl Persisted for AudioSource {
    const NAME: &'static str = "AudioSource";
    const VERSION: u32 = 1;
}

impl Persisted for Animator {
    const NAME: &'static str = "Animator";
    const VERSION: u32 = 1;
}

/// Advances every playing animator by one tick (per-entity arithmetic only: the result does not
/// depend on the order entities are visited in). Clip lengths live in assets the simulation does
/// not load, so `time` grows without wrapping; the renderer wraps or holds by `looped`.
fn animate(clock: Res<SimClock>, mut q: Query<&mut Animator>) {
    let dt = clock.dt();
    for mut a in &mut q {
        if a.playing && a.speed != 0.0 {
            a.time += dt * a.speed;
        }
    }
}

/// Registers the visual components with the world's component registry.
pub fn plugin(sim: &mut Sim) -> Result<(), Problem> {
    let w = sim.world_mut();
    ComponentRegistry::register::<Model>(w, None)?;
    ComponentRegistry::register::<Light>(w, None)?;
    ComponentRegistry::register::<Camera>(w, None)?;
    ComponentRegistry::register::<Environment>(w, None)?;
    ComponentRegistry::register::<Splat>(w, None)?;
    ComponentRegistry::register::<Animator>(w, None)?;
    ComponentRegistry::register::<UiText>(w, None)?;
    ComponentRegistry::register::<UiBar>(w, None)?;
    ComponentRegistry::register::<AudioSource>(w, None)?;
    ComponentRegistry::register::<ParticleEmitter>(w, None)?;
    sim.add_system("assets.animate", TickPhase::Finish, RunCondition::Always, animate)?;
    Ok(())
}

/// Declares the visual components to persistence: snapshotted, hashed and forked like the rest.
pub fn declare<R: RegisterPersisted>(r: &mut R) {
    r.component::<Model>()
        .component::<Light>()
        .component::<Camera>()
        .component::<Environment>()
        .component::<Splat>()
        .component::<Animator>()
        .component::<UiText>()
        .component::<UiBar>()
        .component::<AudioSource>()
        .component::<ParticleEmitter>();
}
