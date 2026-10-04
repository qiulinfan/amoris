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
                component names (filter, with). get {entity, components}. query {with, fields \
                ['Boat.speed'], name, limit}: rows. schema {component}. edit {ops, label}: one \
                undoable transaction, ops [{spawn:{name,prefab,components}} | {set:{entity,\
                component,value}} (fields merge) | {remove:{entity,component}} | \
                {destroy:{entity}}].",
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
            description: "TypeScript under scripts/ (the pocket SDK: docs/sdk.md). list; read \
                {path}; write {path, text} (diagnostics, no swap); apply: compile, hot-swap, type \
                check with tsc (diagnostics with TS locations); check: the same, nothing swapped; \
                types: write .pocket/types (pocket.d.ts, components.d.ts: every component and \
                field scripts can name) and list the components.",
            schema: obj(
                json!({
                    "action": action(&["list", "read", "write", "apply", "check", "types"]),
                    "path": {"type": "string"},
                    "text": {"type": ["string", "boolean"],
                             "description": "write: the file's text; types: true to return the \
                                 declarations' text too"},
                    "force": {"type": "boolean"},
                }),
                &["action"],
            ),
        },
        ToolDef {
            name: "time",
            description: "Time of the world shown. status; pause; resume; speed {speed}; step \
                {ticks, until:{event:'crate.taken'|'crate.*', subject, tick}, watch:{entity, \
                component, field, op:changes|crosses|>|>=|<|<=|==|!=, value}}: stops early when \
                met; snapshots: kept every 60 ticks; rewind {tick}: restore the kept snapshot at \
                or before tick.",
            schema: obj(
                json!({
                    "action": action(&["status", "pause", "resume", "speed", "step", "snapshots",
                                       "rewind"]),
                    "ticks": {"type": "integer"},
                    "speed": {"type": "number"},
                    "until": {"type": "object"},
                    "watch": {"type": "object"},
                    "tick": {"type": "integer"},
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
            description: "Script debugger: breakpoints.set {file, line, condition}, \
                breakpoints.clear, pause, continue, step {kind: over|into|out}, state, eval \
                {expr, frame}, watch {entity, component, field}, rewind {tick}.",
            schema: obj(
                json!({
                    "action": action(&["breakpoints.set", "breakpoints.clear", "pause",
                                       "continue", "step", "state", "eval", "watch", "rewind"]),
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
