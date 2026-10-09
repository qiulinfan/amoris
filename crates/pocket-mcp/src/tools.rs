//! The tool list (docs/spec/host-protocol.md 7; docs/spec/server.md, MCP): a few grouped tools,
//! each an `action` over catalog methods, so an agent's tool list stays small. Schemas name the
//! parameters and stay loose: the runtime's strict decoder refuses unknown fields with a
//! suggestion, so the catalog stays the one place parameters are checked.

use serde_json::{Map, Value, json};

/// One tool: its name, description and input schema.
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: Value,
}

/// The `debug` tool's actions: every `debug.*` method (pocket-debug's `methods()` and the host's
/// `debug.rewind`; pocket-app's tests check the two agree).
pub const DEBUG_ACTIONS: &[&str] = &[
    "attach",
    "detach",
    "breakpoints.set",
    "breakpoints.clear",
    "breakpoints.list",
    "pause",
    "continue",
    "step",
    "state",
    "eval",
    "set",
    "watch",
    "unwatch",
    "exceptions",
    "wait",
    "rewind",
];

fn action(values: &[&str]) -> Value {
    json!({"type": "string", "enum": values})
}

fn obj(props: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": props, "required": required})
}

/// Every tool.
pub fn tools() -> Vec<ToolDef> {
    let entity = json!({"type": ["integer", "string"], "description": "id, name or Name#id"});
    let strings = json!({"type": "array", "items": {"type": "string"}});
    vec![
        ToolDef {
            name: "world",
            description: "The authoritative world (editors see your edits live). tree: entities with \
                component names (filter, with). get {entity, components, fields ['Boat.rudder']}. \
                query {with, fields ['Boat.speed'], name, limit}: rows. schema {component}. edit \
                {ops, label}: one undoable transaction, ops [{spawn:{name,prefab,components}} | \
                {set:{entity,component,value}} (fields merge) | {remove:{entity,component}} | \
                {destroy:{entity}}]. While the debugger holds the game, reads answer from the \
                last tick's end (paused_at).",
            schema: obj(
                json!({
                    "action": action(&["tree", "get", "query", "schema", "edit"]),
                    "entity": entity,
                    "components": strings,
                    "filter": {"type": "string"},
                    "with": strings,
                    "fields": strings,
                    "name": {"type": "string"},
                    "limit": {"type": "integer"},
                    "component": {"type": "string"},
                    "ops": {"type": "array", "items": {"type": "object"}},
                    "label": {"type": "string"},
                }),
                &["action"],
            ),
        },
        ToolDef {
            name: "scripts",
            description: "TypeScript under scripts/ (the pocket SDK). guide: how to write \
                them (read it first); list; read {path, lines '50-80', numbered}; write {path, \
                text} (diagnostics, no swap); apply: compile, hot-swap, type check with tsc \
                (diagnostics with TS locations); check: the same, nothing swapped; status: the \
                bundle running; types: write .pocket/types (pocket.d.ts, components.d.ts: every \
                component and field scripts can name) and list the components.",
            schema: obj(
                json!({
                    "action": action(&["guide", "list", "read", "write", "apply", "check",
                                       "status", "types"]),
                    "path": {"type": "string"},
                    "text": {"type": ["string", "boolean"],
                             "description": "write: the file's text; types: true to return the \
                                 declarations' text too"},
                    "lines": {"type": "string", "description": "read: 1-based, inclusive: \
                        50-80, 50-, -20 or 67"},
                    "numbered": {"type": "boolean", "description": "read: prefix each line \
                        with its number"},
                    "force": {"type": "boolean"},
                }),
                &["action"],
            ),
        },
        ToolDef {
            name: "time",
            description: "Time of the world shown. status; pause; resume; speed {speed}; step \
                {ticks, until:{event:'crate.taken'|'crate.*', subject, tick}, watch:{entity, \
                component, field, op:changes|crosses|>|>=|<|<=|==|!=, value}, sample:{fields \
                ['Sloop.Boat.rudder'], every}}: stops early when met or at a breakpoint \
                (stopped_by), samples come back as a table; snapshots: kept every 60 ticks; \
                rewind {tick, bundle}: restore the kept snapshot at or before tick under the \
                applied scripts (bundle 'snapshot': its own).",
            schema: obj(
                json!({
                    "action": action(&["status", "pause", "resume", "speed", "step", "snapshots",
                                       "rewind"]),
                    "ticks": {"type": "integer"},
                    "speed": {"type": "number"},
                    "until": {"type": "object"},
                    "watch": {"type": "object"},
                    "sample": {"type": "object", "properties": {
                        "fields": strings, "every": {"type": "integer"}}},
                    "tick": {"type": "integer"},
                    "bundle": {"type": "string", "enum": ["applied", "snapshot"]},
                }),
                &["action"],
            ),
        },
        ToolDef {
            name: "play",
            description: "start: fork the edit world and run the fork in real time {speed, \
                paused}; stop: discard it and return to the edit world.",
            schema: obj(
                json!({
                    "action": action(&["start", "stop"]),
                    "speed": {"type": "number"},
                    "paused": {"type": "boolean"},
                }),
                &["action"],
            ),
        },
        ToolDef {
            name: "history",
            description: "undo / redo the last world edit (shared with the editor); list.",
            schema: obj(
                json!({"action": action(&["undo", "redo", "list"])}),
                &["action"],
            ),
        },
        ToolDef {
            name: "assets",
            description: "list the project's files {dir} with kind and size.",
            schema: obj(
                json!({"action": action(&["list"]), "dir": {"type": "string"}}),
                &["action"],
            ),
        },
        ToolDef {
            name: "events",
            description: "since {seq, limit, name 'crate.*'}: events after seq, the newest without \
                it; why {seq}: the cause chain; log {seq, limit}: script console and errors.",
            schema: obj(
                json!({
                    "action": action(&["since", "why", "log"]),
                    "seq": {"type": "integer"},
                    "limit": {"type": "integer"},
                    "name": {"type": "string"},
                }),
                &["action"],
            ),
        },
        ToolDef {
            name: "debug",
            description: "Script debugger (TypeScript lines, 1-based). breakpoints.set {file, \
                line, condition, log}; breakpoints.clear {id}; breakpoints.list; state {brief}; \
                eval {expr, frame}; set {name, value, frame}; step {kind: over|into|out}; \
                continue; pause; wait {timeout_ms}; watch {entity, component, field}: pause when \
                a script's write changes it; unwatch {id}; exceptions {mode: \
                none|uncaught|all}; rewind {tick, bundle}; attach; detach. A time step that \
                hits a breakpoint returns at once (stopped_by); while held, reads still answer \
                and other calls are refused with debug.paused.",
            schema: obj(
                json!({
                    "action": action(DEBUG_ACTIONS),
                    "file": {"type": "string", "description": "scripts/helm.ts"},
                    "line": {"type": "integer"},
                    "condition": {"type": "string"},
                    "log": {"type": "string"},
                    "id": {"type": "string"},
                    "brief": {"type": "boolean"},
                    "expr": {"type": "string"},
                    "frame": {"type": "integer"},
                    "name": {"type": "string"},
                    "value": {"type": "string"},
                    "kind": action(&["over", "into", "out"]),
                    "timeout_ms": {"type": "integer"},
                    "entity": entity,
                    "component": {"type": "string"},
                    "field": {"type": "string"},
                    "mode": action(&["none", "uncaught", "all"]),
                    "tick": {"type": "integer"},
                    "bundle": {"type": "string", "enum": ["applied", "snapshot"]},
                }),
                &["action"],
            ),
        },
        ToolDef {
            name: "capture",
            description: "A rendered image or the id buffer's summary of a camera view {camera, \
                kind: image|ids}.",
            schema: obj(
                json!({"camera": {"type": "object"}, "kind": action(&["image", "ids"])}),
                &[],
            ),
        },
        ToolDef {
            name: "docs",
            description: "Search the commands and components {query}.",
            schema: obj(json!({"query": {"type": "string"}}), &["query"]),
        },
    ]
}

/// The catalog method and parameters a tool call stands for, or why it stands for none.
pub fn route(tool: &str, mut args: Map<String, Value>) -> Result<(String, Value), String> {
    let action = args
        .remove("action")
        .and_then(|a| a.as_str().map(str::to_owned));
    let take = |args: &mut Map<String, Value>, keys: &[&str]| -> Map<String, Value> {
        keys.iter()
            .filter_map(|k| args.remove(*k).map(|v| ((*k).to_owned(), v)))
            .collect()
    };
    let need = |a: Option<String>| a.ok_or_else(|| format!("{tool} needs an action"));
    let rest = |args: Map<String, Value>| Value::Object(args);
    let (method, params) = match tool {
        "world" | "scripts" | "history" | "assets" | "debug" => {
            (format!("{tool}.{}", need(action)?), rest(args))
        }
        "play" => (format!("play.{}", need(action)?), rest(args)),
        "time" => match need(action)?.as_str() {
            "status" => ("status".into(), rest(args)),
            "pause" => ("time.control".into(), json!({"pause": true})),
            "resume" => ("time.control".into(), json!({"pause": false})),
            "speed" => (
                "time.control".into(),
                Value::Object(take(&mut args, &["speed"])),
            ),
            "step" => ("time.step".into(), rest(args)),
            "snapshots" => ("snapshots.list".into(), rest(args)),
            "rewind" => ("snapshots.restore".into(), rest(args)),
            other => return Err(format!("time has no action '{other}'")),
        },
        "events" => match need(action)?.as_str() {
            "since" => ("events.since".into(), rest(args)),
            "why" => ("events.why".into(), rest(args)),
            "log" => ("log.since".into(), rest(args)),
            other => return Err(format!("events has no action '{other}'")),
        },
        "capture" => ("capture".into(), rest(args)),
        "docs" => ("docs.search".into(), rest(args)),
        other => return Err(format!("there is no tool '{other}'")),
    };
    Ok((method, params))
}
