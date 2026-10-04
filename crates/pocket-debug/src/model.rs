//! What the debugger's parts exchange: pauses, events, and the requests the game thread serves
//! while it is stopped.

use std::sync::Arc;
use std::sync::mpsc::Sender;

use serde_json::{Value as Json, json};

use crate::inspect::Frame;
use crate::scripts::Script;

/// A position in a module: 0-based in its JavaScript, and 0-based in its TypeScript when the
/// JavaScript came from TypeScript.
#[derive(Clone, Debug)]
pub struct Location {
    pub script: Arc<Script>,
    pub js: (u32, u32),
    pub ts: Option<(u32, u32)>,
}

impl Location {
    pub fn new(script: Arc<Script>, js: (u32, u32)) -> Location {
        let ts = script.to_ts(js.0, js.1);
        Location { script, js, ts }
    }

    /// As agents see it: the module path and the 1-based TypeScript line and column (the
    /// JavaScript's when no TypeScript maps there).
    pub fn json(&self) -> Json {
        let (line, column, generated) = match self.ts {
            Some((l, c)) => (l + 1, c + 1, false),
            None => (self.js.0 + 1, self.js.1 + 1, true),
        };
        let mut v = json!({"file": self.script.module, "line": line, "column": column});
        if generated {
            v["generated"] = json!(true);
        }
        v
    }

    /// A CDP `Location`.
    pub fn cdp(&self) -> Json {
        json!({"scriptId": self.script.id, "lineNumber": self.js.0, "columnNumber": self.js.1})
    }
}

/// When the game pauses on exceptions (CDP `Debugger.setPauseOnExceptions`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExceptionMode {
    #[default]
    None,
    Uncaught,
    All,
}

impl ExceptionMode {
    pub fn parse(s: &str) -> Option<ExceptionMode> {
        match s {
            "none" => Some(ExceptionMode::None),
            "uncaught" => Some(ExceptionMode::Uncaught),
            "all" | "caught" => Some(ExceptionMode::All),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ExceptionMode::None => "none",
            ExceptionMode::Uncaught => "uncaught",
            ExceptionMode::All => "all",
        }
    }
}

/// A step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepKind {
    Over,
    Into,
    Out,
}

impl StepKind {
    pub fn parse(s: &str) -> Option<StepKind> {
        match s {
            "over" => Some(StepKind::Over),
            "into" => Some(StepKind::Into),
            "out" => Some(StepKind::Out),
            _ => None,
        }
    }
}

/// Why the game thread stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    Breakpoint,
    Step,
    DebuggerStatement,
    /// `Debugger.pause` or `debug.pause`.
    Request,
    Exception {
        caught: bool,
    },
    /// A data breakpoint: a watched field's staged value changed.
    Data,
}

impl Reason {
    /// The agents' name.
    pub fn name(&self) -> &'static str {
        match self {
            Reason::Breakpoint => "breakpoint",
            Reason::Step => "step",
            Reason::DebuggerStatement => "debugger_statement",
            Reason::Request => "pause",
            Reason::Exception { .. } => "exception",
            Reason::Data => "data_breakpoint",
        }
    }

    /// CDP `Debugger.paused`'s `reason`.
    pub fn cdp(&self) -> &'static str {
        match self {
            Reason::Exception { .. } => "exception",
            Reason::Step => "step",
            Reason::Breakpoint | Reason::DebuggerStatement => "other",
            Reason::Request => "other",
            Reason::Data => "other",
        }
    }
}

/// A data breakpoint's hit.
#[derive(Clone, Debug)]
pub struct WatchHit {
    pub watch: String,
    pub entity: u64,
    pub component: String,
    pub field: Option<String>,
    /// The watched values' names (`position.x`, `taken`).
    pub names: Vec<String>,
    pub before: Json,
    pub after: Json,
    /// The statement that wrote (the one traced before the change was seen).
    pub written_at: Option<Location>,
    /// Seen only when the system returned: the game stopped after its last statement.
    pub after_return: bool,
}

impl WatchHit {
    pub fn json(&self) -> Json {
        json!({
            "watch": self.watch,
            "entity": self.entity,
            "component": self.component,
            "field": self.field,
            "names": self.names,
            "before": self.before,
            "after": self.after,
            "written_at": self.written_at.as_ref().map(Location::json),
            "after_return": self.after_return,
        })
    }
}

/// One stop of the game thread.
#[derive(Clone, Debug)]
pub struct Pause {
    /// Numbers the pauses of this hub; object ids of a pause carry it.
    pub serial: u64,
    pub reason: Reason,
    pub hit_breakpoints: Vec<String>,
    pub tick: u64,
    /// The system running, `None` while a program loads.
    pub system: Option<String>,
    pub frames: Vec<Frame>,
    /// The frames as CDP `callFrames`.
    pub cdp_frames: Json,
    /// The exception, as a CDP `RemoteObject` and as text.
    pub exception: Option<(Json, String)>,
    pub data: Option<WatchHit>,
}

impl Pause {
    /// Where it stopped.
    pub fn location(&self) -> Option<Location> {
        self.frames.first().map(|f| Location {
            script: f.script.clone(),
            js: f.js,
            ts: f.ts,
        })
    }

    /// As agents see it (`debug.state` while paused, the `debug` event).
    pub fn json(&self) -> Json {
        let frames: Vec<Json> = self
            .frames
            .iter()
            .map(|f| {
                let loc = Location {
                    script: f.script.clone(),
                    js: f.js,
                    ts: f.ts,
                };
                json!({
                    "frame": f.level,
                    "function": if f.function.is_empty() { "(anonymous)" } else { &f.function },
                    "location": loc.json(),
                    "locals": f.locals.iter().map(|v| v.json()).collect::<Vec<_>>(),
                    "closure": f.closure.iter().map(|v| v.json()).collect::<Vec<_>>(),
                    "returned": f.returned,
                })
            })
            .collect();
        let mut v = json!({
            "state": "paused",
            "reason": self.reason.name(),
            "tick": self.tick,
            "system": self.system,
            "location": self.location().map(|l| l.json()),
            "frames": frames,
            "hit_breakpoints": self.hit_breakpoints,
        });
        if let Reason::Exception { caught } = self.reason {
            v["exception"] = json!({
                "text": self.exception.as_ref().map(|e| e.1.clone()),
                "caught": caught,
            });
        }
        if let Some(d) = &self.data {
            v["data"] = d.json();
        }
        v
    }

    /// CDP `Debugger.paused`'s parameters.
    pub fn cdp(&self) -> Json {
        let mut p = json!({
            "callFrames": self.cdp_frames,
            "reason": self.reason.cdp(),
            "hitBreakpoints": self.hit_breakpoints,
        });
        match (&self.reason, &self.exception, &self.data) {
            (Reason::Exception { caught }, Some((remote, text)), _) => {
                let mut data = remote.clone();
                data["uncaught"] = json!(!caught);
                if data.get("description").is_none() {
                    data["description"] = json!(text);
                }
                p["data"] = data;
            }
            (Reason::Data, _, Some(hit)) => {
                p["data"] = json!({"pocket": "data_breakpoint", "watch": hit.json()});
            }
            (Reason::DebuggerStatement, _, _) => {
                p["data"] = json!({"pocket": "debugger_statement"});
            }
            _ => {}
        }
        p
    }
}

/// A console line a script wrote.
#[derive(Clone, Debug)]
pub struct ConsoleLine {
    /// `log`, `info`, `warn`, `error` or `debug`.
    pub level: String,
    pub text: String,
    pub tick: u64,
    pub system: Option<String>,
    pub function: String,
    pub location: Option<Location>,
}

impl ConsoleLine {
    /// The protocol's `log` event data.
    pub fn json(&self) -> Json {
        let mut v = json!({"level": self.level, "source": "script", "message": self.text,
                           "tick": self.tick, "system": self.system});
        if let Some(l) = &self.location {
            let at = l.json();
            v["file"] = at["file"].clone();
            v["line"] = at["line"].clone();
            v["column"] = at["column"].clone();
        }
        v
    }
}

/// A system call that failed with a script error.
#[derive(Clone, Debug)]
pub struct ScriptException {
    pub tick: u64,
    pub system: String,
    pub code: String,
    pub message: String,
    /// Where it was thrown (from the trace, when the program is instrumented).
    pub location: Option<Location>,
    /// The error's own TypeScript location, as the host reports it (1-based).
    pub reported: Option<(String, u32, u32)>,
}

impl ScriptException {
    pub fn json(&self) -> Json {
        let mut v = json!({"level": "error", "source": "script", "message": self.message,
                           "code": self.code, "tick": self.tick, "system": self.system});
        if let Some((f, l, c)) = &self.reported {
            v["file"] = json!(f);
            v["line"] = json!(l);
            v["column"] = json!(c);
        } else if let Some(l) = &self.location {
            let at = l.json();
            v["file"] = at["file"].clone();
            v["line"] = at["line"].clone();
            v["column"] = at["column"].clone();
        }
        v
    }
}

/// What a debugger tells its frontends.
#[derive(Clone, Debug)]
pub enum DebugEvent {
    /// Scripts not shown before (a program was loaded).
    Scripts(Vec<Arc<Script>>),
    /// A breakpoint now applies to a script it did not.
    BreakpointResolved {
        id: String,
        location: Location,
    },
    Paused(Arc<Pause>),
    Resumed,
    Console(Arc<ConsoleLine>),
    Exception(Arc<ScriptException>),
}

impl DebugEvent {
    /// The host protocol's pushed event (host-protocol.md 3): `{"event": topic, "data": ...}`;
    /// `None` for what agents need not hear (scripts, breakpoint resolutions).
    pub fn json(&self) -> Option<Json> {
        match self {
            DebugEvent::Paused(p) => Some(json!({"event": "debug", "data": p.json()})),
            DebugEvent::Resumed => Some(json!({"event": "debug", "data": {"state": "running"}})),
            DebugEvent::Console(c) => Some(json!({"event": "log", "data": c.json()})),
            DebugEvent::Exception(e) => Some(json!({"event": "log", "data": e.json()})),
            DebugEvent::Scripts(_) | DebugEvent::BreakpointResolved { .. } => None,
        }
    }
}

/// The answer to a request the game thread serves while stopped.
pub type Reply = Sender<Result<Json, String>>;

/// What the game thread does while stopped.
pub enum Command {
    Resume,
    Step(StepKind),
    /// CDP `evaluateOnCallFrame` (`json: false`) or the agents' `debug.eval` (`json: true`).
    Evaluate {
        frame: usize,
        expression: String,
        by_value: bool,
        json: bool,
        reply: Reply,
    },
    Properties {
        object_id: String,
        reply: Reply,
    },
    CallFunctionOn {
        object_id: Option<String>,
        declaration: String,
        arguments: Vec<Json>,
        by_value: bool,
        reply: Reply,
    },
    SetVariable {
        frame: usize,
        name: String,
        value: Json,
        reply: Reply,
    },
    /// The agents' `debug.set`: `expression`, evaluated in the frame, becomes the value of the
    /// frame's variable `name` (an argument, a local or a closure variable).
    Assign {
        frame: usize,
        name: String,
        expression: String,
        reply: Reply,
    },
}
