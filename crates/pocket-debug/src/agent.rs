//! The agents' debugger (host-protocol.md 6; docs/spec/debugger.md 7): JSON in, JSON out, the
//! same core the CDP endpoint drives. `pocket-server` and `pocket-mcp` call [`DebugHub::call`]
//! with a method name and its parameters and send back the result or the problem; the methods and
//! their parameter schemas are [`methods`].
//!
//! Positions are TypeScript, 1-based (`{file: "scripts/rules.ts", line: 30}`), as the script host's
//! errors give them. Unknown parameters are refused with suggestions (`request.unknown_field`).

use std::sync::atomic::Ordering;
use std::time::Duration;

use pocket_contract::{CheckOptions, Problem, detail};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value as Json, json};

use crate::hub::{AGENT, DebugHub, Target, lock};
use crate::model::{Command, ExceptionMode, StepKind};

/// How long a step, a pause request or a wait blocks by default.
const DEFAULT_WAIT_MS: u64 = 5000;
/// The longest an agent may make a call wait.
const MAX_WAIT_MS: u64 = 120_000;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoParams {}

/// `debug.breakpoints.set`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BreakpointSet {
    /// The module path, as `scripts.list` names it: `scripts/rules.ts`.
    file: String,
    /// The TypeScript line, 1-based; a line without code moves to the next line with code.
    line: u32,
    /// Stop only when this expression, evaluated in the stopped frame, is truthy.
    #[serde(default)]
    condition: Option<String>,
    /// A logpoint: log this template literal's text (`speed ${speed}`) instead of stopping.
    #[serde(default)]
    log: Option<String>,
}

/// `debug.breakpoints.clear`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BreakpointClear {
    /// The breakpoint; every breakpoint agents set when absent.
    #[serde(default)]
    id: Option<String>,
}

/// `debug.pause`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PauseParams {
    /// Wait up to this long for the game to stop (0: return at once).
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum StepParam {
    Over,
    Into,
    Out,
}

/// `debug.step`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct StepParams {
    kind: StepParam,
    /// Wait up to this long for the step to stop (default 5000).
    #[serde(default)]
    timeout_ms: Option<u64>,
}

/// `debug.eval`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EvalParams {
    /// A JavaScript expression; it sees the frame's arguments, locals and closure variables.
    expr: String,
    /// The frame, as `debug.state` numbers them (0: where the game stopped).
    #[serde(default)]
    frame: Option<usize>,
}

/// `debug.watch`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WatchParams {
    /// The entity's id.
    entity: u64,
    /// A component scripts can write, such as `Log`.
    component: String,
    /// One field (every field when absent).
    #[serde(default)]
    field: Option<String>,
}

/// `debug.unwatch`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UnwatchParams {
    /// The watch; every watch agents set when absent.
    #[serde(default)]
    id: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum ModeParam {
    None,
    Uncaught,
    All,
}

/// `debug.exceptions`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExceptionsParams {
    mode: ModeParam,
}

/// `debug.wait`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WaitParams {
    /// Wait up to this long for the game to stop (default 5000).
    #[serde(default)]
    timeout_ms: Option<u64>,
}

/// One method of the agents' debugger.
pub struct Method {
    pub name: &'static str,
    pub doc: &'static str,
    /// Its parameters' JSON Schema.
    pub params: Json,
}

fn schema<T: JsonSchema>() -> Json {
    serde_json::to_value(schemars::schema_for!(T)).unwrap_or(Json::Null)
}

/// The methods [`DebugHub::call`] serves, with their parameters' schemas: what `pocket-mcp` and
/// `pocket-server` put in the command catalog.
pub fn methods() -> Vec<Method> {
    vec![
        Method {
            name: "debug.attach",
            doc: "Starts debugging: the scripts are instrumented from the next tick (breakpoints, pauses and watches attach implicitly).",
            params: schema::<NoParams>(),
        },
        Method {
            name: "debug.detach",
            doc: "Stops debugging: the agents' breakpoints and watches go, a pause resumes, scripts run uninstrumented from the next tick.",
            params: schema::<NoParams>(),
        },
        Method {
            name: "debug.breakpoints.set",
            doc: "Sets a breakpoint on a TypeScript line (1-based), with an optional condition, or a logpoint.",
            params: schema::<BreakpointSet>(),
        },
        Method {
            name: "debug.breakpoints.clear",
            doc: "Removes a breakpoint, or every breakpoint agents set.",
            params: schema::<BreakpointClear>(),
        },
        Method {
            name: "debug.breakpoints.list",
            doc: "Lists every breakpoint (the agents' and the CDP clients') with where it resolves.",
            params: schema::<NoParams>(),
        },
        Method {
            name: "debug.pause",
            doc: "Stops the game at the next script statement; waits up to timeout_ms for it.",
            params: schema::<PauseParams>(),
        },
        Method {
            name: "debug.continue",
            doc: "Resumes a paused game (and releases a game waiting for a debugger).",
            params: schema::<NoParams>(),
        },
        Method {
            name: "debug.step",
            doc: "Steps over, into or out of the current statement and returns where it stopped.",
            params: schema::<StepParams>(),
        },
        Method {
            name: "debug.state",
            doc: "Running or paused; when paused, why, the tick and system, and the frames with TypeScript locations and locals.",
            params: schema::<NoParams>(),
        },
        Method {
            name: "debug.eval",
            doc: "Evaluates an expression in a frame of the paused game.",
            params: schema::<EvalParams>(),
        },
        Method {
            name: "debug.watch",
            doc: "A data breakpoint: pause when a script system's staged write changes the field.",
            params: schema::<WatchParams>(),
        },
        Method {
            name: "debug.unwatch",
            doc: "Removes a data breakpoint, or every one agents set.",
            params: schema::<UnwatchParams>(),
        },
        Method {
            name: "debug.exceptions",
            doc: "Pauses on exceptions: none, uncaught (no try catches them) or all.",
            params: schema::<ExceptionsParams>(),
        },
        Method {
            name: "debug.wait",
            doc: "Waits up to timeout_ms for the game to stop and returns the state.",
            params: schema::<WaitParams>(),
        },
    ]
}

fn decode<T: serde::de::DeserializeOwned + JsonSchema>(
    params: &Json,
    method: &str,
) -> Result<T, Problem> {
    let empty = json!({});
    let p = if params.is_null() { &empty } else { params };
    let owner = format!("the {method} request");
    pocket_contract::decode::<T>(p, &CheckOptions::new(&owner)).map(|d| d.value)
}

fn problem(code: &str, message: impl Into<String>, fields: Json) -> Problem {
    let mut d = detail([]);
    if let Json::Object(m) = fields {
        d.extend(m);
    }
    Problem::new(code, message, d)
}

fn not_paused() -> Problem {
    problem(
        "debug.not_paused",
        "The game is running; pause it (debug.pause) or wait for a breakpoint (debug.wait) first.",
        json!({}),
    )
}

fn wait_ms(t: Option<u64>, default: u64) -> Duration {
    Duration::from_millis(t.unwrap_or(default).min(MAX_WAIT_MS))
}

impl DebugHub {
    /// Serves one method of the agents' debugger (`debug.*`, [`methods`]). Blocks at most for the
    /// waits the method asks (`timeout_ms`), never for the game thread otherwise.
    pub fn call(&self, method: &str, params: &Json) -> Result<Json, Problem> {
        match method {
            "debug.attach" => {
                decode::<NoParams>(params, method)?;
                self.attach_agent();
                Ok(self.state_json())
            }
            "debug.detach" => {
                decode::<NoParams>(params, method)?;
                {
                    let mut st = self.state();
                    st.breakpoints.retain(|b| b.owner != AGENT);
                    st.watches.retain(|w| w.owner != AGENT);
                    if let Some(c) = st.clients.get_mut(&AGENT) {
                        c.enabled = false;
                        c.exceptions = ExceptionMode::None;
                    }
                    self.recompute(&mut st);
                }
                if !self.inner.attached.load(Ordering::Acquire) {
                    self.inner.pause_requested.store(false, Ordering::Release);
                    let _ = self.send(Command::Resume);
                }
                Ok(self.state_json())
            }
            "debug.breakpoints.set" => {
                let p: BreakpointSet = decode(params, method)?;
                self.attach_agent();
                let module = self.module_named(&p.file)?;
                let line = p.line.max(1) - 1;
                let (id, resolved) = self.add_breakpoint(
                    AGENT,
                    Target::Module(module.clone()),
                    line,
                    p.condition.filter(|c| !c.trim().is_empty()),
                    p.log,
                );
                let locations: Vec<Json> = resolved.iter().map(|r| r.shown.json()).collect();
                if locations.is_empty() {
                    self.remove_breakpoint(Some(AGENT), &id);
                    return Err(problem(
                        "debug.no_code",
                        format!("{module} has no code at or after line {}.", p.line),
                        json!({"file": module, "line": p.line}),
                    ));
                }
                let at = &locations[0];
                Ok(
                    json!({"id": id, "file": module, "line": at["line"], "verified": true,
                          "locations": locations}),
                )
            }
            "debug.breakpoints.clear" => {
                let p: BreakpointClear = decode(params, method)?;
                let cleared = match p.id {
                    Some(id) => {
                        if !self.remove_breakpoint(None, &id) {
                            let ids: Vec<String> = self
                                .state()
                                .breakpoints
                                .iter()
                                .map(|b| b.id.clone())
                                .collect();
                            return Err(problem(
                                "debug.unknown_breakpoint",
                                format!("There is no breakpoint '{id}'."),
                                json!({"id": id, "allowed": ids}),
                            ));
                        }
                        1
                    }
                    None => {
                        let mut st = self.state();
                        let before = st.breakpoints.len();
                        st.breakpoints.retain(|b| b.owner != AGENT);
                        let n = before - st.breakpoints.len();
                        self.recompute(&mut st);
                        n
                    }
                };
                Ok(json!({"cleared": cleared}))
            }
            "debug.breakpoints.list" => {
                decode::<NoParams>(params, method)?;
                Ok(json!({"breakpoints": self.breakpoints_json()}))
            }
            "debug.pause" => {
                let p: PauseParams = decode(params, method)?;
                self.attach_agent();
                let before = self.state().pauses;
                if !self.is_paused() {
                    self.request_pause();
                    let wait = wait_ms(p.timeout_ms, 0);
                    if !wait.is_zero() {
                        self.wait_pause(before, wait);
                    }
                }
                Ok(self.state_json())
            }
            "debug.continue" => {
                decode::<NoParams>(params, method)?;
                self.release();
                if let Some(p) = self.pause_info() {
                    self.resume(None)
                        .map_err(|m| problem("debug.not_paused", m, json!({})))?;
                    // Returns once the game thread has left the pause, so a following call sees
                    // it running (or stopped again).
                    self.wait_left(p.serial, Duration::from_secs(2));
                }
                Ok(json!({"state": "running"}))
            }
            "debug.step" => {
                let p: StepParams = decode(params, method)?;
                let kind = match p.kind {
                    StepParam::Over => StepKind::Over,
                    StepParam::Into => StepKind::Into,
                    StepParam::Out => StepKind::Out,
                };
                let before = self.pause_info().ok_or_else(not_paused)?.serial;
                self.resume(Some(kind)).map_err(|_| not_paused())?;
                self.wait_pause(before, wait_ms(p.timeout_ms, DEFAULT_WAIT_MS));
                Ok(self.state_json())
            }
            "debug.state" => {
                decode::<NoParams>(params, method)?;
                Ok(self.state_json())
            }
            "debug.eval" => {
                let p: EvalParams = decode(params, method)?;
                let pause = self.pause_info().ok_or_else(not_paused)?;
                let index = p.frame.unwrap_or(0);
                let frame = pause.frames.get(index).ok_or_else(|| {
                    problem(
                        "debug.no_frame",
                        format!(
                            "There is no frame {index}; the game stopped with {} frames.",
                            pause.frames.len()
                        ),
                        json!({"frame": index, "frames": pause.frames.len()}),
                    )
                })?;
                let level = frame.level;
                let expression = p.expr;
                let r = self
                    .ask(|reply| Command::Evaluate {
                        frame: level,
                        expression,
                        by_value: true,
                        json: true,
                        reply,
                    })
                    .map_err(|m| problem("debug.eval_failed", m, json!({})))?;
                if let Some(e) = r.get("error") {
                    return Err(problem(
                        "debug.eval_failed",
                        format!("The expression threw: {}", e.as_str().unwrap_or("")),
                        json!({"error": e}),
                    ));
                }
                Ok(r)
            }
            "debug.watch" => {
                let p: WatchParams = decode(params, method)?;
                self.attach_agent();
                let id = self.add_watch(AGENT, p.entity, &p.component, p.field.as_deref());
                Ok(
                    json!({"id": id, "entity": p.entity, "component": p.component, "field": p.field}),
                )
            }
            "debug.unwatch" => {
                let p: UnwatchParams = decode(params, method)?;
                let mut st = self.state();
                let before = st.watches.len();
                match &p.id {
                    Some(id) => st.watches.retain(|w| &w.id != id),
                    None => st.watches.retain(|w| w.owner != AGENT),
                }
                let n = before - st.watches.len();
                self.recompute(&mut st);
                if n == 0
                    && let Some(id) = p.id
                {
                    return Err(problem(
                        "debug.unknown_watch",
                        format!("There is no watch '{id}'."),
                        json!({"id": id}),
                    ));
                }
                Ok(json!({"cleared": n}))
            }
            "debug.exceptions" => {
                let p: ExceptionsParams = decode(params, method)?;
                let mode = match p.mode {
                    ModeParam::None => ExceptionMode::None,
                    ModeParam::Uncaught => ExceptionMode::Uncaught,
                    ModeParam::All => ExceptionMode::All,
                };
                self.attach_agent();
                self.update_client(AGENT, |c| c.exceptions = mode);
                Ok(json!({"mode": mode.name()}))
            }
            "debug.wait" => {
                let p: WaitParams = decode(params, method)?;
                if !self.is_paused() {
                    let before = self.state().pauses;
                    self.wait_pause(before, wait_ms(p.timeout_ms, DEFAULT_WAIT_MS));
                }
                Ok(self.state_json())
            }
            "debug.rewind" => Err(problem(
                "debug.unsupported",
                "debug.rewind needs the runtime's snapshot ring, which this build does not keep yet.",
                json!({"method": method}),
            )),
            other => {
                let names: Vec<&str> = methods().iter().map(|m| m.name).collect();
                Err(problem(
                    "request.unknown_method",
                    format!("There is no request '{other}'."),
                    json!({"method": other,
                           "suggestions": pocket_contract::suggest::suggest_names(other, names.iter().copied()),
                           "allowed": names}),
                ))
            }
        }
    }

    fn attach_agent(&self) {
        self.update_client(AGENT, |c| c.enabled = true);
    }

    /// A module path of the running program, or `debug.unknown_file` with suggestions.
    fn module_named(&self, file: &str) -> Result<String, Problem> {
        let st = self.state();
        let wanted = file.trim_start_matches("./").replace('\\', "/");
        if st.registry.module(&wanted).is_some() {
            return Ok(wanted);
        }
        let modules = st.registry.modules();
        // A path that ends with the module path (an absolute path from an editor) names it.
        if let Some(m) = modules.iter().find(|m| wanted.ends_with(m.as_str())) {
            return Ok(m.clone());
        }
        Err(problem(
            "debug.unknown_file",
            format!("'{file}' is not a module of the running scripts."),
            json!({"file": file,
                   "suggestions": pocket_contract::suggest::suggest_names(&wanted, modules.iter().map(String::as_str)),
                   "allowed": modules}),
        ))
    }

    fn breakpoints_json(&self) -> Vec<Json> {
        let st = self.state();
        st.breakpoints
            .iter()
            .map(|b| {
                let owner = st.clients.get(&b.owner).map_or("gone", |c| c.kind);
                let target = match &b.target {
                    Target::Module(m) => json!({"file": m, "line": b.line + 1}),
                    Target::Url(u) => json!({"url": u, "js_line": b.line}),
                    Target::Regex(r) => json!({"url_regex": r.as_str(), "js_line": b.line}),
                    Target::Script(id) => json!({"script_id": id, "js_line": b.line}),
                };
                let at: Vec<Json> = crate::hub::resolve(b, &st.registry)
                    .iter()
                    .map(|r| r.shown.json())
                    .collect();
                json!({"id": b.id, "owner": owner, "target": target, "condition": b.condition,
                       "log": b.log, "locations": at})
            })
            .collect()
    }

    /// `debug.state`.
    pub fn state_json(&self) -> Json {
        let pause = self.pause_info();
        let mut v = match &pause {
            Some(p) => p.json(),
            None => {
                let st = self.state();
                json!({"state": "running", "tick": st.running.tick, "system": st.running.system})
            }
        };
        let (instrumented, exceptions, watches) = {
            let st = self.state();
            let mode = st
                .clients
                .values()
                .filter(|c| c.enabled)
                .map(|c| c.exceptions)
                .max();
            let watches: Vec<Json> = st
                .watches
                .iter()
                .map(|w| json!({"id": w.id, "entity": w.entity, "component": w.component, "field": w.field}))
                .collect();
            (st.running.instrumented, mode.unwrap_or_default(), watches)
        };
        v["attached"] = json!(self.inner.attached.load(Ordering::Acquire));
        v["instrumented"] = json!(instrumented);
        v["exceptions"] = json!(exceptions.name());
        v["breakpoints"] = json!(self.breakpoints_json());
        v["watches"] = json!(watches);
        v["waiting_for_debugger"] = json!(*lock(&self.inner.gate));
        v
    }
}
