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
    child: Child,
    url: String,
    project: String,
    stdout: std::thread::JoinHandle<String>,
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
        tool("pocket_gen", "Regenerate code from component metadata (engine/*/meta/*.toml): C++, TypeScript, docs.", obj_schema(json!({ "check": { "type": "boolean", "default": false } }), &[])),
        tool("pocket_scenario", "Run a project's gameplay scenarios (scenarios/*.ts: play through actions, wait for outcomes, check state) at several seeds and return pass counts, ticks to pass and each failure's reason.", obj_schema(json!({
            "project": { "type": "string", "description": "Project name under samples/ or a directory with project.toml" },
            "file": { "type": "string", "description": "One scenario file instead of every file under scenarios/" },
            "seeds": { "type": "integer", "default": 5 },
            "frames": { "type": "integer", "default": 1800, "description": "frame budget per run" },
            "only": { "type": "string", "description": "substring filter on scenario names" }
        }), &["project"])),
        tool("pocket_bench", "Run a project's perception benchmarks (benches/*.ts: gameplay questions answered through the instruments) and return, per question, whether the answer was correct, the tokens it cost and what frame-by-frame vision would have cost.", obj_schema(json!({
            "project": { "type": "string", "description": "Project name under samples/ or a directory with project.toml" },
            "file": { "type": "string", "description": "One benchmark file instead of every file under benches/" },
            "frames": { "type": "integer", "default": 1800, "description": "frame budget per run" },
            "only": { "type": "string", "description": "substring filter on questions" }
        }), &["project"])),
        tool("pocket_run_headless", "Run a project headless for N frames and return its JSON report (exposed state, state hash, world summary, events, optional capture).", obj_schema(json!({
            "project": { "type": "string", "description": "Project name under samples/ or a directory with project.toml" },
            "frames": { "type": "integer", "default": 120 },
            "capture": { "type": "string", "description": "PNG path for the last frame" },
            "seed": { "type": "integer" },
            "size": { "type": "string", "description": "WxH, e.g. 640x360" }
        }), &["project"])),
        tool("runtime_start", "Start a project in a paused runtime with the control server, so it can be stepped and inspected. One session at a time.", obj_schema(json!({
            "project": { "type": "string" },
            "headless": { "type": "boolean", "default": true, "description": "false opens a window" },
            "editor": { "type": "boolean", "default": false, "description": "open the project in the Pocket editor (hierarchy, inspector, play/stop) and operate it through ui_* tools" },
            "seed": { "type": "integer" },
            "size": { "type": "string", "description": "WxH render target size" },
            "history": { "type": "integer", "description": "keep the last N ticks for recorder.at/diff/track/first (time travel)" }
        }), &["project"])),
        tool("runtime_stop", "Stop the running session and return its final JSON report.", obj_schema(json!({}), &[])),
        tool("runtime_command", "Send any runtime command with JSON params. Use runtime_commands to list them; the world.*, events.* (events.why explains an event by its causes), recorder.* (time travel when the session started with history), render.* (render.visible: what the camera sees; render.unproject: the world point under a pixel), tilemap.* (tilemap.set/fill edit a map, tilemap.save writes it back), nav.* (nav.bake a walkability grid, nav.path / nav.reachable / nav.nearest over it) families plus state, step, capture, log.tail, report.", obj_schema(json!({
            "method": { "type": "string" },
            "params": { "type": "object" }
        }), &["method"])),
        tool("runtime_commands", "List every command the running runtime understands.", obj_schema(json!({}), &[])),
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
        tool("world_describe", "Everything about one entity: path, parent, children, component values.", obj_schema(json!({ "entity": { "description": "Entity id or path" } }), &["entity"])),
        tool("world_schema", "Component vocabulary: names, fields, types, docs, defaults.", obj_schema(json!({}), &[])),
        tool("step", "Advance the paused simulation by N ticks and return the state summary (tick, exposed state, hashes).", obj_schema(json!({ "ticks": { "type": "integer", "default": 1 } }), &[])),
        tool("events_since", "Causal event log entries after a sequence number, oldest first.", obj_schema(json!({
            "seq": { "type": "integer", "default": 0 },
            "limit": { "type": "integer", "default": 200 },
            "type": { "type": "string", "description": "type prefix filter, e.g. 'player.'" }
        }), &[])),
        tool("capture", "Write the last frame to a PNG (and optionally the entity id buffer) and return which entities are visible.", obj_schema(json!({
            "path": { "type": "string" },
            "ids": { "type": "string", "description": "PNG path for the false-color id buffer" }
        }), &["path"])),
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
        let text = match &value {
            Value::String(s) => s.clone(),
            other => serde_json::to_string_pretty(other).unwrap_or_default(),
        };
        json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
    }

    fn report_result(rep: Report) -> Value {
        let ok = rep.ok;
        Self::text_result(serde_json::to_value(&rep).unwrap_or(json!({})), !ok)
    }

    fn rpc(&mut self, method: &str, params: Value) -> Result<Value> {
        let session = self.session.as_ref().ok_or_else(|| anyhow!("no running session; call runtime_start first"))?;
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
        let outcome = crate::commands::build_targets(self.ws, "debug", &["pocket_runtime".to_string()], false)?;
        if !outcome.ok {
            bail!("runtime build failed:\n{}", outcome.output);
        }
        let bundle = crate::commands::bundle_project(self.ws, &dir, None)?;
        let exe = crate::commands::exe_path(self.ws, "debug", "pocket_runtime")?;
        let mut cmd = crate::toolchain::command(exe.to_str().unwrap());
        cmd.arg("--project").arg(&dir).arg("--bundle").arg(&bundle.out).args(["--serve", "0", "--paused", "--json", "--log-level", "warn"]);
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
        self.session = Some(RuntimeSession { child, url: url.clone(), project: project.to_string(), stdout: stdout_thread });
        let state = self.rpc("state", json!({}))?;
        Ok(json!({ "project": project, "url": url, "state": state, "hint": "use step, world_tree, world_query, events_since, capture; runtime_stop returns the final report" }))
    }

    fn stop_session(&mut self) -> Result<Value> {
        let Some(mut session) = self.session.take() else { bail!("no running session") };
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "quit", "params": {} }).to_string();
        let _ = http_post(&session.url, "/rpc", &body);
        let status = session.child.wait()?;
        let out = session.stdout.join().unwrap_or_default();
        let report: Value = serde_json::from_str(&out).unwrap_or(json!({ "raw": out }));
        Ok(json!({ "project": session.project, "exit_code": status.code(), "report": report }))
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
            "pocket_gen" => Ok(Self::report_result(crate::commands::gen(self.ws, args.get("check").and_then(|c| c.as_bool()).unwrap_or(false))?)),
            "pocket_scenario" => {
                let project = s("project").ok_or_else(|| anyhow!("project is required"))?;
                let rep = crate::commands::scenario(self.ws, "debug", &project, s("file").as_deref(), args.get("seeds").and_then(|v| v.as_u64()).unwrap_or(5), args.get("frames").and_then(|v| v.as_i64()).unwrap_or(1800), s("only").as_deref())?;
                Ok(Self::report_result(rep))
            }
            "pocket_bench" => {
                let project = s("project").ok_or_else(|| anyhow!("project is required"))?;
                let rep = crate::commands::bench(self.ws, "debug", &project, s("file").as_deref(), args.get("frames").and_then(|v| v.as_i64()).unwrap_or(1800), s("only").as_deref())?;
                Ok(Self::report_result(rep))
            }
            "pocket_run_headless" => {
                let project = s("project").ok_or_else(|| anyhow!("project is required"))?;
                let mut extra = vec!["--headless".to_string(), "--json".to_string(), "--frames".to_string(), args.get("frames").and_then(|f| f.as_i64()).unwrap_or(120).to_string(), "--log-level".to_string(), "warn".to_string()];
                if let Some(c) = s("capture") { extra.push("--capture".into()); extra.push(c); }
                if let Some(seed) = args.get("seed").and_then(|v| v.as_u64()) { extra.push("--seed".into()); extra.push(seed.to_string()); }
                if let Some(size) = s("size") { extra.push("--size".into()); extra.push(size); }
                let rep = crate::commands::run_captured(self.ws, "debug", &project, &extra)?;
                Ok(Self::report_result(rep))
            }
            "runtime_start" => self.start_session(&args).map(|v| Self::text_result(v, false)),
            "runtime_stop" => self.stop_session().map(|v| Self::text_result(v, false)),
            "runtime_command" => {
                let method = s("method").ok_or_else(|| anyhow!("method is required"))?;
                let params = args.get("params").cloned().unwrap_or(json!({}));
                self.rpc(&method, params).map(|v| Self::text_result(v, false))
            }
            "runtime_commands" => self.rpc("commands", json!({})).map(|v| Self::text_result(v, false)),
            "world_tree" => self.rpc("world.tree", args).map(|v| Self::text_result(v.get("text").cloned().unwrap_or(v), false)),
            "world_query" => self.rpc("world.query", args).map(|v| Self::text_result(v, false)),
            "world_describe" => self.rpc("world.describe", args).map(|v| Self::text_result(v, false)),
            "world_schema" => self.rpc("world.schema", json!({})).map(|v| Self::text_result(v, false)),
            "step" => self.rpc("step", args).map(|v| Self::text_result(v, false)),
            "events_since" => self.rpc("events.since", args).map(|v| Self::text_result(v, false)),
            "capture" => self.rpc("capture", args).map(|v| Self::text_result(v, false)),
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
        let _ = server.stop_session();
    }
    Ok(())
}
