//! The command catalog (architecture.md 4.8): every command's name, kind and purpose. The kind of a
//! command comes from here, never from its sender (threads.md 5.1). Games and tests may add their
//! own commands ([`crate::GameBuilder::command`]).

use std::sync::Arc;

use pocket_contract::codes::unknown_method;
use pocket_contract::{Candidate, Problem};
use pocket_link::{Kind, Source};
use pocket_sim::Boundary;
use serde_json::Value;

/// A command of the catalog.
#[derive(Clone, Copy, Debug)]
pub struct CommandDef {
    pub name: &'static str,
    pub kind: Kind,
    /// Only the runtime's own producers may send it.
    pub host_only: bool,
    pub doc: &'static str,
}

/// The engine's commands in slice 1.
pub const CATALOG: &[CommandDef] = &[
    CommandDef {
        name: "world_edit",
        kind: Kind::Write,
        host_only: false,
        doc: "Spawn, set, remove and destroy, checked whole and applied at one boundary.",
    },
    CommandDef {
        name: "world_get",
        kind: Kind::Read,
        host_only: false,
        doc: "An entity's components as JSON.",
    },
    CommandDef {
        name: "status",
        kind: Kind::Read,
        host_only: false,
        doc: "The tick, the writes applied at this boundary, the world hash and the bundle.",
    },
    CommandDef {
        name: "snapshot",
        kind: Kind::Read,
        host_only: false,
        doc: "The world's snapshot (in process; its tick and hash over the wire).",
    },
    CommandDef {
        name: "scripts.apply",
        kind: Kind::Request,
        host_only: false,
        doc: "Compile and load scripts: a hot update, or with force a reload of unchanged ones.",
    },
    CommandDef {
        name: "scripts.swap",
        kind: Kind::Write,
        host_only: true,
        doc: "Swap the world's scripts for a prepared bundle; produced by scripts.apply.",
    },
    CommandDef {
        name: "scripts.status",
        kind: Kind::Read,
        host_only: false,
        doc: "The current and previous bundle hashes and the systems.",
    },
    CommandDef {
        name: "step",
        kind: Kind::Control,
        host_only: false,
        doc: "Run ticks; answered after the last.",
    },
    CommandDef {
        name: "time_control",
        kind: Kind::Control,
        host_only: false,
        doc: "Pause, resume or change the pacing (the game thread only).",
    },
];

/// A command a game or a test adds: a function over the boundary that returns the result and the
/// canonical recorded form of its parameters. A Write must finish validating before it changes a
/// component or despawns: when it fails, the game undoes only its spawns, ids and events
/// (`Boundary::rollback`), so a failure after another change would leave a refused call applied
/// and the recording without it (charter 3.4; threads.md 5.4).
pub type CommandFn =
    Arc<dyn Fn(&mut Boundary<'_>, &Value) -> Result<(Value, Value), Problem> + Send + Sync>;

/// One command as the game applies it.
#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    pub source: Source,
    pub seq: u64,
    pub name: String,
    pub params: Value,
}

impl Command {
    pub fn new(source: Source, seq: u64, name: &str, params: Value) -> Command {
        Command {
            source,
            seq,
            name: name.to_owned(),
            params,
        }
    }
}

/// `request.unknown_method` with the catalog's nearest names.
pub fn unknown(name: &str, extra: &[(String, Kind, CommandFn)]) -> Problem {
    let mut valid: Vec<Candidate<'_>> = CATALOG.iter().map(|c| Candidate::new(c.name)).collect();
    valid.extend(extra.iter().map(|(n, _, _)| Candidate::new(n)));
    unknown_method(name, &valid, None)
}

/// `command.host_only {command}`: a Host write sent by another source.
pub fn host_only(name: &str, source: Source) -> Problem {
    Problem::new(
        "command.host_only",
        format!("Only the runtime itself sends {name}; {source:?} cannot."),
        pocket_contract::detail([
            ("command", serde_json::json!(name)),
            ("source", pocket_link::source_json(source)),
        ]),
    )
}

/// The catalog entry of `name`.
pub fn find(name: &str) -> Option<&'static CommandDef> {
    CATALOG.iter().find(|c| c.name == name)
}
