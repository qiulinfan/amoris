//! One CDP client: its requests in the `Debugger` and `Runtime` domains, and the hub's events as
//! CDP events. Positions on the wire are the scripts' JavaScript (0-based); the client maps them to
//! TypeScript through the source maps `Debugger.scriptParsed` gives.

use std::collections::BTreeSet;
use std::net::TcpStream;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::{Value as Json, json};
use tungstenite::{Message, WebSocket};

use crate::hub::{ClientId, DebugHub, Target};
use crate::model::{Command, DebugEvent, ExceptionMode, StepKind};

/// How long a session's read waits before it looks at the hub's events.
const POLL: Duration = Duration::from_millis(4);

pub(crate) struct Session {
    hub: DebugHub,
    client: ClientId,
    events: Receiver<DebugEvent>,
    debugger: bool,
    runtime: bool,
    log: bool,
    /// Breakpoints this session set (for `Debugger.breakpointResolved`).
    breakpoints: BTreeSet<String>,
    /// Messages to send after the response being built.
    after: Vec<Json>,
    /// Messages to send before it (V8 sends `scriptParsed` before `Debugger.enable` answers).
    before: Vec<Json>,
    started: Instant,
}

impl Session {
    pub(crate) fn new(hub: DebugHub, log: bool) -> Session {
        let client = hub.open_client("cdp");
        let events = hub.subscribe();
        Session {
            hub,
            client,
            events,
            debugger: false,
            runtime: false,
            log,
            breakpoints: BTreeSet::new(),
            after: Vec::new(),
            before: Vec::new(),
            started: Instant::now(),
        }
    }

    pub(crate) fn run(mut self, mut ws: WebSocket<TcpStream>) {
        let _ = ws.get_mut().set_read_timeout(Some(POLL));
        if self.log {
            eprintln!("pocket-cdp: client {} attached", self.client);
        }
        'session: loop {
            match ws.read() {
                Ok(Message::Text(text)) => {
                    let out = self.handle(text.as_str());
                    for m in out {
                        if self.send(&mut ws, &m).is_err() {
                            break 'session;
                        }
                    }
                }
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => break,
            }
            while let Ok(event) = self.events.try_recv() {
                for m in self.event(event) {
                    if self.send(&mut ws, &m).is_err() {
                        break 'session;
                    }
                }
            }
        }
        if self.log {
            eprintln!("pocket-cdp: client {} detached", self.client);
        }
        self.hub.close_client(self.client);
    }

    fn send(&self, ws: &mut WebSocket<TcpStream>, m: &Json) -> Result<(), ()> {
        let text = m.to_string();
        if self.log {
            eprintln!("pocket-cdp -> {}", truncate(&text));
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut pending = Some(Message::text(text));
        loop {
            let r = match pending.take() {
                Some(msg) => ws.send(msg),
                None => ws.flush(),
            };
            match r {
                Ok(()) => return Ok(()),
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    if Instant::now() > deadline {
                        return Err(());
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(_) => return Err(()),
            }
        }
    }

    /// One request in; the messages to send out (events before, the response, events after).
    fn handle(&mut self, text: &str) -> Vec<Json> {
        if self.log {
            eprintln!("pocket-cdp <- {}", truncate(text));
        }
        let Ok(msg) = serde_json::from_str::<Json>(text) else {
            return Vec::new();
        };
        let Some(id) = msg.get("id").cloned() else {
            return Vec::new();
        };
        let method = msg["method"].as_str().unwrap_or("").to_owned();
        let p = msg.get("params").cloned().unwrap_or(json!({}));
        let mut response = match self.dispatch(&method, &p) {
            Ok(result) => json!({"id": id, "result": result}),
            Err(message) => json!({"id": id, "error": {"code": -32000, "message": message}}),
        };
        if let Some(s) = msg.get("sessionId") {
            response["sessionId"] = s.clone();
        }
        let mut out = std::mem::take(&mut self.before);
        out.push(response);
        out.append(&mut self.after);
        out
    }

    fn event(&mut self, e: DebugEvent) -> Vec<Json> {
        let ev = |method: &str, params: Json| json!({"method": method, "params": params});
        match e {
            DebugEvent::Scripts(scripts) if self.debugger => scripts
                .iter()
                .map(|s| ev("Debugger.scriptParsed", s.script_parsed()))
                .collect(),
            DebugEvent::BreakpointResolved { id, location }
                if self.debugger && self.breakpoints.contains(&id) =>
            {
                vec![ev(
                    "Debugger.breakpointResolved",
                    json!({"breakpointId": id, "location": location.cdp()}),
                )]
            }
            DebugEvent::Paused(p) if self.debugger => vec![ev("Debugger.paused", p.cdp())],
            DebugEvent::Resumed if self.debugger => vec![ev("Debugger.resumed", json!({}))],
            DebugEvent::Console(c) if self.runtime => {
                let kind = match c.level.as_str() {
                    "warn" => "warning",
                    "error" => "error",
                    "info" => "info",
                    "debug" => "debug",
                    _ => "log",
                };
                let mut stack = Vec::new();
                if let Some(l) = &c.location {
                    stack.push(json!({"functionName": c.function, "scriptId": l.script.id,
                                      "url": l.script.url, "lineNumber": l.js.0, "columnNumber": l.js.1}));
                }
                vec![ev(
                    "Runtime.consoleAPICalled",
                    json!({
                        "type": kind,
                        "args": [{"type": "string", "value": c.text}],
                        "executionContextId": 1,
                        "timestamp": self.started.elapsed().as_secs_f64() * 1000.0,
                        "stackTrace": {"callFrames": stack},
                    }),
                )]
            }
            DebugEvent::Exception(x) if self.runtime => {
                let mut details = json!({
                    "exceptionId": 1,
                    "text": format!("Uncaught ({} in tick {}): {}", x.system, x.tick, x.message),
                    "lineNumber": 0,
                    "columnNumber": 0,
                    "executionContextId": 1,
                    "exception": {"type": "object", "subtype": "error", "className": "Error",
                                  "description": x.message},
                });
                if let Some(l) = &x.location {
                    details["scriptId"] = json!(l.script.id);
                    details["url"] = json!(l.script.url);
                    details["lineNumber"] = json!(l.js.0);
                    details["columnNumber"] = json!(l.js.1);
                    details["stackTrace"] = json!({"callFrames": [{"functionName": x.system,
                        "scriptId": l.script.id, "url": l.script.url, "lineNumber": l.js.0,
                        "columnNumber": l.js.1}]});
                }
                vec![ev(
                    "Runtime.exceptionThrown",
                    json!({
                        "timestamp": self.started.elapsed().as_secs_f64() * 1000.0,
                        "exceptionDetails": details,
                    }),
                )]
            }
            _ => Vec::new(),
        }
    }

    fn dispatch(&mut self, method: &str, p: &Json) -> Result<Json, String> {
        let hub = self.hub.clone();
        match method {
            "Runtime.enable" => {
                self.runtime = true;
                self.before.push(
                    json!({"method": "Runtime.executionContextCreated", "params": {"context": {
                    "id": 1, "origin": "", "name": hub.inner.title, "uniqueId": "pocket-game-1",
                    "auxData": {"isDefault": true}}}}),
                );
                Ok(json!({}))
            }
            "Runtime.disable" => {
                self.runtime = false;
                Ok(json!({}))
            }
            "Debugger.enable" => {
                self.debugger = true;
                hub.update_client(self.client, |c| c.enabled = true);
                let (scripts, pause) = {
                    let st = hub.state();
                    let s: Vec<Json> = st.registry.current().map(|s| s.script_parsed()).collect();
                    (s, st.pause.clone())
                };
                for s in scripts {
                    self.before
                        .push(json!({"method": "Debugger.scriptParsed", "params": s}));
                }
                if let Some(pause) = pause {
                    self.after
                        .push(json!({"method": "Debugger.paused", "params": pause.cdp()}));
                }
                Ok(json!({"debuggerId": "pocket-game-debugger"}))
            }
            "Debugger.disable" => {
                self.debugger = false;
                hub.update_client(self.client, |c| c.enabled = false);
                Ok(json!({}))
            }
            "Runtime.runIfWaitingForDebugger" => {
                hub.release();
                Ok(json!({}))
            }
            "Debugger.setPauseOnExceptions" => {
                let state = p["state"].as_str().unwrap_or("none");
                let mode = ExceptionMode::parse(state)
                    .ok_or_else(|| format!("Unknown state '{state}'."))?;
                hub.update_client(self.client, |c| c.exceptions = mode);
                Ok(json!({}))
            }
            "Debugger.setBreakpointsActive" => {
                let active = p["active"].as_bool().unwrap_or(true);
                hub.update_client(self.client, |c| c.active = active);
                Ok(json!({}))
            }
            "Debugger.setSkipAllPauses" => {
                let skip = p["skip"].as_bool().unwrap_or(false);
                hub.update_client(self.client, |c| c.skip = skip);
                Ok(json!({}))
            }
            "Debugger.setBreakpointByUrl" => {
                let line = p["lineNumber"].as_u64().ok_or("lineNumber is required")? as u32;
                let target = if let Some(u) = p["url"].as_str() {
                    Target::Url(u.to_owned())
                } else if let Some(r) = p["urlRegex"].as_str() {
                    Target::Regex(Regex::new(r).map_err(|e| format!("urlRegex: {e}"))?)
                } else if let Some(h) = p["scriptHash"].as_str() {
                    let id = hub
                        .state()
                        .registry
                        .current()
                        .find(|s| s.hash == h)
                        .map(|s| s.id.clone());
                    Target::Script(id.unwrap_or_default())
                } else {
                    return Err("setBreakpointByUrl needs url, urlRegex or scriptHash.".into());
                };
                let (id, resolved) =
                    hub.add_breakpoint(self.client, target, line, condition(p), None);
                self.breakpoints.insert(id.clone());
                let locations: Vec<Json> = resolved.iter().map(|r| r.shown.cdp()).collect();
                Ok(json!({"breakpointId": id, "locations": locations}))
            }
            "Debugger.setBreakpoint" => {
                let loc = &p["location"];
                let script = loc["scriptId"]
                    .as_str()
                    .ok_or("location.scriptId is required")?
                    .to_owned();
                let line = loc["lineNumber"]
                    .as_u64()
                    .ok_or("location.lineNumber is required")? as u32;
                let (id, resolved) = hub.add_breakpoint(
                    self.client,
                    Target::Script(script),
                    line,
                    condition(p),
                    None,
                );
                let Some(r) = resolved.first() else {
                    hub.remove_breakpoint(Some(self.client), &id);
                    return Err("Could not resolve breakpoint".into());
                };
                self.breakpoints.insert(id.clone());
                Ok(json!({"breakpointId": id, "actualLocation": r.shown.cdp()}))
            }
            "Debugger.removeBreakpoint" => {
                let id = p["breakpointId"].as_str().unwrap_or("");
                hub.remove_breakpoint(Some(self.client), id);
                self.breakpoints.remove(id);
                Ok(json!({}))
            }
            "Debugger.getPossibleBreakpoints" => {
                let start = &p["start"];
                let script_id = start["scriptId"]
                    .as_str()
                    .ok_or("start.scriptId is required")?;
                let script = hub
                    .state()
                    .registry
                    .by_id(script_id)
                    .cloned()
                    .ok_or("No such script.")?;
                let from = start["lineNumber"].as_u64().unwrap_or(0) as u32;
                // `end` is exclusive.
                let to = p["end"]["lineNumber"]
                    .as_u64()
                    .map(|l| {
                        (l as u32).saturating_sub(
                            if p["end"]["columnNumber"].as_u64().unwrap_or(0) == 0 {
                                1
                            } else {
                                0
                            },
                        )
                    })
                    .unwrap_or(from);
                let locations: Vec<Json> = script
                    .js_code_positions(from, to.max(from))
                    .into_iter()
                    .map(
                        |(l, c)| json!({"scriptId": script.id, "lineNumber": l, "columnNumber": c}),
                    )
                    .collect();
                Ok(json!({"locations": locations}))
            }
            "Debugger.getScriptSource" => {
                let id = p["scriptId"].as_str().unwrap_or("");
                let script = hub
                    .state()
                    .registry
                    .by_id(id)
                    .cloned()
                    .ok_or("No such script.")?;
                Ok(json!({"scriptSource": script.js}))
            }
            "Debugger.pause" => {
                if !hub.is_paused() {
                    hub.request_pause();
                }
                Ok(json!({}))
            }
            "Debugger.resume" => hub
                .resume(None)
                .map(|()| json!({}))
                .map_err(|_| paused_only()),
            "Debugger.stepOver" => hub
                .resume(Some(StepKind::Over))
                .map(|()| json!({}))
                .map_err(|_| paused_only()),
            "Debugger.stepInto" => hub
                .resume(Some(StepKind::Into))
                .map(|()| json!({}))
                .map_err(|_| paused_only()),
            "Debugger.stepOut" => hub
                .resume(Some(StepKind::Out))
                .map(|()| json!({}))
                .map_err(|_| paused_only()),
            "Debugger.evaluateOnCallFrame" => {
                let frame = p["callFrameId"]
                    .as_str()
                    .and_then(|f| f.parse().ok())
                    .ok_or("callFrameId")?;
                let expression = p["expression"]
                    .as_str()
                    .ok_or("expression is required")?
                    .to_owned();
                let by_value = p["returnByValue"].as_bool().unwrap_or(false);
                hub.ask(|reply| Command::Evaluate {
                    frame,
                    expression,
                    by_value,
                    json: false,
                    reply,
                })
            }
            "Debugger.setVariableValue" => {
                let frame = p["callFrameId"]
                    .as_str()
                    .and_then(|f| f.parse().ok())
                    .ok_or("callFrameId")?;
                let name = p["variableName"]
                    .as_str()
                    .ok_or("variableName is required")?
                    .to_owned();
                let value = p["newValue"].clone();
                hub.ask(|reply| Command::SetVariable {
                    frame,
                    name,
                    value,
                    reply,
                })
            }
            "Runtime.getProperties" => {
                let object_id = p["objectId"]
                    .as_str()
                    .ok_or("objectId is required")?
                    .to_owned();
                if p["accessorPropertiesOnly"].as_bool() == Some(true) {
                    return Ok(json!({"result": []}));
                }
                if !hub.is_paused() {
                    return Ok(json!({"result": []}));
                }
                hub.ask(|reply| Command::Properties { object_id, reply })
            }
            "Runtime.callFunctionOn" => {
                // js-debug treats an answer without `result` as a failure of a whole listing.
                if !hub.is_paused() {
                    return Ok(json!({"result": {"type": "undefined"}}));
                }
                let object_id = p["objectId"].as_str().map(str::to_owned);
                let declaration = p["functionDeclaration"]
                    .as_str()
                    .ok_or("functionDeclaration is required")?
                    .to_owned();
                let arguments = p["arguments"].as_array().cloned().unwrap_or_default();
                let by_value = p["returnByValue"].as_bool().unwrap_or(false);
                hub.ask(|reply| Command::CallFunctionOn {
                    object_id,
                    declaration,
                    arguments,
                    by_value,
                    reply,
                })
            }
            "Runtime.evaluate" => {
                let expression = p["expression"].as_str().unwrap_or("").to_owned();
                match hub
                    .pause_info()
                    .and_then(|pz| pz.frames.first().filter(|f| !f.returned).map(|f| f.level))
                {
                    Some(frame) => {
                        let by_value = p["returnByValue"].as_bool().unwrap_or(false);
                        hub.ask(|reply| Command::Evaluate {
                            frame,
                            expression,
                            by_value,
                            json: false,
                            reply,
                        })
                    }
                    None if p["objectGroup"].as_str() == Some("console")
                        || p["replMode"].as_bool() == Some(true) =>
                    {
                        let text = "Pocket3D: the game thread is running; pause it to evaluate.";
                        Ok(
                            json!({"result": {"type": "object", "subtype": "error", "className": "Error", "description": text},
                                  "exceptionDetails": {"exceptionId": 1, "text": text, "lineNumber": 0, "columnNumber": 0}}),
                        )
                    }
                    None => Ok(json!({"result": {"type": "undefined"}})),
                }
            }
            "Runtime.getIsolateId" => Ok(json!({"id": "pocket-game"})),
            "Runtime.getHeapUsage" => Ok(json!({"usedSize": 0, "totalSize": 0})),
            "Runtime.globalLexicalScopeNames" => Ok(json!({"names": []})),
            "Runtime.compileScript" => Ok(json!({})),
            "Debugger.setInstrumentationBreakpoint" => {
                Ok(json!({"breakpointId": "instrumentation"}))
            }
            "Debugger.restartFrame" => Err("Restarting a frame is not supported.".into()),
            "Debugger.setScriptSource" => {
                Err("Live edit is not supported; save the file to hot-update the scripts.".into())
            }
            "Schema.getDomains" => Ok(json!({"domains": [
                {"name": "Debugger", "version": "1.3"}, {"name": "Runtime", "version": "1.3"}]})),
            // Accepted without effect, as V8 accepts them for a target without the domain's
            // subject; clients send several at attach.
            _ => Ok(json!({})),
        }
    }
}

fn paused_only() -> String {
    "Can only perform operation while paused.".into()
}

fn condition(p: &Json) -> Option<String> {
    p["condition"]
        .as_str()
        .filter(|c| !c.trim().is_empty())
        .map(str::to_owned)
}

fn truncate(s: &str) -> &str {
    &s[..s.char_indices().nth(400).map_or(s.len(), |(i, _)| i)]
}
