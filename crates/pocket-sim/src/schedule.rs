//! Phases, system keys, the system order and the hooks around them (docs/spec/simulation.md 4.3,
//! 4.4 and 6).

use pocket_contract::{Problem, detail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::time::{RunCondition, Tick};

/// The fixed stages of a tick, in order.
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize, JsonSchema,
)]
pub enum TickPhase {
    /// `sim.begin`: the clock advances to tick n.
    Begin,
    /// Intent controllers and control processing (`pocket-interface`).
    Control,
    /// The project's TypeScript systems and script intent executors (`script.update`).
    Update,
    /// Environment and force systems: wind, buoyancy, sail, hull and keel (`pocket-physics`).
    Forces,
    /// The rigid-body step, its write-back and contact events.
    Physics,
    /// Perception and the turn rule, then `sim.finish`.
    Finish,
}

impl TickPhase {
    pub const ALL: [TickPhase; 6] = [
        TickPhase::Begin,
        TickPhase::Control,
        TickPhase::Update,
        TickPhase::Forces,
        TickPhase::Physics,
        TickPhase::Finish,
    ];

    pub fn name(self) -> &'static str {
        match self {
            TickPhase::Begin => "Begin",
            TickPhase::Control => "Control",
            TickPhase::Update => "Update",
            TickPhase::Forces => "Forces",
            TickPhase::Physics => "Physics",
            TickPhase::Finish => "Finish",
        }
    }
}

/// A system's stable key: `physics.step`, `script:take_crates`. One to 64 bytes of lowercase ASCII
/// letters, digits and `_ . : -`.
#[derive(
    Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize, JsonSchema,
)]
pub struct SystemKey(String);

impl SystemKey {
    /// A key, or `sim.system_key_invalid`.
    pub fn new(key: impl Into<String>) -> Result<SystemKey, Problem> {
        let key = key.into();
        let ok = !key.is_empty()
            && key.len() <= 64
            && key
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_.:-".contains(&c));
        if ok {
            Ok(SystemKey(key))
        } else {
            Err(Problem::new(
                "sim.system_key_invalid",
                format!(
                    "'{key}' is not a system key: 1 to 64 lowercase letters, digits and _ . : -."
                ),
                detail([("system", json!(key))]),
            ))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SystemKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The system order of slice 1 (simulation.md 4.4): crates register their systems into these
/// slots by key. A system whose key is not here runs in its phase after the listed systems of that
/// phase (before `sim.finish` in `Finish`), in registration order.
pub const SYSTEM_ORDER: [(&str, TickPhase); 12] = [
    ("sim.begin", TickPhase::Begin),
    ("interface.intents", TickPhase::Control),
    ("script.update", TickPhase::Update),
    ("physics.wind", TickPhase::Forces),
    ("physics.buoyancy", TickPhase::Forces),
    ("physics.sail", TickPhase::Forces),
    ("physics.hull", TickPhase::Forces),
    ("physics.step", TickPhase::Physics),
    ("physics.contacts", TickPhase::Physics),
    ("interface.perception", TickPhase::Finish),
    ("interface.turns", TickPhase::Finish),
    ("sim.finish", TickPhase::Finish),
];

/// The position a system takes: listed keys at twice their row, others just after the last listed
/// row of their phase that is not `sim.finish`.
pub(crate) fn slot(key: &str, phase: TickPhase) -> Result<u32, Problem> {
    if let Some(i) = SYSTEM_ORDER.iter().position(|(k, _)| *k == key) {
        let expected = SYSTEM_ORDER[i].1;
        if expected != phase {
            return Err(Problem::new(
                "sim.system_phase",
                format!(
                    "System {key} runs in the {} phase, not {} (simulation.md 4.4).",
                    expected.name(),
                    phase.name()
                ),
                detail([
                    ("system", json!(key)),
                    ("phase", json!(phase.name())),
                    ("expected", json!(expected.name())),
                ]),
            ));
        }
        return Ok(2 * row(i));
    }
    let last = SYSTEM_ORDER
        .iter()
        .enumerate()
        .filter(|(_, (k, p))| *p == phase && *k != "sim.finish")
        .map(|(i, _)| i)
        .max();
    let last = match last {
        Some(i) => i,
        // A phase with no listed system other than sim.finish: just before sim.finish.
        None => SYSTEM_ORDER.len() - 2,
    };
    Ok(2 * row(last) + 1)
}

fn row(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(u32::MAX / 4)
}

/// One line of the flattened schedule: what the schedule golden prints (simulation.md 12, item 1).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ScheduleEntry {
    pub phase: TickPhase,
    pub key: SystemKey,
    pub condition: RunCondition,
}

/// Called around phases and systems by `Sim::step`, and around each script system by
/// `script.update`: timing and profiling live here, outside the simulation, with a clock the
/// caller injects (threads.md 3.1).
pub trait StepHooks {
    fn phase(&mut self, _phase: TickPhase, _begin: bool) {}
    fn system(&mut self, _key: &SystemKey, _begin: bool) {}
}

/// Hooks that do nothing.
pub struct NoHooks;

impl StepHooks for NoHooks {}

/// What a system written over the whole world receives besides the world: its key, phase and
/// tick, the step's hooks (to time the systems it runs itself, as `script.update` does), and a way
/// to report a fault that poisons the world (simulation.md 4.6).
pub struct SystemCtx<'a> {
    pub(crate) hooks: &'a mut dyn StepHooks,
    pub(crate) key: &'a SystemKey,
    pub(crate) phase: TickPhase,
    pub(crate) tick: Tick,
    pub(crate) fault: Option<Problem>,
}

impl SystemCtx<'_> {
    pub fn hooks(&mut self) -> &mut dyn StepHooks {
        self.hooks
    }

    pub fn key(&self) -> &SystemKey {
        self.key
    }

    pub fn phase(&self) -> TickPhase {
        self.phase
    }

    pub fn tick(&self) -> Tick {
        self.tick
    }

    /// Reports a fault (`script.out_of_memory`, `script.stack_overflow`, or a panic caught at a
    /// native): when the system returns, the step stops, the world is poisoned and `step` answers
    /// with this problem.
    pub fn fault(&mut self, problem: Problem) {
        if self.fault.is_none() {
            self.fault = Some(problem);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_follow_the_table() {
        assert_eq!(slot("sim.begin", TickPhase::Begin).unwrap(), 0);
        assert_eq!(slot("physics.step", TickPhase::Physics).unwrap(), 14);
        assert_eq!(slot("sim.finish", TickPhase::Finish).unwrap(), 22);
        // Unlisted systems: after the listed ones of their phase, before sim.finish.
        assert_eq!(slot("test.extra", TickPhase::Finish).unwrap(), 21);
        assert_eq!(slot("test.extra", TickPhase::Update).unwrap(), 5);
        assert_eq!(slot("test.extra", TickPhase::Begin).unwrap(), 1);
        let e = slot("physics.step", TickPhase::Update).unwrap_err();
        assert_eq!(e.code, "sim.system_phase");
        assert!(SystemKey::new("script:take_crates").is_ok());
        assert!(SystemKey::new("Bad Key").is_err());
        assert!(SystemKey::new("").is_err());
    }
}
