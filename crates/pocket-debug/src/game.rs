//! The game thread's half of the debugger (docs/spec/debugger.md 3 and 5): the
//! [`DebugHook`] the script host calls. It decides at each traced statement whether to stop
//! (a pause request, a `debugger;` statement, a breakpoint and its condition, a step, an
//! exception, a data breakpoint) and, when it stops, serves the frontends' requests from inside
//! the trace call until they resume, so the world stays owned by this one thread while rendering
//! and the server go on.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;

use pocket_script::debug::{
    CallInfo, DebugHook, FieldWatch, FrameInfo, Trace, TraceKind, WatchValue, frame_info,
    set_tracing, stack_depth,
};
use pocket_script::program::SystemOutcome;
use pocket_script::{CompiledSet, ffi};
use rquickjs::qjs::{self, JSAtom, JSContext};
use serde_json::{Value as Json, json};

use crate::hub::{Inner, Watch, emit, lock};
use crate::inspect::{Frame, Inspector, Top, condition_holds, log_message};
use crate::model::{
    Command, ConsoleLine, DebugEvent, ExceptionMode, Location, Pause, Reason, ScriptException,
    StepKind, WatchHit,
};
use crate::scripts::Script;

struct Bp {
    id: String,
    condition: Option<String>,
    log: Option<String>,
}

struct GameWatch {
    watch: Watch,
    probe: FieldWatch,
    /// The staged value at the last probe of this call (`None` before the first).
    last: Option<Option<Vec<WatchValue>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    None,
    Over {
        depth: usize,
        at: (usize, u32),
    },
    Into {
        depth: usize,
        at: (usize, u32),
    },
    Out {
        depth: usize,
    },
    /// Stop at the next statement anywhere (a step left the system's `run`).
    Next,
}

/// What the hook works from on this thread.
struct Local {
    generation: u64,
    scripts: Vec<Arc<Script>>,
    by_module: HashMap<String, usize>,
    atoms: HashMap<JSAtom, Option<usize>>,
    /// Per script: generated lines with a breakpoint.
    hot: Vec<Vec<bool>>,
    bps: HashMap<(usize, u32), Vec<Bp>>,
    watches: Vec<GameWatch>,
    exceptions: ExceptionMode,
    debugger_statements: bool,
    step: Step,
    /// The statement traced before this one: script, generated line and column.
    prev: Option<(usize, u32, u32)>,
    /// The exception the game last stopped on (its bits), not to stop on its rethrows.
    last_exception: Option<Vec<u8>>,
    /// Where the running call last threw in project code.
    last_throw: Option<(usize, u32, u32)>,
}

impl Local {
    fn refresh(&mut self, inner: &Inner) {
        let g = inner.generation.load(Ordering::Acquire);
        if g == self.generation {
            return;
        }
        let s = lock(&inner.state).settings();
        self.generation = g;
        let reload = self.scripts.len() != s.scripts.len()
            || self
                .scripts
                .iter()
                .zip(&s.scripts)
                .any(|(a, b)| !Arc::ptr_eq(a, b));
        if reload {
            self.atoms.clear();
            self.prev = None;
        }
        self.scripts = s.scripts;
        self.by_module = self
            .scripts
            .iter()
            .enumerate()
            .map(|(i, s)| (s.module.clone(), i))
            .collect();
        self.hot = vec![Vec::new(); self.scripts.len()];
        self.bps.clear();
        for b in s.breakpoints {
            let Some(&i) = self.by_module.get(&b.script.module) else {
                continue;
            };
            if !Arc::ptr_eq(&self.scripts[i], &b.script) {
                continue;
            }
            let l = b.line as usize;
            if self.hot[i].len() <= l {
                self.hot[i].resize(l + 1, false);
            }
            self.hot[i][l] = true;
            self.bps.entry((i, b.line)).or_default().push(Bp {
                id: b.id,
                condition: b.condition,
                log: b.log,
            });
        }
        let mut old: Vec<GameWatch> = std::mem::take(&mut self.watches);
        for w in s.watches {
            if let Some(pos) = old.iter().position(|g| g.watch.id == w.id) {
                self.watches.push(old.swap_remove(pos));
            } else if let Some(probe) = FieldWatch::new(w.entity, &w.component, w.field.as_deref())
            {
                self.watches.push(GameWatch {
                    watch: w,
                    probe,
                    last: None,
                });
            }
        }
        self.exceptions = s.exceptions;
        self.debugger_statements = s.debugger_statements;
    }

    fn script_index(&mut self, ctx: *mut JSContext, atom: JSAtom) -> Option<usize> {
        if let Some(hit) = self.atoms.get(&atom) {
            return *hit;
        }
        let name = unsafe { pocket_script::debug::atom_text(ctx, atom) };
        let found = self.by_module.get(&name).copied();
        self.atoms.insert(atom, found);
        found
    }

    fn location(&self, at: (usize, u32, u32)) -> Option<Location> {
        let s = self.scripts.get(at.0)?;
        Some(Location::new(s.clone(), (at.1, at.2)))
    }

    /// Probes the data breakpoints: the first whose staged value changed since the last probe.
    fn probe_watches(&mut self, ctx: *mut JSContext) -> Option<(usize, Json, Json)> {
        let mut hit = None;
        for (i, w) in self.watches.iter_mut().enumerate() {
            let Ok(now) = (unsafe { w.probe.read(ctx) }) else {
                continue;
            };
            let changed = match &w.last {
                None => false,
                Some(before) => !same(before, &now),
            };
            if changed && hit.is_none() {
                let before = w.last.clone().flatten();
                hit = Some((
                    i,
                    values_json(before.as_deref()),
                    values_json(now.as_deref()),
                ));
            }
            w.last = Some(now);
        }
        hit
    }
}

fn same(a: &Option<Vec<WatchValue>>, b: &Option<Vec<WatchValue>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| p.same(q)),
        _ => false,
    }
}

fn values_json(v: Option<&[WatchValue]>) -> Json {
    match v {
        None => Json::Null,
        Some(vals) => Json::Array(
            vals.iter()
                .map(|x| match x {
                    WatchValue::Number(f) => crate::inspect::number_json(*f),
                    WatchValue::Text(s) => json!(&**s),
                })
                .collect(),
        ),
    }
}

/// What a statement leads to.
enum Decision {
    Go,
    Stop {
        reason: Reason,
        data: Option<WatchHit>,
    },
    /// Breakpoints to check (their conditions run JavaScript, so outside the hook's borrows),
    /// then stop for `otherwise` if none holds.
    Check {
        bps: Vec<(String, Option<String>, Option<String>)>,
        otherwise: Option<Reason>,
    },
}

/// The hook: lives on the game thread.
pub(crate) struct GameHook {
    inner: Arc<Inner>,
    rx: Receiver<Command>,
    local: RefCell<Local>,
    /// The call running: tick, system, serial.
    call: RefCell<Option<(u64, String, u64)>>,
}

impl GameHook {
    pub(crate) fn new(inner: Arc<Inner>, rx: Receiver<Command>) -> GameHook {
        GameHook {
            inner,
            rx,
            local: RefCell::new(Local {
                generation: 0,
                scripts: Vec::new(),
                by_module: HashMap::new(),
                atoms: HashMap::new(),
                hot: Vec::new(),
                bps: HashMap::new(),
                watches: Vec::new(),
                exceptions: ExceptionMode::None,
                debugger_statements: false,
                step: Step::None,
                prev: None,
                last_exception: None,
                last_throw: None,
            }),
            call: RefCell::new(None),
        }
    }

    fn decide(&self, t: &Trace) -> (Decision, Option<usize>) {
        let Ok(mut local) = self.local.try_borrow_mut() else {
            return (Decision::Go, None);
        };
        local.refresh(&self.inner);
        let idx = local.script_index(t.ctx, t.file);
        let line = t.line.saturating_sub(1);
        let col = t.column.saturating_sub(1);
        if let TraceKind::Exception { caught } = t.kind {
            let Some(i) = idx else {
                return (Decision::Go, None);
            };
            local.last_throw = Some((i, line, col));
            let mode = local.exceptions;
            if mode == ExceptionMode::None || (mode == ExceptionMode::Uncaught && caught) {
                return (Decision::Go, idx);
            }
            let bits = exception_bits(t.ctx);
            if bits.is_some() && bits == local.last_exception {
                return (Decision::Go, idx);
            }
            local.last_exception = bits;
            return (
                Decision::Stop {
                    reason: Reason::Exception { caught },
                    data: None,
                },
                idx,
            );
        }
        let Some(i) = idx else {
            return (Decision::Go, None);
        };
        let prev = local.prev.replace((i, line, col));
        let mut stop: Option<Reason> = None;
        if self.inner.pause_requested.swap(false, Ordering::AcqRel) {
            stop = Some(Reason::Request);
        } else if t.kind == TraceKind::DebuggerStatement && local.debugger_statements {
            stop = Some(Reason::DebuggerStatement);
        }
        let mut data = None;
        if !local.watches.is_empty()
            && let Some((w, before, after)) = local.probe_watches(t.ctx)
        {
            let gw = &local.watches[w];
            data = Some(WatchHit {
                watch: gw.watch.id.clone(),
                entity: gw.watch.entity,
                component: gw.watch.component.clone(),
                field: gw.watch.field.clone(),
                names: gw.probe.names(),
                before,
                after,
                written_at: prev.and_then(|p| local.location(p)),
                after_return: false,
            });
            stop = stop.or(Some(Reason::Data));
        }
        if stop.is_none() && local.step != Step::None {
            let depth = unsafe { stack_depth(t.ctx) };
            let here = (i, line);
            let reached = match local.step {
                Step::None => false,
                Step::Over { depth: d, at } => depth < d || (depth == d && here != at),
                Step::Into { depth: d, at } => depth != d || here != at,
                Step::Out { depth: d } => depth < d,
                Step::Next => true,
            };
            // Generated code no TypeScript wrote (the harden epilogue) is stepped through.
            if reached && local.scripts[i].to_ts(line, col).is_some() {
                stop = Some(Reason::Step);
            }
        }
        let hot = local.hot[i].get(line as usize).copied().unwrap_or(false)
            && prev.map(|p| (p.0, p.1)) != Some((i, line));
        if hot
            && !matches!(
                stop,
                Some(Reason::Request | Reason::DebuggerStatement | Reason::Data)
            )
        {
            let bps = local
                .bps
                .get(&(i, line))
                .map(|v| {
                    v.iter()
                        .map(|b| (b.id.clone(), b.condition.clone(), b.log.clone()))
                        .collect()
                })
                .unwrap_or_default();
            return (
                Decision::Check {
                    bps,
                    otherwise: stop,
                },
                idx,
            );
        }
        match stop {
            Some(reason) => (Decision::Stop { reason, data }, idx),
            None => (Decision::Go, idx),
        }
    }

    /// Stops the game thread: builds the pause, tells the frontends, serves them until they resume.
    /// `top`: the stopped statement, or `None` for a call that has returned (a data breakpoint seen
    /// at its end), whose pause shows where the write was.
    fn stop(
        &self,
        ctx: *mut JSContext,
        top: Option<Top>,
        reason: Reason,
        hits: Vec<String>,
        data: Option<WatchHit>,
    ) {
        unsafe { set_tracing(ctx, false) };
        while self.rx.try_recv().is_ok() {} // requests from before this pause
        let scripts: Vec<Arc<Script>> = self.local.borrow().scripts.clone();
        let serial = {
            let mut st = lock(&self.inner.state);
            st.pauses += 1;
            st.pauses
        };
        let mut insp = unsafe { Inspector::new(ctx, serial) };
        let lookup = |module: &str| scripts.iter().find(|s| s.module == module).cloned();
        let (frames, cdp_frames) = match &top {
            Some(top) => insp.capture(top, &lookup),
            None => returned_frame(&data),
        };
        let exception = if matches!(reason, Reason::Exception { .. }) {
            let raw = unsafe { ffi::JS_GetDebugTraceException(ctx) };
            let rctx =
                unsafe { rquickjs::Ctx::from_raw(std::ptr::NonNull::new(ctx).expect("ctx")) };
            let v = unsafe { rquickjs::Value::from_raw(rctx, raw) };
            Some((insp.remote(&v), insp.text(&v)))
        } else {
            None
        };
        let (tick, system) = match &*self.call.borrow() {
            Some((t, s, _)) => (*t, Some(s.clone())),
            None => (lock(&self.inner.state).running.tick, None),
        };
        let pause = Arc::new(Pause {
            serial,
            reason,
            hit_breakpoints: hits,
            tick,
            system,
            frames,
            cdp_frames,
            exception,
            data,
        });
        let at = top.as_ref().and_then(|t| {
            let i = scripts.iter().position(|s| Arc::ptr_eq(s, &t.script))?;
            Some((i, t.line.saturating_sub(1)))
        });
        let depth = unsafe { stack_depth(ctx) };
        {
            let mut st = lock(&self.inner.state);
            st.pause = Some(pause.clone());
        }
        self.inner.paused.store(true, Ordering::Release);
        self.inner.changed.notify_all();
        emit(&self.inner, DebugEvent::Paused(pause));
        let mut step = Step::None;
        loop {
            match self.rx.recv() {
                Ok(Command::Resume) | Err(_) => break,
                Ok(Command::Step(kind)) => {
                    step = match (kind, at) {
                        (StepKind::Over, Some(at)) => Step::Over { depth, at },
                        (StepKind::Into, Some(at)) => Step::Into { depth, at },
                        (StepKind::Out, Some(_)) => Step::Out { depth },
                        (_, None) => Step::Next,
                    };
                    break;
                }
                Ok(Command::Evaluate {
                    frame,
                    expression,
                    by_value,
                    json,
                    reply,
                }) => {
                    let r = if top.is_none() {
                        Err("The system has returned; nothing can be evaluated in it.".into())
                    } else if json {
                        Ok(insp.evaluate_json(frame, &expression))
                    } else {
                        Ok(insp.evaluate(frame, &expression, by_value))
                    };
                    let _ = reply.send(r);
                }
                Ok(Command::Properties { object_id, reply }) => {
                    let _ = reply.send(insp.properties(&object_id, true));
                }
                Ok(Command::CallFunctionOn {
                    object_id,
                    declaration,
                    arguments,
                    by_value,
                    reply,
                }) => {
                    let r = insp.call_function_on(
                        object_id.as_deref(),
                        &declaration,
                        &arguments,
                        by_value,
                    );
                    let _ = reply.send(r);
                }
                Ok(Command::SetVariable {
                    frame,
                    name,
                    value,
                    reply,
                }) => {
                    let _ = reply.send(insp.set_variable(frame, &name, &value));
                }
            }
        }
        drop(insp); // releases every value handed out during the pause
        if let Ok(mut local) = self.local.try_borrow_mut() {
            local.step = step;
        }
        {
            let mut st = lock(&self.inner.state);
            st.pause = None;
        }
        self.inner.paused.store(false, Ordering::Release);
        self.inner.changed.notify_all();
        emit(&self.inner, DebugEvent::Resumed);
        let instrumented = lock(&self.inner.state).running.instrumented;
        if top.is_some() || instrumented {
            unsafe { set_tracing(ctx, true) };
        }
    }

    fn script_of(&self, module: &str) -> Option<Arc<Script>> {
        lock(&self.inner.state).registry.module(module).cloned()
    }
}

/// The frame of a data breakpoint seen when the system had returned: where the write was, no
/// scopes.
fn returned_frame(data: &Option<WatchHit>) -> (Vec<Frame>, Json) {
    let Some(at) = data.as_ref().and_then(|d| d.written_at.clone()) else {
        return (Vec::new(), json!([]));
    };
    let frame = Frame {
        level: 0,
        function: "(returned)".into(),
        script: at.script.clone(),
        js: at.js,
        ts: at.ts,
        locals: Vec::new(),
        closure: Vec::new(),
        returned: true,
    };
    let cdp = json!([{
        "callFrameId": "0",
        "functionName": "(system returned)",
        "location": at.cdp(),
        "url": at.script.url,
        "scopeChain": [],
        "this": {"type": "undefined"},
        "canBeRestarted": false,
    }]);
    (vec![frame], cdp)
}

/// The bytes of the exception a trace call reports (its identity: an object's pointer).
fn exception_bits(ctx: *mut JSContext) -> Option<Vec<u8>> {
    let v = unsafe { ffi::JS_GetDebugTraceException(ctx) };
    let bytes = unsafe {
        std::slice::from_raw_parts(
            (&v as *const qjs::JSValue).cast::<u8>(),
            std::mem::size_of::<qjs::JSValue>(),
        )
    }
    .to_vec();
    unsafe { qjs::JS_FreeValue(ctx, v) };
    Some(bytes)
}

impl DebugHook for GameHook {
    fn instrument(&self) -> bool {
        self.inner.attached.load(Ordering::Acquire)
    }

    fn loaded(&self, set: &Arc<CompiledSet>, instrumented: bool) {
        let (fresh, resolved) = {
            let mut st = lock(&self.inner.state);
            let fresh = st.registry.load(set);
            st.running.instrumented = instrumented;
            let mut resolved = Vec::new();
            for bp in &st.breakpoints {
                for r in crate::hub::resolve(bp, &st.registry) {
                    if fresh.iter().any(|s| Arc::ptr_eq(s, &r.script)) {
                        resolved.push((bp.id.clone(), r.shown));
                    }
                }
            }
            crate::hub::recompute(&self.inner, &mut st);
            (fresh, resolved)
        };
        if !fresh.is_empty() {
            emit(&self.inner, DebugEvent::Scripts(fresh));
        }
        for (id, location) in resolved {
            emit(&self.inner, DebugEvent::BreakpointResolved { id, location });
        }
    }

    fn call_begin(&self, call: &CallInfo<'_>) {
        *self.call.borrow_mut() = Some((call.tick, call.system.to_owned(), call.serial));
        {
            let mut st = lock(&self.inner.state);
            st.running.tick = call.tick;
            st.running.system = Some(call.system.to_owned());
        }
        if let Ok(mut local) = self.local.try_borrow_mut() {
            for w in &mut local.watches {
                w.last = None;
            }
            local.last_throw = None;
            local.prev = None;
        }
    }

    fn call_returned(&self, ctx: *mut JSContext, _call: &CallInfo<'_>) {
        if !self.inner.attached.load(Ordering::Acquire) {
            return;
        }
        let hit = {
            let Ok(mut local) = self.local.try_borrow_mut() else {
                return;
            };
            local.refresh(&self.inner);
            // A step that leaves the system's `run` stops at the next statement anywhere.
            if local.step != Step::None {
                local.step = Step::Next;
            }
            if local.watches.is_empty() {
                None
            } else {
                local.probe_watches(ctx).map(|(w, before, after)| {
                    let gw = &local.watches[w];
                    WatchHit {
                        watch: gw.watch.id.clone(),
                        entity: gw.watch.entity,
                        component: gw.watch.component.clone(),
                        field: gw.watch.field.clone(),
                        names: gw.probe.names(),
                        before,
                        after,
                        written_at: local.prev.and_then(|p| local.location(p)),
                        after_return: true,
                    }
                })
            }
        };
        if let Some(hit) = hit {
            self.stop(ctx, None, Reason::Data, Vec::new(), Some(hit));
        }
    }

    fn call_end(&self, call: &CallInfo<'_>, outcome: &SystemOutcome) {
        let error = match outcome {
            SystemOutcome::Failed { error, .. } | SystemOutcome::Fault { error } => Some(error),
            SystemOutcome::Ok(_) => None,
        };
        let thrown = self.local.try_borrow_mut().ok().and_then(|mut l| {
            l.last_exception = None;
            let at = l.last_throw.take()?;
            l.location(at)
        });
        *self.call.borrow_mut() = None;
        if let Some(e) = error
            && self.inner.listeners.load(Ordering::Acquire) > 0
        {
            let reported = e
                .detail
                .location
                .as_ref()
                .map(|l| (l.file.clone(), l.line, l.column));
            emit(
                &self.inner,
                DebugEvent::Exception(Arc::new(ScriptException {
                    tick: call.tick,
                    system: call.system.to_owned(),
                    code: e.code.clone(),
                    message: e.message.clone(),
                    location: thrown,
                    reported,
                })),
            );
        }
    }

    fn trace(&self, t: &Trace) {
        if !self.inner.attached.load(Ordering::Relaxed) {
            return;
        }
        let (decision, idx) = self.decide(t);
        let (reason, hits, data) = match decision {
            Decision::Go => return,
            Decision::Stop { reason, data } => (reason, Vec::new(), data),
            Decision::Check { bps, otherwise } => {
                let mut hits = Vec::new();
                for (id, condition, log) in bps {
                    // A condition or a message runs the project's code: untraced, under its own
                    // budget (pocket_script::debug::guarded).
                    unsafe { set_tracing(t.ctx, false) };
                    let holds = condition
                        .as_deref()
                        .is_none_or(|c| unsafe { condition_holds(t.ctx, c) });
                    if holds && let Some(template) = log {
                        let text = unsafe { log_message(t.ctx, &template) };
                        self.logpoint(t, idx, text);
                    } else if holds {
                        hits.push(id);
                    }
                    unsafe { set_tracing(t.ctx, true) };
                }
                if hits.is_empty() {
                    match otherwise {
                        Some(r) => (r, hits, None),
                        None => return,
                    }
                } else {
                    (Reason::Breakpoint, hits, None)
                }
            }
        };
        let Some(script) = idx.and_then(|i| self.local.borrow().scripts.get(i).cloned()) else {
            return;
        };
        let top = Top {
            script,
            line: t.line,
            column: t.column,
        };
        self.stop(t.ctx, Some(top), reason, hits, data);
    }

    fn console(&self, ctx: *mut JSContext, line: &pocket_script::LogLine) {
        if self.inner.listeners.load(Ordering::Acquire) == 0 {
            return;
        }
        let (function, location) = match unsafe { frame_info(ctx, 1) } {
            Some(FrameInfo::Script {
                file,
                function,
                line: l,
                column,
            }) => {
                let loc = self
                    .script_of(&file)
                    .map(|s| Location::new(s, (l.saturating_sub(1), column.saturating_sub(1))));
                (function, loc)
            }
            _ => (String::new(), None),
        };
        emit(
            &self.inner,
            DebugEvent::Console(Arc::new(ConsoleLine {
                level: line.level.clone(),
                text: line.text.clone(),
                tick: line.tick,
                system: line.system.clone(),
                function,
                location,
            })),
        );
    }
}

impl GameHook {
    /// A logpoint's line, as a console line at the statement.
    fn logpoint(&self, t: &Trace, idx: Option<usize>, text: String) {
        let script = idx.and_then(|i| self.local.borrow().scripts.get(i).cloned());
        let (tick, system) = match &*self.call.borrow() {
            Some((tk, s, _)) => (*tk, Some(s.clone())),
            None => (0, None),
        };
        let function = unsafe { pocket_script::debug::atom_text(t.ctx, t.function) };
        emit(
            &self.inner,
            DebugEvent::Console(Arc::new(ConsoleLine {
                level: "log".into(),
                text,
                tick,
                system,
                function,
                location: script.map(|s| {
                    Location::new(s, (t.line.saturating_sub(1), t.column.saturating_sub(1)))
                }),
            })),
        );
    }
}
