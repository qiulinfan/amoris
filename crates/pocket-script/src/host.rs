//! The script host (docs/spec/script-host.md 9; script-sandbox.md 4): one QuickJS-ng runtime on the
//! thread that built it, its limits, the interrupt handler that turns the work counter into
//! budgets, and the state its natives reach during a call. It holds no game state.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use rquickjs::{Ctx, JsLifetime, Persistent, Runtime, Value, qjs};
use serde::{Deserialize, Serialize};

use crate::call::CallState;
use crate::error::{ErrorPhase, ScriptError};
use crate::ffi;
use crate::js;
use crate::source::CompiledSet;
use crate::sourcemap::LineMap;

/// The limits of a host (script-sandbox.md 4.1). The step limits and `max_call_depth` are run
/// configuration a replay records (versions.md 3.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptLimits {
    /// Steps of one system call; at most `i32::MAX`.
    pub steps_per_system: u32,
    /// Steps of all script calls of a tick.
    pub steps_per_tick: u64,
    /// Steps of evaluating a program's modules.
    pub load_steps: u32,
    /// Steps of a derived fact, an instrument or a predicate.
    pub steps_per_call: u32,
    /// Nested calls of any kind (P5).
    pub max_call_depth: u32,
    /// Bytes per QuickJS-ng runtime.
    pub memory_bytes: usize,
    /// QuickJS-ng's stack limit, measured from the start of each call (script-sandbox.md 4.3).
    pub stack_bytes: usize,
    /// Console lines written per tick; beyond them lines are counted, not written.
    pub log_lines_per_tick: u32,
    /// Bytes of the buffers the host makes for one call from script values (query columns and
    /// ids, shuffle permutations, `rng.fill`): past it the call fails with
    /// `script.budget_exceeded`, before the host allocates (script-sandbox.md 4.3).
    pub host_bytes_per_call: u32,
}

impl Default for ScriptLimits {
    fn default() -> Self {
        ScriptLimits {
            steps_per_system: 1_000_000,
            steps_per_tick: 2_000_000,
            load_steps: 1_000_000,
            steps_per_call: 100_000,
            max_call_depth: 200,
            memory_bytes: 64 << 20,
            stack_bytes: DEFAULT_STACK_BYTES,
            log_lines_per_tick: 100,
            host_bytes_per_call: 16 << 20,
        }
    }
}

/// The stack QuickJS-ng may use per call (script-sandbox.md 4.3): wasm32's limit (script-web spike;
/// the web report shows `max_call_depth` fits in it), and natively the use tests/depth.rs measured at
/// `max_call_depth` through the deepest frame shape, times 1.5, rounded up: 356,186 bytes in a
/// release build and 3,467,626 in a debug build on x86_64-pc-windows-msvc with clang-cl.
#[cfg(target_arch = "wasm32")]
pub const DEFAULT_STACK_BYTES: usize = 256 * 1024;
#[cfg(all(not(target_arch = "wasm32"), not(debug_assertions)))]
pub const DEFAULT_STACK_BYTES: usize = 1 << 20;
#[cfg(all(not(target_arch = "wasm32"), debug_assertions))]
pub const DEFAULT_STACK_BYTES: usize = 6 << 20;

/// The stack a thread that runs scripts should have: `stack_bytes` and room for the Rust frames
/// below the first call (script-sandbox.md 4.3; the game thread's `ThreadOptions::stack_bytes`).
pub const THREAD_STACK_BYTES: usize = DEFAULT_STACK_BYTES + (2 << 20);

/// One console line (script-host.md 5.7).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLine {
    pub tick: u64,
    pub system: Option<String>,
    pub level: String,
    pub location: Option<String>,
    pub text: String,
}

/// A promise rejected with no handler during a call: it fails the call (script-sandbox.md 2.2).
pub(crate) struct Rejection {
    pub promise: Persistent<Value<'static>>,
    pub error: ScriptError,
}

/// An error a native threw, kept for the call so `error_from_value` knows it by identity: a script
/// cannot forge a host code by giving its own error the same properties.
pub(crate) struct HostThrow {
    pub value: Persistent<Value<'static>>,
    pub error: ScriptError,
}

/// What the runtime's natives, loader and interrupt handler share.
pub(crate) struct Shared {
    pub limits: ScriptLimits,
    /// A budgeted call runs: the interrupt handler aborts it when the counter runs out.
    pub budget_active: Cell<bool>,
    pub exceeded: Cell<bool>,
    /// The runtime, for the interrupt handler, which rquickjs gives no arguments.
    pub rt: Cell<*mut qjs::JSRuntime>,
    /// P2's out-of-memory count when the call began: any change means memory ran out in it.
    pub oom_base: Cell<u32>,
    /// The errors natives threw in this call (P2-like identity, script-sandbox.md 5).
    pub thrown: RefCell<Vec<HostThrow>>,
    /// Bytes of host buffers made in this call (`host_bytes_per_call`).
    pub host_bytes: Cell<u64>,
    /// The running system call's state.
    pub call: RefCell<Option<CallState>>,
    /// The serial of the current call (RNG handles of another call are expired).
    pub serial: Cell<u64>,
    /// The source maps of the program running or loading.
    pub maps: RefCell<Option<Rc<Maps>>>,
    /// The set being instantiated: what the loader serves.
    pub loading: RefCell<Option<Rc<CompiledSet>>>,
    /// A load failure the loader stored (rquickjs reports only that loading failed).
    pub load_error: RefCell<Option<ScriptError>>,
    pub rejections: RefCell<Vec<Rejection>>,
    /// A native panicked: the call faults with `sim.internal`.
    pub panic: RefCell<Option<String>>,
    pub log: RefCell<Vec<LogLine>>,
    pub log_tick: Cell<u64>,
    pub log_count: Cell<u32>,
    /// What each context's lockdown froze, by context: harden skips these.
    pub intrinsics:
        RefCell<std::collections::BTreeMap<usize, Rc<std::collections::BTreeSet<usize>>>>,
    /// The class ids of generators and built-in iterators.
    pub iterators: RefCell<Vec<u32>>,
    /// The debugger attached to this host (docs/spec/debugger.md 3), if any.
    pub debug: RefCell<Option<Rc<dyn crate::debug::DebugHook>>>,
    /// A program is being instantiated with the trace handler set: the loader compiles the prelude
    /// without it, so the debugger steps through project code only.
    pub instrumenting: Cell<bool>,
    /// A debugger's JavaScript runs: the natives that write refuse (`script.debug_read_only`).
    pub debug_read_only: Cell<bool>,
    /// The first tick a debugger evaluated in, and why, until the game takes it.
    pub taint: RefCell<Option<(u64, String)>>,
}

/// The source maps of a program's modules and of the prelude, by module name.
#[derive(Default)]
pub(crate) struct Maps {
    pub by_module: std::collections::BTreeMap<String, LineMap>,
}

impl Maps {
    /// The TypeScript position of a generated one, or the generated one.
    pub fn map(&self, file: &str, line: u32, column: u32) -> (u32, u32) {
        self.by_module
            .get(file)
            .and_then(|m| m.lookup(line, column))
            .unwrap_or((line, column))
    }
}

/// The host's shared state, stored in the runtime for natives to find.
pub(crate) struct HostData(pub Rc<Shared>);

// SAFETY: HostData holds no JavaScript values.
unsafe impl<'js> JsLifetime<'js> for HostData {
    type Changed<'to> = HostData;
}

/// The shared state of the runtime a context belongs to.
pub(crate) fn shared(ctx: &Ctx<'_>) -> Rc<Shared> {
    ctx.userdata::<HostData>()
        .map(|d| d.0.clone())
        .expect("the host stores its state before any script runs")
}

/// A QuickJS-ng runtime with its limits. Not `Send`: rquickjs 0.14.0 implements `Send` for its
/// runtime only under its `parallel` feature, which pulls tokio in (architecture.md 6). Each
/// thread that runs scripts builds its own host.
pub struct ScriptHost {
    pub(crate) rt: Runtime,
    pub(crate) shared: Rc<Shared>,
}

impl ScriptHost {
    /// A runtime with the memory and stack limits, the interrupt handler, the loader and the
    /// rejection tracker installed.
    pub fn new(limits: ScriptLimits) -> Result<ScriptHost, ScriptError> {
        let rt = Runtime::new().map_err(|e| {
            ScriptError::new(
                "script.out_of_memory",
                format!("No runtime: {e}."),
                ErrorPhase::Load,
            )
        })?;
        rt.set_memory_limit(limits.memory_bytes);
        rt.set_max_stack_size(limits.stack_bytes);
        let shared = Rc::new(Shared {
            limits,
            budget_active: Cell::new(false),
            exceeded: Cell::new(false),
            rt: Cell::new(std::ptr::null_mut()),
            oom_base: Cell::new(0),
            thrown: RefCell::new(Vec::new()),
            host_bytes: Cell::new(0),
            call: RefCell::new(None),
            serial: Cell::new(0),
            maps: RefCell::new(None),
            loading: RefCell::new(None),
            load_error: RefCell::new(None),
            rejections: RefCell::new(Vec::new()),
            panic: RefCell::new(None),
            log: RefCell::new(Vec::new()),
            log_tick: Cell::new(0),
            log_count: Cell::new(0),
            intrinsics: RefCell::new(std::collections::BTreeMap::new()),
            iterators: RefCell::new(Vec::new()),
            debug: RefCell::new(None),
            instrumenting: Cell::new(false),
            debug_read_only: Cell::new(false),
            taint: RefCell::new(None),
        });
        let sh = shared.clone();
        rt.set_interrupt_handler(Some(Box::new(move || {
            if !sh.budget_active.get() {
                return false;
            }
            // P2 zeroes the counter when memory runs out, so the call stops at its next poll
            // even if a builtin swallowed the error; that stop is a fault, not an overrun.
            if !out_of_memory(&sh) {
                sh.exceeded.set(true);
            }
            true
        })));
        let loader = crate::loader::SetLoader;
        rt.set_loader(loader, loader);
        rt.set_host_promise_rejection_tracker(Some(Box::new(track_rejection)));
        Ok(ScriptHost { rt, shared })
    }

    /// The limits.
    pub fn limits(&self) -> &ScriptLimits {
        &self.shared.limits
    }

    /// Takes the console lines written since the last call.
    pub fn take_log(&self) -> Vec<LogLine> {
        std::mem::take(&mut self.shared.log.borrow_mut())
    }

    /// The debugger attached to this host, if any.
    pub fn debugger(&self) -> Option<Rc<dyn crate::debug::DebugHook>> {
        self.shared.debug.borrow().clone()
    }

    /// The heap QuickJS-ng holds, in bytes.
    pub fn heap_bytes(&self) -> i64 {
        self.rt.memory_usage().memory_used_size
    }

    /// Forgets a call a panic cut short: its state (with the world it borrowed), its maps, its
    /// errors and the jobs it queued.
    pub(crate) fn abandon_call(&self) {
        let sh = &self.shared;
        sh.budget_active.set(false);
        *sh.call.borrow_mut() = None;
        *sh.maps.borrow_mut() = None;
        *sh.panic.borrow_mut() = None;
        sh.rejections.borrow_mut().clear();
        sh.thrown.borrow_mut().clear();
        let rt = sh.rt.get();
        if !rt.is_null() {
            unsafe { ffi::JS_DiscardPendingJobs(rt) };
        }
    }
}

impl Drop for ScriptHost {
    /// JavaScript values the shared state holds go before the runtime, which asserts that no object
    /// outlives it.
    fn drop(&mut self) {
        self.shared.rejections.borrow_mut().clear();
        self.shared.thrown.borrow_mut().clear();
        *self.shared.call.borrow_mut() = None;
    }
}

/// Records a promise rejected without a handler, and forgets it when a handler is added.
fn track_rejection<'js>(ctx: Ctx<'js>, promise: Value<'js>, reason: Value<'js>, handled: bool) {
    let sh = shared(&ctx);
    if handled {
        sh.rejections.borrow_mut().retain(|r| {
            r.promise
                .clone()
                .restore(&ctx)
                .map_or(true, |p| !js::same(&p, &promise))
        });
        return;
    }
    let error = crate::caught::error_from_value(&ctx, &sh, &reason, ErrorPhase::Run);
    let promise = Persistent::save(&ctx, promise);
    sh.rejections
        .borrow_mut()
        .push(Rejection { promise, error });
}

/// Starts a budgeted call: the counter at `steps` (P1), the depth limit at `max_call_depth` (P5),
/// the stack top at this frame. Returns the steps given.
pub(crate) fn begin_budget(ctx: &Ctx<'_>, sh: &Shared, steps: u32) -> u32 {
    let c = js::raw(ctx);
    let steps = steps.min(i32::MAX.unsigned_abs());
    let rt = unsafe { qjs::JS_GetRuntime(c) };
    unsafe {
        qjs::JS_UpdateStackTop(rt);
        ffi::JS_SetInterruptCounter(c, i32::try_from(steps).unwrap_or(i32::MAX));
        ffi::JS_SetCallDepthLimit(c, sh.limits.max_call_depth);
    }
    reset_oom(ctx, sh);
    sh.thrown.borrow_mut().clear();
    sh.host_bytes.set(0);
    sh.exceeded.set(false);
    sh.budget_active.set(true);
    steps
}

/// Starts counting out-of-memory errors from now (P2): `out_of_memory` then tells whether memory ran
/// out since.
pub(crate) fn reset_oom(ctx: &Ctx<'_>, sh: &Shared) {
    let rt = unsafe { qjs::JS_GetRuntime(js::raw(ctx)) };
    sh.rt.set(rt);
    sh.oom_base.set(unsafe { ffi::JS_GetOutOfMemoryCount(rt) });
}

/// Whether memory ran out since the call began (P2), whether or not its error reached the host.
pub(crate) fn out_of_memory(sh: &Shared) -> bool {
    let rt = sh.rt.get();
    !rt.is_null() && unsafe { ffi::JS_GetOutOfMemoryCount(rt) } != sh.oom_base.get()
}

/// Drops the jobs a failed call queued (P8): run later, they would act under another call.
pub(crate) fn discard_jobs(ctx: &Ctx<'_>) {
    unsafe { ffi::JS_DiscardPendingJobs(qjs::JS_GetRuntime(js::raw(ctx))) };
}

/// Charges `bytes` of host buffers to the call (`host_bytes_per_call`), before they are made.
pub(crate) fn charge_host_bytes(sh: &Shared, bytes: u64, what: &str) -> Result<(), ScriptError> {
    let total = sh.host_bytes.get().saturating_add(bytes);
    let limit = u64::from(sh.limits.host_bytes_per_call);
    if total > limit {
        return Err(ScriptError::new(
            "script.budget_exceeded",
            format!(
                "{what} would bring the host buffers of this call to {total} bytes, past its \
                 budget of {limit}; ask for fewer rows or fields."
            ),
            ErrorPhase::Run,
        )
        .with("budget", serde_json::json!("host_bytes_per_call"))
        .with("limit", serde_json::json!(limit)));
    }
    sh.host_bytes.set(total);
    Ok(())
}

/// Ends a budgeted call and returns the steps it used: all of them when it overran.
pub(crate) fn end_budget(ctx: &Ctx<'_>, sh: &Shared, steps: u32) -> u64 {
    sh.budget_active.set(false);
    let left = unsafe { ffi::JS_GetInterruptCounter(js::raw(ctx)) };
    if sh.exceeded.get() {
        u64::from(steps)
    } else {
        u64::from(steps.saturating_sub(left.max(0).unsigned_abs()))
    }
}

/// Runs the job queue until it is empty or a job fails; a failure comes back as the exception
/// left on the context.
pub(crate) fn drain_jobs<'js>(ctx: &Ctx<'js>) -> Result<(), Value<'js>> {
    let rt = unsafe { qjs::JS_GetRuntime(js::raw(ctx)) };
    loop {
        let mut job_ctx: *mut qjs::JSContext = std::ptr::null_mut();
        let r = unsafe { qjs::JS_ExecutePendingJob(rt, &mut job_ctx) };
        if r == 0 {
            return Ok(());
        }
        if r < 0 {
            return Err(ctx.catch());
        }
    }
}

/// Whether jobs are still queued.
pub(crate) fn jobs_pending(ctx: &Ctx<'_>) -> bool {
    unsafe { qjs::JS_IsJobPending(qjs::JS_GetRuntime(js::raw(ctx))) }
}
