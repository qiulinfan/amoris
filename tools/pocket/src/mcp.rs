//! `pocket mcp`: a Model Context Protocol server over stdio.
//!
//! It exposes the build tool (build, test, run, gen, doctor) and a live runtime session (start a
//! project with the HTTP control server, step it, read the tree, query, events, capture, pick)
//! as MCP tools, so an agent in Claude Code, Codex or any MCP client works with the engine
//! through the same commands scripts and tests use. Transport: newline-delimited JSON-RPC 2.0.

use crate::manifest::Workspace;
use crate::report::Report;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Stdio};
use std::time::Duration;

const PROTOCOL_VERSION: &str = "2025-06-18";

struct RuntimeSession {
    child: Option<Child>,   // None when attached to a runtime something else started
    url: String,
    project: String,
    stdout: Option<std::thread::JoinHandle<String>>,
}

struct McpServer<'a> {
    ws: &'a Workspace,
    session: Option<RuntimeSession>,
}

fn tool(name: &str, description: &str, schema: Value) -> Value {
    json!({ "name": name, "description": description, "inputSchema": schema })
}

fn obj_schema(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required, "additionalProperties": true })
}

fn tools_list() -> Value {
    json!([
        tool("pocket_doctor", "Report toolchain and dependency status of the Pocket workspace.", obj_schema(json!({}), &[])),
        tool("pocket_build", "Build engine modules (default: everything) with the pocket build tool. Returns structured compiler diagnostics.", obj_schema(json!({
            "targets": { "type": "array", "items": { "type": "string" }, "description": "Module names; empty for all" },
            "config": { "type": "string", "enum": ["debug", "release"], "default": "debug" }
        }), &[])),
        tool("pocket_test", "Build and run the C++ and TypeScript test suites. Returns per-module results and output tails.", obj_schema(json!({
            "filter": { "type": "string", "description": "Substring filter on test module names" },
            "config": { "type": "string", "enum": ["debug", "release"], "default": "debug" }
        }), &[])),
        tool("pocket_check", "Type-check TypeScript against the SDK's types (TypeScript 7): a project's scripts, or the whole workspace without one. Returns each type error with its file, line and column. Run it after editing a script, before running the game.", obj_schema(json!({
            "project": { "type": "string", "description": "project directory or sample name (samples/<name>); the workspace when left out" }
        }), &[])),
        tool("docs_search", "Search the engine's documentation by topic and get the few sections that match (each under its file and heading, about 2,500 characters at most), instead of reading whole files: \"render scale\", \"tilemap sight\", \"Animator.locomotion\", \"save slots\".", obj_schema(json!({
            "query": { "type": "string", "description": "what to look for" },
            "limit": { "type": "integer", "default": 3, "description": "how many sections (1 to 10)" }
        }), &["query"])),
        tool("pocket_gen", "Regenerate code from component metadata (engine/*/meta/*.toml): C++, TypeScript, docs.", obj_schema(json!({ "check": { "type": "boolean", "default": false } }), &[])),
        tool("pocket_scenario", "Run a project's gameplay scenarios (scenarios/*.ts: play through actions, wait for outcomes, check state) at several seeds and return pass counts, ticks to pass and each failure's reason.", obj_schema(json!({
            "project": { "type": "string", "description": "Project name under samples/ or a directory with project.toml" },
            "file": { "type": "string", "description": "One scenario file instead of every file under scenarios/" },
            "seeds": { "type": "integer", "default": 5 },
            "frames": { "type": "integer", "default": 1800, "description": "frame budget per run" },
            "only": { "type": "string", "description": "substring filter on scenario names" },
            "config": { "type": "string", "default": "release", "description": "build configuration of the runtime: release (optimized, the default for agent work) or debug (sanitized, slower)" },
        }), &["project"])),
        tool("pocket_bench", "Run a project's perception benchmarks (benches/*.ts: gameplay questions answered through the instruments) and return, per question, whether the answer was correct, the tokens it cost and what frame-by-frame vision would have cost.", obj_schema(json!({
            "project": { "type": "string", "description": "Project name under samples/ or a directory with project.toml" },
            "file": { "type": "string", "description": "One benchmark file instead of every file under benches/" },
            "frames": { "type": "integer", "default": 1800, "description": "frame budget per run" },
            "only": { "type": "string", "description": "substring filter on questions" },
            "config": { "type": "string", "default": "release", "description": "build configuration of the runtime: release (optimized, the default for agent work) or debug (sanitized, slower)" },
        }), &["project"])),
        tool("pocket_run_headless", "Run a project headless for N frames and return its JSON report (exposed state, state hash, world summary, events, optional capture).", obj_schema(json!({
            "project": { "type": "string", "description": "Project name under samples/ or a directory with project.toml" },
            "frames": { "type": "integer", "default": 120 },
            "capture": { "type": "string", "description": "PNG path for the last frame" },
            "seed": { "type": "integer" },
            "size": { "type": "string", "description": "WxH, e.g. 640x360" },
            "config": { "type": "string", "default": "release", "description": "build configuration of the runtime: release (optimized, the default for agent work) or debug (sanitized, slower)" },
        }), &["project"])),
        tool("runtime_start", "Start a project in a paused runtime with the control server, so it can be stepped and inspected. One session at a time.", obj_schema(json!({
            "project": { "type": "string" },
            "headless": { "type": "boolean", "default": true, "description": "false opens a window" },
            "editor": { "type": "boolean", "default": false, "description": "open the project in the Pocket editor (hierarchy, inspector, play/stop) and operate it through ui_* tools" },
            "seed": { "type": "integer" },
            "size": { "type": "string", "description": "WxH render target size" },
            "history": { "type": "integer", "description": "keep the last N ticks for recorder.at/diff/track/first (time travel)" },
            "config": { "type": "string", "default": "release", "description": "build configuration of the runtime: release (optimized, the default for agent work) or debug (sanitized, slower)" },
        }), &["project"])),
        tool("runtime_attach", "Attach to a runtime that is already running with its control server (started with --serve, an editor, a benchmark harness), by its url; tools then act on it. The server attaches to $POCKET_RPC_URL by itself when that is set.", obj_schema(json!({
            "url": { "type": "string", "description": "the control server's base url, e.g. http://127.0.0.1:4711" }
        }), &["url"])),
        tool("runtime_stop", "Stop the running session and return its final JSON report; an attached runtime is let go (quit: true stops it too).", obj_schema(json!({ "quit": { "type": "boolean", "default": false } }), &[])),
        tool("project_apply", "After editing the running project's scripts or project.toml: bundle and type-check them, reload the project and step it, in one call; answers the type errors, the reload, the state after and any script errors. A bundle that fails leaves the running project as it was.", obj_schema(json!({ "ticks": { "type": "integer", "default": 1 } }), &[])),
        tool("runtime_command", "Send any runtime command with JSON params. Use runtime_commands to list them; the world.*, events.* (events.why explains an event by its causes), recorder.* (time travel when the session started with history), render.* (render.visible: what the camera sees; render.unproject: the world point under a pixel), tilemap.* (tilemap.set/fill edit a map, tilemap.save writes it back), nav.* (nav.bake a walkability grid, nav.path / nav.reachable / nav.nearest over it) families plus state, step, capture, log.tail, report. Several at once: `calls: [{method, params}, ...]` runs them in order and answers each (its result or its error).", obj_schema(json!({
            "method": { "type": "string" },
            "params": { "type": "object" },
            "calls": { "type": "array", "items": { "type": "object" }, "description": "several commands run in order: [{method, params}, ...]" }
        }), &[])),
        tool("runtime_commands", "List the commands the running runtime understands: without arguments their names by family; with a family (world, render, physics, ...) or a search word, one line each with its parameters and what it does.", obj_schema(json!({
            "family": { "type": "string", "description": "e.g. world, render, physics, input, ui" },
            "search": { "type": "string", "description": "a word in a command's name, parameters or summary" }
        }), &[])),
        tool("runtime_help", "How to call one command: its parameters (? optional, a | b alternatives) and what it does. A command refuses parameters it does not take and says which it takes.", obj_schema(json!({ "command": { "type": "string", "description": "e.g. world.set" } }), &["command"])),
        tool("world_tree", "The AI-native tree: one line per entity with the fields that differ from defaults.", obj_schema(json!({
            "root": { "description": "Entity id or path" },
            "depth": { "type": "integer", "default": -1 },
            "max_entities": { "type": "integer", "default": 200 },
            "values": { "type": "boolean", "default": true }
        }), &[])),
        tool("world_query", "Entities matching component and name filters, with the requested component values.", obj_schema(json!({
            "with": { "type": "array", "items": { "type": "string" } },
            "without": { "type": "array", "items": { "type": "string" } },
            "name": { "type": "string", "description": "glob on the entity name" },
            "under": { "description": "Entity id or path" },
            "fields": { "type": "array", "items": { "type": "string" } },
            "limit": { "type": "integer", "default": 200 }
        }), &[])),
        tool("world_describe", "Everything about one entity: path, parent, children, component values.", obj_schema(json!({ "entity": { "description": "Entity id, name or path" }, "path": { "type": "string", "description": "the entity's name or path, instead of entity" } }), &[])),
        tool("world_schema", "Component vocabulary: names, fields, types, docs, defaults; component or components for some, search for those that mention a word.", obj_schema(json!({
            "component": { "type": "string" },
            "components": { "type": "array", "items": { "type": "string" } },
            "search": { "type": "string" }
        }), &[])),
        tool("step", "Advance the paused simulation by N ticks and return the state summary (tick, exposed state, hashes). `until` stops at the first tick after which something holds: {event: \"coin.\"} (an event type or prefix), {state: \"score\", at_least: 3}, or {entity, component, field: \"position.y\", below: 0}, compared with equals, above, below, at_least, at_most or changes, or {where: \"Plot.stage == 3\"} (until an entity meets a world.query condition; count: n or {at_least: n} for more); the answer's `until` says whether it was met, at which tick and what was seen. `watch` follows values through the step (\"score\", \"Player:Transform.position.y\") and answers first, last, min, max with their ticks and how often each changed, instead of stepping a tick at a time; `keys` keeps only those exposed values in the answer.", obj_schema(json!({ "ticks": { "type": "integer", "default": 1 }, "until": { "type": "object", "description": "a condition that ends the step early" }, "watch": { "type": "array", "items": {}, "description": "values to follow: an exposed name, \"Entity:Component.field\", or {entity, component, field}" }, "every": { "type": "integer", "description": "also keep each watched value every n ticks" }, "keys": { "type": "array", "items": { "type": "string" }, "description": "only these exposed values in the answer" } }), &[])),
        tool("events_since", "Causal event log entries after a sequence number, oldest first.", obj_schema(json!({
            "seq": { "type": "integer", "default": 0 },
            "limit": { "type": "integer", "default": 200 },
            "type": { "type": "string", "description": "type prefix filter, e.g. 'player.'" }
        }), &[])),
        tool("capture", "Write the last frame to a PNG (and optionally the entity id buffer) and return which entities are visible; image: true also returns the frame itself, for a model that sees; ascii (true, or a width) the frame in characters and colour letters, for a model that reads.", obj_schema(json!({
            "path": { "type": "string", "description": "PNG path (default: build/mcp/capture.png in the workspace)" },
            "ascii": { "description": "true (64 across) or a width: the frame as rows of characters dark to light and rows of colour letters, with its colours by name" },
            "ids": { "type": "string", "description": "PNG path for the false-color id buffer" },
            "image": { "type": "boolean", "default": false }
        }), &[])),
        tool("asset_preview", "A model file (glTF, OBJ, STL, or one Blender reads) drawn on its own, framed from three quarters above under a sky and a sun, returned as a picture: see a model before placing it. The scene is untouched.", obj_schema(json!({
            "path": { "type": "string", "description": "project-relative model path, e.g. assets/crate.glb" },
            "size": { "type": "integer", "default": 256 }
        }), &["path"])),
        tool("look_around", "The scene, or one entity and what is under it, drawn from six sides at once (front, right, back, left, top, three quarters above) into one picture, returned as an image: check what was built from every side without moving the camera. The world is untouched.", obj_schema(json!({
            "entity": { "type": "string", "description": "an entity's name or path to frame (default: the whole scene)" },
            "views": { "type": "array", "items": { "type": "string" }, "description": "which of front, back, right, left, top, bottom, perspective (default: all but bottom)" }
        }), &[])),
        tool("render_pick", "Entity under a pixel of the last frame.", obj_schema(json!({ "x": { "type": "number" }, "y": { "type": "number" } }), &["x", "y"])),
        tool("ui_snapshot", "The interface as text: one line per element with type, id, name, rectangle, text or value, listeners, focus and scroll state. Read this instead of screenshots.", obj_schema(json!({ "depth": { "type": "integer" }, "max_nodes": { "type": "integer" }, "root": { "type": "integer" } }), &[])),
        tool("ui_query", "Find interface elements by name, text substring or type (box, text, input).", obj_schema(json!({ "name": { "type": "string" }, "text": { "type": "string" }, "type": { "type": "string" } }), &[])),
        tool("ui_click", "Click an interface element by id, or a point in window points. Goes through the same path as a player's click and returns the UI events it produced.", obj_schema(json!({ "id": { "type": "integer" }, "x": { "type": "number" }, "y": { "type": "number" } }), &[])),
        tool("ui_type", "Type text into the focused input (ui_click it first).", obj_schema(json!({ "text": { "type": "string" } }), &["text"])),
        tool("ui_key", "Press a key by name: Return, Backspace, Escape, Left, Right, Tab...", obj_schema(json!({ "key": { "type": "string" } }), &["key"])),
        tool("transcript", "The run so far compressed into segments where every exposed value keeps its trend, with the events of each segment grouped by type. Read this instead of stepping tick by tick.", obj_schema(json!({
            "since_tick": { "type": "integer", "default": 0 },
            "until_tick": { "type": "integer", "default": -1 },
            "max_lines": { "type": "integer", "default": 40 }
        }), &[])),
    ])
}

impl<'a> McpServer<'a> {
    fn text_result(value: Value, is_error: bool) -> Value {
        // Compact JSON (pretty printing nearly doubled what a model reads), cut at a bound with a
        // note on asking for less, as the pi tools do.
        const LIMIT: usize = 24000;
        let mut text = match &value {
            Value::String(s) => s.clone(),
            other => serde_json::to_string(other).unwrap_or_default(),
        };
        if text.len() > LIMIT {
            let mut cut = LIMIT;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            let rest = text.len() - cut;
            text.truncate(cut);
            text.push_str(&format!("\n[cut: {rest} more bytes; ask for less: commands {{family | search, text: true}}, help {{command}}, world.schema {{component}}, world.tree {{depth}}, world.query {{limit}}, one entity, or a batch's calls one at a time]"));
        }
        json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
    }

    fn report_result(rep: Report) -> Value {
        let ok = rep.ok;
        Self::text_result(serde_json::to_value(&rep).unwrap_or(json!({})), !ok)
    }

    /// A scenario or bench report for an agent's context: the summary says each scenario's passes,
    /// ticks, reports and failures, so a passing run keeps only its seed and ticks, and a failing one
    /// its step, error and exposed state (without the runner's own `__scenario` keys). A full run's
    /// JSON was twenty thousand tokens; `pocket scenario --json` still gives all of it.
    fn compact_runs(mut rep: Report) -> Report {
        if let Some(results) = rep.data.get_mut("results").and_then(|r| r.as_array_mut()) {
            for r in results {
                let Some(runs) = r.get_mut("runs").and_then(|v| v.as_array_mut()) else { continue };
                for run in runs.iter_mut() {
                    let ok = run.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                    let Some(o) = run.as_object_mut() else { continue };
                    if ok {
                        o.retain(|k, _| k == "seed" || k == "ticks" || k == "ok");
                    } else {
                        o.remove("bots");
                        o.remove("elapsed_ms");
                        if let Some(state) = o.get_mut("state").and_then(|s| s.as_object_mut()) {
                            state.retain(|k, _| !k.starts_with("__"));
                        }
                    }
                }
            }
        }
        rep
    }

    fn rpc(&mut self, method: &str, params: Value) -> Result<Value> {
        if self.session.is_none() {
            // A runtime someone else started and named in the environment (a benchmark, an editor).
            if let Ok(url) = std::env::var("POCKET_RPC_URL") {
                if !url.is_empty() {
                    self.session = Some(RuntimeSession { child: None, url, project: "attached".into(), stdout: None });
                }
            }
        }
        let session = self.session.as_ref().ok_or_else(|| anyhow!("no running session; call runtime_start, or runtime_attach to a running one"))?;
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }).to_string();
        let resp = http_post(&session.url, "/rpc", &body)?;
        let v: Value = serde_json::from_str(&resp).context("runtime returned invalid JSON")?;
        if let Some(err) = v.get("error") {
            bail!("{}", err.get("message").and_then(|m| m.as_str()).unwrap_or("runtime error"));
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }

    fn start_session(&mut self, args: &Value) -> Result<Value> {
        if self.session.is_some() {
            bail!("a session is already running; call runtime_stop first");
        }
        let project = args.get("project").and_then(|p| p.as_str()).ok_or_else(|| anyhow!("project is required"))?;
        let dir = crate::commands::find_project(self.ws, project).ok_or_else(|| anyhow!("unknown project '{project}'"))?;
        // The optimized runtime unless asked otherwise (docs/decisions/0006-agent-runs-release.md),
        // built first; POCKET_RUNTIME names a runtime to use as it is instead (a benchmark's copy,
        // which engine work going on in the same checkout must not rebuild under the agent).
        let exe = match std::env::var_os("POCKET_RUNTIME").filter(|p| !p.is_empty()) {
            Some(p) => std::path::PathBuf::from(p),
            None => {
                let config = args.get("config").and_then(|c| c.as_str()).unwrap_or("release");
                let outcome = crate::commands::build_targets(self.ws, config, &["pocket_runtime".to_string()], false)?;
                if !outcome.ok {
                    bail!("runtime build failed:\n{}", outcome.output);
                }
                crate::commands::exe_path(self.ws, config, "pocket_runtime")?
            }
        };
        let bundle = crate::commands::bundle_project(self.ws, &dir, None)?;
        let mut cmd = crate::commands::runtime_command(self.ws, &exe);
        // --exit-with-parent: the runtime ends with this server, so an agent that never stops it leaves nothing serving.
        cmd.arg("--project").arg(&dir).arg("--bundle").arg(&bundle.out).args(["--serve", "0", "--paused", "--json", "--log-level", "warn", "--exit-with-parent"]);
        if args.get("editor").and_then(|e| e.as_bool()).unwrap_or(false) {
            // The editor beside the project: its panes show up in ui_snapshot and its buttons
            // answer ui_click, so an agent can operate it exactly like a person.
            let editor_dir = self.ws.root.join("editor");
            if !editor_dir.join("project.toml").exists() {
                bail!("editor/project.toml is missing");
            }
            let editor_bundle = crate::commands::bundle_project(self.ws, &editor_dir, None)?;
            cmd.arg("--editor").arg(&editor_bundle.out);
        }
        if args.get("headless").and_then(|h| h.as_bool()).unwrap_or(true) {
            cmd.arg("--headless");
        }
        if let Some(seed) = args.get("seed").and_then(|s| s.as_u64()) {
            cmd.arg("--seed").arg(seed.to_string());
        }
        if let Some(size) = args.get("size").and_then(|s| s.as_str()) {
            cmd.arg("--size").arg(size);
        }
        if let Some(history) = args.get("history").and_then(|h| h.as_u64()) {
            cmd.arg("--history").arg(history.to_string());
        }
        cmd.current_dir(&self.ws.root).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().context("spawning pocket_runtime")?;
        let stderr = child.stderr.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stdout_thread = std::thread::spawn(move || {
            let mut s = String::new();
            let mut r = BufReader::new(stdout);
            let _ = r.read_to_string(&mut s);
            s
        });
        // Wait for the listening announcement on stderr.
        let mut url = None;
        let mut lines = BufReader::new(stderr).lines();
        let mut stderr_tail = vec![];
        for line in lines.by_ref() {
            let line = line?;
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                if v.get("event").and_then(|e| e.as_str()) == Some("listening") {
                    url = v.get("url").and_then(|u| u.as_str()).map(|s| s.to_string());
                    break;
                }
            }
            stderr_tail.push(line);
            if stderr_tail.len() > 50 {
                stderr_tail.remove(0);
            }
        }
        let Some(url) = url else {
            let _ = child.kill();
            bail!("runtime did not start:\n{}", stderr_tail.join("\n"));
        };
        // Drain the rest of stderr in the background so the child never blocks on a full pipe.
        std::thread::spawn(move || for _ in lines {});
        self.session = Some(RuntimeSession { child: Some(child), url: url.clone(), project: project.to_string(), stdout: Some(stdout_thread) });
        let state = self.rpc("state", json!({}))?;
        // The brief first (what the project is made of and what it is doing), as text.
        if let Some(brief) = self.rpc("project.brief", json!({})).ok().and_then(|b| b.get("text").and_then(|t| t.as_str()).map(str::to_string)) {
            return Ok(Value::String(format!("started {project} at {url}\n{brief}runtime_stop returns the final report")));
        }
        Ok(json!({ "project": project, "url": url, "state": state, "hint": "use step, world_tree, world_query, events_since, capture; runtime_stop returns the final report" }))
    }

    fn attach(&mut self, args: &Value) -> Result<Value> {
        if self.session.as_ref().map(|s| s.child.is_some()).unwrap_or(false) {
            bail!("a session this server started is running; call runtime_stop first");
        }
        let url = args.get("url").and_then(|u| u.as_str()).ok_or_else(|| anyhow!("url is required"))?.trim_end_matches('/').to_string();
        self.session = Some(RuntimeSession { child: None, url: url.clone(), project: "attached".into(), stdout: None });
        let state = self.rpc("state", json!({}))?;
        if let Some(brief) = self.rpc("project.brief", json!({})).ok().and_then(|b| b.get("text").and_then(|t| t.as_str()).map(str::to_string)) {
            return Ok(Value::String(format!("attached to {url}\n{brief}")));
        }
        Ok(json!({ "attached": url, "state": state }))
    }

    fn stop_session(&mut self, args: &Value) -> Result<Value> {
        let Some(mut session) = self.session.take() else { bail!("no running session") };
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "quit", "params": {} }).to_string();
        let Some(mut child) = session.child.take() else {
            // Attached: let it go, or stop it when asked.
            if args.get("quit").and_then(|q| q.as_bool()).unwrap_or(false) {
                let _ = http_post(&session.url, "/rpc", &body);
                return Ok(json!({ "stopped": session.url }));
            }
            return Ok(json!({ "detached": session.url }));
        };
        let _ = http_post(&session.url, "/rpc", &body);
        let status = child.wait()?;
        let out = session.stdout.take().map(|h| h.join().unwrap_or_default()).unwrap_or_default();
        let report: Value = serde_json::from_str(&out).unwrap_or(json!({ "raw": out }));
        Ok(json!({ "project": session.project, "exit_code": status.code(), "report": report }))
    }

    // A capture, with the frame itself as an image block when asked.
    fn capture(&mut self, mut args: Value) -> Result<Value> {
        let want_image = args.get("image").and_then(|i| i.as_bool()).unwrap_or(false);
        if args.get("path").and_then(|p| p.as_str()).is_none() {
            let dir = self.ws.root.join("build").join("mcp");
            std::fs::create_dir_all(&dir)?;
            args["path"] = json!(dir.join("capture.png").to_string_lossy());
        }
        let path = args["path"].as_str().unwrap_or_default().to_string();
        if let Some(o) = args.as_object_mut() {
            o.remove("image");   // the tool's own option, not the command's
        }
        let v = self.rpc("capture", args)?;
        let path = v.get("path").and_then(|p| p.as_str()).map(String::from).unwrap_or(path);   // a relative path lands under the project
        let text = serde_json::to_string(&v).unwrap_or_default();
        let mut content = vec![json!({ "type": "text", "text": text })];
        if want_image {
            let bytes = std::fs::read(&path).with_context(|| format!("reading {path}"))?;
            content.push(json!({ "type": "image", "data": base64(&bytes), "mimeType": "image/png" }));
        }
        Ok(json!({ "content": content, "isError": false }))
    }

    fn call_tool(&mut self, name: &str, args: Value) -> Result<Value> {
        let s = |k: &str| args.get(k).and_then(|v| v.as_str()).map(|s| s.to_string());
        match name {
            "pocket_doctor" => Ok(Self::report_result(crate::commands::doctor(self.ws)?)),
            "pocket_build" => {
                let targets: Vec<String> = args.get("targets").and_then(|t| t.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
                Ok(Self::report_result(crate::commands::build(self.ws, s("config").as_deref().unwrap_or("debug"), &targets, false)?))
            }
            "pocket_test" => Ok(Self::report_result(crate::commands::test(self.ws, s("config").as_deref().unwrap_or("debug"), s("filter").as_deref())?)),
            "pocket_check" => {
                let project = s("project").map(|p| { let direct = std::path::PathBuf::from(&p); if direct.exists() { direct } else { self.ws.root.join("samples").join(&p) } });
                Ok(Self::report_result(crate::check::check(self.ws, project.as_deref())?))
            }
            "docs_search" => {
                let query = s("query").ok_or_else(|| anyhow!("query is required"))?;
                let rep = crate::docs::search(self.ws, &query, args.get("limit").and_then(|v| v.as_u64()).unwrap_or(3) as usize)?;
                Ok(Self::text_result(Value::String(crate::docs::as_text(&rep).trim().to_string().chars().take(24000).collect::<String>()).to_owned(), !rep.ok))
            }
            "pocket_gen" => Ok(Self::report_result(crate::commands::gen(self.ws, args.get("check").and_then(|c| c.as_bool()).unwrap_or(false))?)),
            "pocket_scenario" => {
                let project = s("project").ok_or_else(|| anyhow!("project is required"))?;
                let rep = crate::commands::scenario(self.ws, s("config").as_deref().unwrap_or("release"), &project, s("file").as_deref(), args.get("seeds").and_then(|v| v.as_u64()).unwrap_or(5), args.get("frames").and_then(|v| v.as_i64()).unwrap_or(1800), s("only").as_deref())?;
                Ok(Self::report_result(Self::compact_runs(rep)))
            }
            "pocket_bench" => {
                let project = s("project").ok_or_else(|| anyhow!("project is required"))?;
                let rep = crate::commands::bench(self.ws, s("config").as_deref().unwrap_or("release"), &project, s("file").as_deref(), args.get("frames").and_then(|v| v.as_i64()).unwrap_or(1800), s("only").as_deref())?;
                Ok(Self::report_result(Self::compact_runs(rep)))
            }
            "pocket_run_headless" => {
                let project = s("project").ok_or_else(|| anyhow!("project is required"))?;
                let mut extra = vec!["--headless".to_string(), "--json".to_string(), "--frames".to_string(), args.get("frames").and_then(|f| f.as_i64()).unwrap_or(120).to_string(), "--log-level".to_string(), "warn".to_string()];
                if let Some(c) = s("capture") { extra.push("--capture".into()); extra.push(c); }
                if let Some(seed) = args.get("seed").and_then(|v| v.as_u64()) { extra.push("--seed".into()); extra.push(seed.to_string()); }
                if let Some(size) = s("size") { extra.push("--size".into()); extra.push(size); }
                let rep = crate::commands::run_captured(self.ws, s("config").as_deref().unwrap_or("release"), &project, &extra)?;
                Ok(Self::report_result(rep))
            }
            "runtime_start" => self.start_session(&args).map(|v| Self::text_result(v, false)),
            "runtime_stop" => self.stop_session(&args).map(|v| Self::text_result(v, false)),
            "runtime_attach" => self.attach(&args).map(|v| Self::text_result(v, false)),
            "project_apply" => self.rpc("project.apply", args).map(|v| Self::text_result(v, false)),
            "runtime_command" => {
                if let Some(calls) = args.get("calls").and_then(|c| c.as_array()).filter(|c| !c.is_empty()) {
                    // In order, each answered on its own: one failing call does not hide the others.
                    let mut out = vec![];
                    for c in calls {
                        let method = c.get("method").and_then(|m| m.as_str()).unwrap_or("").to_string();
                        let params = c.get("params").cloned().unwrap_or(json!({}));
                        out.push(match self.rpc(&method, params) {
                            Ok(v) => json!({ "method": method, "result": v }),
                            Err(e) => json!({ "method": method, "error": format!("{e:#}") }),
                        });
                    }
                    return Ok(Self::text_result(Value::Array(out), false));
                }
                let method = s("method").ok_or_else(|| anyhow!("method (or calls) is required"))?;
                let params = args.get("params").cloned().unwrap_or(json!({}));
                self.rpc(&method, params).map(|v| Self::text_result(v, false))
            }
            "runtime_commands" => {
                // One line a command, narrowed by family or search when given.
                let mut params = json!({ "text": true });
                for k in ["family", "search"] { if let Some(v) = args.get(k) { params[k] = v.clone(); } }
                self.rpc("commands", params).map(|v| Self::text_result(v.get("text").cloned().unwrap_or(v), false))
            }
            "runtime_help" => self.rpc("help", args).map(|v| Self::text_result(v, false)),
            "world_tree" => self.rpc("world.tree", args).map(|v| Self::text_result(v.get("text").cloned().unwrap_or(v), false)),
            "world_query" => self.rpc("world.query", args).map(|v| Self::text_result(v, false)),
            "world_describe" => self.rpc("world.describe", args).map(|v| Self::text_result(v, false)),
            "world_schema" => self.rpc("world.schema", args).map(|v| Self::text_result(v, false)),
            "step" => self.rpc("step", args).map(|v| Self::text_result(v, false)),
            "events_since" => self.rpc("events.since", args).map(|v| Self::text_result(v, false)),
            "capture" => self.capture(args),
            "asset_preview" => {
                let mut a = args.clone();
                a["image"] = json!(true);
                let mut v = self.rpc("assets.preview", a)?;
                let png = v.get("png").and_then(|p| p.as_str()).unwrap_or_default().to_string();
                if let Some(o) = v.as_object_mut() { o.remove("png"); }
                let mut content = vec![json!({ "type": "text", "text": serde_json::to_string(&v).unwrap_or_default() })];
                if !png.is_empty() { content.push(json!({ "type": "image", "data": png, "mimeType": "image/png" })); }
                Ok(json!({ "content": content, "isError": false }))
            }
            "look_around" => {
                let dir = self.ws.root.join("build").join("mcp");
                std::fs::create_dir_all(&dir)?;
                let path = dir.join("views.png").to_string_lossy().to_string();
                let mut a = args.clone();
                a["path"] = json!(path);
                let v = self.rpc("render.views", a)?;
                let bytes = std::fs::read(&path).with_context(|| format!("reading {path}"))?;
                Ok(json!({ "content": [
                    { "type": "text", "text": serde_json::to_string(&v).unwrap_or_default() },
                    { "type": "image", "data": base64(&bytes), "mimeType": "image/png" }
                ], "isError": false }))
            }
            "render_pick" => self.rpc("render.pick", args).map(|v| Self::text_result(v, false)),
            "ui_snapshot" => self.rpc("ui.snapshot", args).map(|v| Self::text_result(v.get("text").cloned().unwrap_or(v), false)),
            "ui_query" => self.rpc("ui.query", args).map(|v| Self::text_result(v, false)),
            "ui_click" => self.rpc("ui.click", args).map(|v| Self::text_result(v.get("events").cloned().unwrap_or(v), false)),
            "ui_type" => self.rpc("ui.type", args).map(|v| Self::text_result(v.get("events").cloned().unwrap_or(v), false)),
            "ui_key" => self.rpc("ui.key", args).map(|v| Self::text_result(v.get("events").cloned().unwrap_or(v), false)),
            "transcript" => self.rpc("transcript", args).map(|v| Self::text_result(v.get("text").cloned().unwrap_or(v), false)),
            other => bail!("unknown tool '{other}'"),
        }
    }

    fn handle(&mut self, msg: &Value) -> Option<Value> {
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(json!({}));
        let result: Result<Value> = match method {
            "initialize" => Ok(json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "pocket", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Pocket is an agent-first game engine. Build with pocket_build, test with pocket_test, run a project with pocket_run_headless, or start a paused session with runtime_start and then step, world_tree, world_query, events_since, capture, render_pick. Entities are addressed by id or path such as /Level/Player."
            })),
            "notifications/initialized" | "notifications/cancelled" => return None,
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools_list() })),
            "tools/call" => {
                let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                match self.call_tool(name, args) {
                    Ok(v) => Ok(v),
                    Err(e) => Ok(Self::text_result(json!(format!("{e:#}")), true)),
                }
            }
            "resources/list" => Ok(json!({ "resources": [] })),
            "prompts/list" => Ok(json!({ "prompts": [] })),
            _ => Err(anyhow!("method not found: {method}")),
        };
        id.as_ref()?;
        Some(match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("{e:#}") } }),
        })
    }
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// `pocket rpc <method> [params]`: one command to a running runtime, its result printed as JSON (a
/// text result such as world.tree's printed as the text), for any agent that can run a shell.
pub fn rpc_cli(method: &str, params: Option<&str>, url: Option<&str>) -> i32 {
    let Some(url) = url.map(|s| s.to_string()).or_else(|| std::env::var("POCKET_RPC_URL").ok()).filter(|u| !u.is_empty()) else {
        eprintln!("no runtime: pass --url or set POCKET_RPC_URL (start one with `pocket run <project> -- --serve 4711 --paused`)");
        return 2;
    };
    let params: Value = match params {
        None => json!({}),
        Some(p) => match serde_json::from_str(p) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("params are not JSON: {e}");
                return 2;
            }
        },
    };
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }).to_string();
    let reply = http_post(&url, "/rpc", &body).and_then(|r| serde_json::from_str::<Value>(&r).map_err(|e| anyhow!("the runtime's reply is not JSON: {e}")));
    match reply {
        Ok(v) => {
            if let Some(err) = v.get("error") {
                eprintln!("{}", err.get("message").and_then(|m| m.as_str()).unwrap_or("runtime error"));
                println!("{}", serde_json::to_string_pretty(err).unwrap_or_default());
                return 1;
            }
            let result = v.get("result").cloned().unwrap_or(Value::Null);
            match result.as_object().and_then(|o| if o.len() <= 3 { o.get("text") } else { None }).and_then(|t| t.as_str()) {
                Some(text) => println!("{text}"),
                None => println!("{}", serde_json::to_string_pretty(&result).unwrap_or_default()),
            }
            0
        }
        Err(e) => {
            eprintln!("{e:#}");
            2
        }
    }
}

pub(crate) fn http_post(base: &str, path: &str, body: &str) -> Result<String> {
    let host = base.trim_start_matches("http://").trim_end_matches('/');
    let mut stream = TcpStream::connect(host).with_context(|| format!("connecting to {host}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(600)))?;
    write!(stream, "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let text = String::from_utf8_lossy(&raw);
    let (headers, body) = text.split_once("\r\n\r\n").ok_or_else(|| anyhow!("malformed HTTP response"))?;
    let status = headers.lines().next().unwrap_or("");
    if !status.contains("200") && !status.contains("400") {
        bail!("runtime HTTP error: {status}");
    }
    Ok(body.to_string())
}

pub fn serve(ws: &Workspace) -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut server = McpServer { ws, session: None };
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let err = json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("parse error: {e}") } });
                writeln!(stdout, "{err}")?;
                stdout.flush()?;
                continue;
            }
        };
        if let Some(resp) = server.handle(&msg) {
            writeln!(stdout, "{resp}")?;
            stdout.flush()?;
        }
    }
    if server.session.is_some() {
        let _ = server.stop_session(&json!({}));   // a runtime it started stops; an attached one is let go
    }
    Ok(())
}
