//! The command catalog (architecture.md 4.8; docs/spec/host-protocol.md 4; docs/spec/server.md):
//! every command's name, kind, purpose and parameters' JSON Schema. The kind of a command comes
//! from here, never from its sender (threads.md 5.1). Games and tests may add their own commands
//! ([`crate::GameBuilder::command`]).
//!
//! Names are `noun.verb`. The slice 1 names (`world_get`, `step`, `time_control`) stay as aliases;
//! `world_edit` stays a command of its own, the canonical recorded form every world edit, undo and
//! redo is recorded in (threads.md 5.3), so recordings keep one write for edits.
//!
//! Some commands need the game thread ([`CommandDef::thread`]): pacing, Play, the kept snapshots.
//! A [`crate::Game`] driven directly refuses them with `command.thread_only`.

use std::sync::Arc;

use pocket_contract::codes::unknown_method;
use pocket_contract::{Candidate, Problem, detail};
use pocket_link::{Kind, Source};
use pocket_sim::Boundary;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::control::{PlayParams, SnapshotsRestoreParams, StepParams, TimeControlParams};
use crate::edit::{WorldEditOps, WorldEditParams, WorldGetParams};
use crate::files::{ScriptReadParams, ScriptWriteParams};
use crate::inspect::{WorldQueryParams, WorldSchemaParams, WorldTreeParams};
use crate::scripts::{ScriptsApplyParams, ScriptsSwapParams};
use crate::types::ScriptsTypesParams;

/// A command that takes no parameters.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoParams {}

/// A command of the catalog.
#[derive(Clone, Copy, Debug)]
pub struct CommandDef {
    pub name: &'static str,
    pub kind: Kind,
    /// Only the runtime's own producers may send it.
    pub host_only: bool,
    /// Needs the game thread (pacing, Play, kept snapshots).
    pub thread: bool,
    pub doc: &'static str,
    /// Older names that still reach it.
    pub aliases: &'static [&'static str],
    /// The JSON Schema of its parameters.
    pub params: fn() -> Value,
}

fn schema<T: JsonSchema>() -> Value {
    schemars::schema_for!(T).to_value()
}

const fn def(
    name: &'static str,
    kind: Kind,
    doc: &'static str,
    params: fn() -> Value,
) -> CommandDef {
    CommandDef {
        name,
        kind,
        host_only: false,
        thread: false,
        doc,
        aliases: &[],
        params,
    }
}

const fn on_thread(mut d: CommandDef) -> CommandDef {
    d.thread = true;
    d
}

const fn aka(mut d: CommandDef, aliases: &'static [&'static str]) -> CommandDef {
    d.aliases = aliases;
    d
}

/// The engine's commands.
pub const CATALOG: &[CommandDef] = &[
    def(
        "catalog.list",
        Kind::Read,
        "Every command: name, kind, doc, aliases and the JSON Schema of its parameters.",
        schema::<NoParams>,
    ),
    (def(
        "status",
        Kind::Read,
        "Tick, time, pacing, Play or edit, world hash, entity count, bundle.",
        schema::<NoParams>,
    )),
    def(
        "project.info",
        Kind::Read,
        "The project: name, root, tick rate, scene, scripts and assets.",
        schema::<NoParams>,
    ),
    (def(
        "project.save",
        Kind::Request,
        "Writes the edit world to the project's scene.json (Play's world is never saved).",
        schema::<NoParams>,
    )),
    def(
        "world.tree",
        Kind::Read,
        "Entities with their names and component names; filter by name or components.",
        schema::<WorldTreeParams>,
    ),
    aka(
        def(
            "world.get",
            Kind::Read,
            "An entity's components as JSON (all, or those named).",
            schema::<WorldGetParams>,
        ),
        &["world_get"],
    ),
    def(
        "world.query",
        Kind::Read,
        "Rows of the entities that have every listed component, with the fields asked for.",
        schema::<WorldQueryParams>,
    ),
    def(
        "world.schema",
        Kind::Read,
        "Component JSON Schemas with their docs (all, or one).",
        schema::<WorldSchemaParams>,
    ),
    def(
        "world.edit",
        Kind::Write,
        "Spawn, set (fields merge), remove and destroy, all or nothing at one boundary; undoable.",
        schema::<WorldEditOps>,
    ),
    def(
        "world_edit",
        Kind::Write,
        "world.edit's canonical recorded form (edits tagged by op; revive restores a destroyed id).",
        schema::<WorldEditParams>,
    ),
    def(
        "history.list",
        Kind::Read,
        "The undo and redo stacks' labels, most recent last.",
        schema::<NoParams>,
    ),
    def(
        "history.undo",
        Kind::Write,
        "Undoes the last world edit (from the editor or an agent).",
        schema::<NoParams>,
    ),
    def(
        "history.redo",
        Kind::Write,
        "Redoes the last undone edit.",
        schema::<NoParams>,
    ),
    on_thread(aka(
        def(
            "time.control",
            Kind::Control,
            "Pause, resume, set the real-time speed or the pacing.",
            schema::<TimeControlParams>,
        ),
        &["time_control"],
    )),
    aka(
        def(
            "time.step",
            Kind::Control,
            "Runs ticks; with until/watch, stops early when an event is emitted or a field meets a test.",
            schema::<StepParams>,
        ),
        &["step"],
    ),
    on_thread(def(
        "play.start",
        Kind::Control,
        "Play: forks the edit world and runs the fork in real time; edits go to the fork.",
        schema::<PlayParams>,
    )),
    on_thread(def(
        "play.stop",
        Kind::Control,
        "Stop: discards Play's fork and returns to the edit world as it was.",
        schema::<NoParams>,
    )),
    on_thread(def(
        "snapshots.list",
        Kind::Read,
        "The kept snapshots of the world shown (one every 60 ticks, the last 120).",
        schema::<NoParams>,
    )),
    on_thread(def(
        "snapshots.restore",
        Kind::Control,
        "Restores the kept snapshot at or before a tick; clears the history.",
        schema::<SnapshotsRestoreParams>,
    )),
    def(
        "snapshot",
        Kind::Read,
        "The world's snapshot (in process; its tick and hash over the wire).",
        schema::<NoParams>,
    ),
    def(
        "scripts.list",
        Kind::Read,
        "The project's script files with their sizes and last diagnostics.",
        schema::<NoParams>,
    ),
    def(
        "scripts.read",
        Kind::Read,
        "A script file's text.",
        schema::<ScriptReadParams>,
    ),
    def(
        "scripts.write",
        Kind::Request,
        "Writes a script file and compiles the scripts with it (no swap); returns diagnostics.",
        schema::<ScriptWriteParams>,
    ),
    def(
        "scripts.types",
        Kind::Read,
        "Writes the SDK's declarations for the scripts on disk into .pocket/types (pocket.d.ts, components.d.ts, and the tsconfig.json tsc uses there) and, unless tsconfig is false, a tsconfig.json for editors if the project has none.",
        schema::<ScriptsTypesParams>,
    ),
    def(
        "scripts.apply",
        Kind::Request,
        "Compiles the project's scripts (or the given files) and hot-swaps them at a boundary.",
        schema::<ScriptsApplyParams>,
    ),
    CommandDef {
        host_only: true,
        ..def(
            "scripts.swap",
            Kind::Write,
            "Swaps the world's scripts for a prepared bundle; produced by scripts.apply.",
            schema::<ScriptsSwapParams>,
        )
    },
    def(
        "scripts.status",
        Kind::Read,
        "The current bundle hash and the systems that ran last tick.",
        schema::<NoParams>,
    ),
    // The player tools (docs/spec/player.md; shared/contract/mcp.md 5): a command sent as
    // `Source::Player(i)` is a player at seat i, restricted to its own perception.
    def(
        "player.session",
        Kind::Read,
        "Who you are: role, seat, pacing, the pending decision, the push cursor, decision stats.",
        schema::<crate::player::SessionParams>,
    ),
    def(
        "player.describe",
        Kind::Read,
        "The game definition for your seat (or one part: intents, kinds, ...), or one entity you know: its facts, affordances and latest events.",
        schema::<crate::player::DescribeParams>,
    ),
    def(
        "player.observe",
        Kind::Read,
        "Your seat's instruments, intents, pending decision, ranked percepts and events since, within budget_tokens; projection text, json or tensor.",
        schema::<pocket_interface::perception::ObserveRequest>,
    ),
    def(
        "player.nearby",
        Kind::Read,
        "Percepts filtered by kind, distance, sector and visibility.",
        schema::<pocket_interface::perception::NearbyRequest>,
    ),
    def(
        "player.events",
        Kind::Read,
        "Your perceived events after a seq, oldest first.",
        schema::<pocket_interface::perception::EventsRequest>,
    ),
    def(
        "player.affordances",
        Kind::Read,
        "What you can do with what you perceive: each verb, available or why not.",
        schema::<pocket_interface::action::AffordancesRequest>,
    ),
    def(
        "player.intents",
        Kind::Read,
        "Your intents, newest first, failed ones with their whole problem.",
        schema::<pocket_interface::action::IntentsRequest>,
    ),
    def(
        "player.act",
        Kind::Write,
        "Actions for your seat, validated whole and applied at this boundary: set, pulse, start an intent, use an affordance, cancel; resume answers the pending decision.",
        schema::<pocket_interface::action::ActRequest>,
    ),
    def(
        "player.wait",
        Kind::Control,
        "Lets time pass until your next decision point (or until), at most ticks; answers with the events since your cursor and, with observe, an observation.",
        schema::<crate::player::WaitParams>,
    ),
    def(
        "player.continue",
        Kind::Control,
        "Answers your pending decision without acting (real time resumes).",
        schema::<pocket_interface::time::play::ContinueRequest>,
    ),
    def(
        "player.pacing",
        Kind::Control,
        "Developer: the players' pacing: stepped, or real_time {speed, pause_on_decision, clock}.",
        schema::<crate::player::PacingParams>,
    ),
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
        detail([
            ("command", json!(name)),
            ("source", pocket_link::source_json(source)),
        ]),
    )
}

/// `command.thread_only {command}`: a command that needs the game thread, sent to a game driven
/// directly.
pub fn thread_only(name: &str) -> Problem {
    Problem::new(
        "command.thread_only",
        format!("{name} needs the game thread (pocket serve); a game driven directly has none."),
        detail([("command", json!(name))]),
    )
}

/// The catalog entry of `name` or of one of its aliases.
pub fn find(name: &str) -> Option<&'static CommandDef> {
    CATALOG
        .iter()
        .find(|c| c.name == name || c.aliases.contains(&name))
}

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Read => "read",
        Kind::Write => "write",
        Kind::Control => "control",
        Kind::Request => "request",
    }
}

/// The catalog as JSON: `[{name, kind, doc, aliases, params, thread}]`, the engine's commands and
/// then a game's own (whose parameters have no schema).
pub fn catalog_json(extra: &[(String, Kind, CommandFn)]) -> Value {
    let mut out: Vec<Value> = CATALOG
        .iter()
        .filter(|c| !c.host_only)
        .map(|c| {
            json!({
                "name": c.name,
                "kind": kind_name(c.kind),
                "doc": c.doc,
                "aliases": c.aliases,
                "thread": c.thread,
                "params": (c.params)(),
            })
        })
        .collect();
    out.extend(extra.iter().map(|(n, k, _)| {
        json!({"name": n, "kind": kind_name(*k), "doc": "A command the game adds.",
               "aliases": [], "thread": false, "params": {"type": "object"}})
    }));
    Value::Array(out)
}
