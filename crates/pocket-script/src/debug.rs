//! The debugger's hooks in the script host (docs/spec/debugger.md 3). The debugger itself
//! (breakpoints, stepping, the Chrome DevTools Protocol, the agents' API) is `pocket-debug`; this
//! module is what it needs from the host, and holds no breakpoint and no protocol.
//!
//! - A debugger attaches to a world's script host with [`attach`] as a [`DebugHook`], on the game
//!   thread that owns the world.
//! - A program is compiled *instrumented* (quickjs-ng PR #1421's `OP_debug` at every statement)
//!   only while [`DebugHook::instrument`] says so: `script.update` asks at the start of every tick
//!   and instantiates the installed program again when the answer changed. Programs hold no state,
//!   so this is a reload; uninstrumented, a program runs the stock engine's bytecode and the
//!   debugger costs nothing. The prelude (`pocket`) is never instrumented.
//! - Instrumented, QuickJS-ng calls [`DebugHook::trace`] at every statement, at `debugger;`
//!   statements and (P10) when an exception is thrown. The hook may stop the game thread inside
//!   that call (a pause) and inspect the stopped frames with the functions here and in [`ffi`];
//!   its own JavaScript (evaluations, conditions) runs under [`guarded`].
//! - The host tells the hook about programs ([`DebugHook::loaded`]), system calls
//!   ([`DebugHook::call_begin`], [`DebugHook::call_returned`], [`DebugHook::call_end`]) and console
//!   lines ([`DebugHook::console`]); [`FieldWatch`] reads what a running call has staged for one
//!   component field (a data breakpoint).
//!
//! [`ffi`]: crate::ffi

use std::cell::Cell;
use std::ffi::CStr;
use std::os::raw::{c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;

use bevy_ecs::prelude::World;
use pocket_sim::EntityId;
use pocket_sim::registry::FieldType;
use rquickjs::qjs::{self, JSAtom, JSContext};
use rquickjs::{Ctx, TypedArray};

use crate::call::{Comp, resolve_comp};
use crate::ffi;
use crate::host::{LogLine, Shared, shared};
use crate::program::SystemOutcome;
use crate::scripts::Scripts;
use crate::source::CompiledSet;

/// Why QuickJS-ng called the trace handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceKind {
    /// A statement is about to run.
    Statement,
    /// A `debugger;` statement.
    DebuggerStatement,
    /// An exception was thrown at this position (P10); `caught`: a `try` is predicted to catch it.
    Exception { caught: bool },
}

/// One trace call. The context and the atoms are valid only during the call.
#[derive(Clone, Copy, Debug)]
pub struct Trace {
    pub ctx: *mut JSContext,
    /// The module's name (a module path such as `scripts/rules.ts`, or `pocket`).
    pub file: JSAtom,
    /// The function's name (0 for an anonymous function or a module's body).
    pub function: JSAtom,
    /// 1-based line and column in the module's JavaScript.
    pub line: u32,
    pub column: u32,
    pub kind: TraceKind,
}

/// The system call a hook hears about.
#[derive(Clone, Copy, Debug)]
pub struct CallInfo<'a> {
    pub tick: u64,
    pub system: &'a str,
    /// The system's index in the program's run order.
    pub index: usize,
    /// The call's serial in this host (strictly increasing).
    pub serial: u64,
}

/// A debugger attached to a world's script host (game thread only). Every method is called by the
/// host on the thread that owns the world; none may panic across QuickJS-ng (the host catches a
/// panic and goes on) and none may hold a borrow across JavaScript it runs, since that JavaScript
/// can call [`DebugHook::console`] again.
pub trait DebugHook {
    /// Whether programs are to be instrumented now. Asked when a program is instantiated and at the
    /// start of every `script.update`; a change re-instantiates the installed program there.
    fn instrument(&self) -> bool;

    /// A program was instantiated from `set` (at attach, the installed one): its modules are what
    /// trace calls name. `instrumented` says whether its statements are traced.
    fn loaded(&self, set: &Arc<CompiledSet>, instrumented: bool);

    /// A system call is about to run its `run` (its queries are prepared).
    fn call_begin(&self, _call: &CallInfo<'_>) {}

    /// The call's `run` and its jobs have returned (or thrown); its staged writes are still
    /// readable ([`FieldWatch`]) and no frame is on the stack.
    fn call_returned(&self, _ctx: *mut JSContext, _call: &CallInfo<'_>) {}

    /// The call ended: committed, failed or faulted.
    fn call_end(&self, _call: &CallInfo<'_>, _outcome: &SystemOutcome) {}

    /// The trace handler (instrumented programs only).
    fn trace(&self, trace: &Trace);

    /// A script wrote a console line (`ctx` is the writing call's context; the writer's frame is
    /// level 1, the console function level 0).
    fn console(&self, _ctx: *mut JSContext, _line: &LogLine) {}
}

/// Attaches `hook` to the script host of `world` (or detaches with `None`), on the thread that owns
/// the world; tells it of the installed program. Returns false when the world has no script host.
pub fn attach(world: &mut World, hook: Option<Rc<dyn DebugHook>>) -> bool {
    let Some(scripts) = world.get_non_send_mut::<Scripts>() else {
        return false;
    };
    *scripts.host.shared.debug.borrow_mut() = hook.clone();
    if let (Some(h), Some(p)) = (hook, &scripts.program) {
        h.loaded(p.compiled_arc(), p.instrumented());
    }
    true
}

/// The trace handler QuickJS-ng calls; `opaque` is the host's `Shared`.
unsafe extern "C" fn trampoline(
    ctx: *mut JSContext,
    file: JSAtom,
    function: JSAtom,
    line: c_int,
    col: c_int,
    flags: c_int,
    opaque: *mut c_void,
) -> c_int {
    // SAFETY: the handler is installed with the host's `Shared`, which outlives every context of
    // the host (contexts are dropped before it).
    let sh = unsafe { &*(opaque as *const Shared) };
    let kind = if flags & ffi::JS_DEBUG_TRACE_EXCEPTION != 0 {
        TraceKind::Exception {
            caught: flags & ffi::JS_DEBUG_TRACE_EXCEPTION_CAUGHT != 0,
        }
    } else if flags & ffi::JS_DEBUG_TRACE_DEBUGGER_STMT != 0 {
        TraceKind::DebuggerStatement
    } else {
        TraceKind::Statement
    };
    let trace = Trace {
        ctx,
        file,
        function,
        line: u32::try_from(line).unwrap_or(0),
        column: u32::try_from(col).unwrap_or(0),
        kind,
    };
    // The borrow is shared: attaching (the only mutable borrow) never happens while scripts run.
    if let Ok(hook) = sh.debug.try_borrow()
        && let Some(h) = hook.as_ref()
    {
        let _ = catch_unwind(AssertUnwindSafe(|| h.trace(&trace)));
    }
    0
}

/// Installs the trace handler on `ctx` (`on`) or clears it. Code compiled while it is set is
/// instrumented; instrumented code calls it while it is set.
///
/// # Safety
/// `ctx` must be a live context of a script host, on its thread.
pub unsafe fn set_tracing(ctx: *mut JSContext, on: bool) {
    let Some(raw) = NonNull::new(ctx) else { return };
    // SAFETY: the caller hands a live context on its thread.
    let rctx = unsafe { Ctx::from_raw(raw) };
    let sh = shared(&rctx);
    unsafe { install(ctx, &sh, on) };
}

/// Installs or clears the handler with `sh` as its opaque.
pub(crate) unsafe fn install(ctx: *mut JSContext, sh: &Rc<Shared>, on: bool) {
    unsafe {
        if on {
            ffi::JS_SetDebugTraceHandler(ctx, Some(trampoline), Rc::as_ptr(sh) as *mut c_void);
        } else {
            ffi::JS_SetDebugTraceHandler(ctx, None, std::ptr::null_mut());
        }
    }
}

/// Runs `f`, a debugger's own JavaScript on the game thread while a script is stopped in `ctx` (an
/// evaluation, a breakpoint's condition, a property listing), under its own budget of
/// `steps_per_call` steps; then puts back the stopped call's budget, its overrun flag and its
/// out-of-memory baseline, so what `f` ran is not charged to the call. Memory `f` runs out of is
/// `f`'s failure, not the call's.
///
/// # Safety
/// `ctx` must be a live context of a script host, on its thread.
pub unsafe fn guarded<R>(ctx: *mut JSContext, f: impl FnOnce() -> R) -> R {
    let Some(raw) = NonNull::new(ctx) else {
        return f();
    };
    // SAFETY: the caller hands a live context on its thread.
    let rctx = unsafe { Ctx::from_raw(raw) };
    let sh = shared(&rctx);
    let rt = unsafe { qjs::JS_GetRuntime(ctx) };
    let active = sh.budget_active.get();
    let exceeded = sh.exceeded.get();
    let counter = unsafe { ffi::JS_GetInterruptCounter(ctx) };
    let oom = unsafe { ffi::JS_GetOutOfMemoryCount(rt) };
    let steps = i32::try_from(sh.limits.steps_per_call).unwrap_or(i32::MAX);
    unsafe { ffi::JS_SetInterruptCounter(ctx, steps) };
    sh.exceeded.set(false);
    sh.budget_active.set(true);
    let r = catch_unwind(AssertUnwindSafe(f));
    let oom_after = unsafe { ffi::JS_GetOutOfMemoryCount(rt) };
    sh.oom_base
        .set(sh.oom_base.get().wrapping_add(oom_after.wrapping_sub(oom)));
    unsafe { ffi::JS_SetInterruptCounter(ctx, counter) };
    sh.budget_active.set(active);
    sh.exceeded.set(exceeded);
    match r {
        Ok(r) => r,
        Err(p) => resume_unwind(p),
    }
}

/// An atom's text ("" for none).
///
/// # Safety
/// `ctx` must be a live context and `atom` valid in its runtime.
pub unsafe fn atom_text(ctx: *mut JSContext, atom: JSAtom) -> String {
    if atom == 0 {
        return String::new();
    }
    unsafe {
        let p = qjs::JS_AtomToCStringLen(ctx, std::ptr::null_mut(), atom);
        if p.is_null() {
            return String::new();
        }
        let s = CStr::from_ptr(p).to_string_lossy().into_owned();
        qjs::JS_FreeCString(ctx, p);
        s
    }
}

/// A frame on the stack, as `JS_GetStackFrameInfo` (P10) gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameInfo {
    /// A bytecode frame: its module, function and the 1-based position it is at (for a caller, its
    /// call; 0 when unknown).
    Script {
        file: String,
        function: String,
        line: u32,
        column: u32,
    },
    /// A native function's frame.
    Native,
}

/// The frame at `level` (0: the innermost), or `None` past the outermost.
///
/// # Safety
/// `ctx` must be a live context of a script host, on its thread.
pub unsafe fn frame_info(ctx: *mut JSContext, level: usize) -> Option<FrameInfo> {
    let level = c_int::try_from(level).ok()?;
    let (mut file, mut function, mut line, mut col) = (0, 0, 0, 0);
    let r = unsafe {
        ffi::JS_GetStackFrameInfo(ctx, level, &mut file, &mut function, &mut line, &mut col)
    };
    match r {
        0 => {
            let info = FrameInfo::Script {
                file: unsafe { atom_text(ctx, file) },
                function: unsafe { atom_text(ctx, function) },
                line: u32::try_from(line).unwrap_or(0),
                column: u32::try_from(col).unwrap_or(0),
            };
            unsafe {
                qjs::JS_FreeAtom(ctx, file);
                qjs::JS_FreeAtom(ctx, function);
            }
            Some(info)
        }
        1 => Some(FrameInfo::Native),
        _ => None,
    }
}

/// Frames on the stack, native ones included.
///
/// # Safety
/// `ctx` must be a live context.
pub unsafe fn stack_depth(ctx: *mut JSContext) -> usize {
    usize::try_from(unsafe { ffi::JS_GetStackDepth(ctx) }).unwrap_or(0)
}

/// One watched slot's staged value.
#[derive(Clone, Debug, PartialEq)]
pub enum WatchValue {
    Number(f64),
    Text(Arc<str>),
}

impl WatchValue {
    /// Equal bit for bit (a NaN equals itself, -0 differs from 0), as the host's write-back
    /// compares.
    pub fn same(&self, other: &WatchValue) -> bool {
        match (self, other) {
            (WatchValue::Number(a), WatchValue::Number(b)) => a.to_bits() == b.to_bits(),
            (WatchValue::Text(a), WatchValue::Text(b)) => a == b,
            _ => false,
        }
    }
}

/// What one component field (or every field) of one entity holds as a running system call has
/// staged it: the world as the call began, the query columns the call changed, then its own
/// commands (`world.set`, `insert`, `remove`, `despawn`); `None` when the entity lacks the
/// component (or is gone). This is a data breakpoint's probe (docs/spec/debugger.md 6): reading it
/// at every statement finds the statement after the one that changed the value.
pub struct FieldWatch {
    entity: EntityId,
    component: String,
    field: Option<String>,
    /// The call serial the cache below belongs to.
    serial: Cell<u64>,
    /// The component, its watched slots (`(name, Some(numeric slot) | None for a string index)`).
    resolved: std::cell::RefCell<Option<Resolved>>,
}

struct Resolved {
    comp: Comp,
    /// Per watched value: its name, and where it lives.
    slots: Vec<(String, Slot)>,
}

#[derive(Clone, Copy)]
enum Slot {
    Num(u16),
    Str(usize),
}

impl FieldWatch {
    /// A watch of `component.field` (every field when `None`) on entity `entity` (its id).
    pub fn new(entity: u64, component: &str, field: Option<&str>) -> Option<FieldWatch> {
        Some(FieldWatch {
            entity: EntityId::from_f64(entity as f64).ok()?,
            component: component.to_owned(),
            field: field.map(str::to_owned),
            serial: Cell::new(0),
            resolved: std::cell::RefCell::new(None),
        })
    }

    /// The names of the watched values (`position.x`, `taken`) once resolved in a call.
    pub fn names(&self) -> Vec<String> {
        self.resolved
            .borrow()
            .as_ref()
            .map(|r| r.slots.iter().map(|(n, _)| n.clone()).collect())
            .unwrap_or_default()
    }

    /// The staged values in the call running in `ctx`: `Err` with a reason when no call runs, the
    /// component is unknown or has no such field; `Ok(None)` when the entity lacks the component.
    ///
    /// # Safety
    /// `ctx` must be a live context of a script host, on its thread, with no JavaScript running
    /// above this call but the stopped frames.
    pub unsafe fn read(&self, ctx: *mut JSContext) -> Result<Option<Vec<WatchValue>>, String> {
        let raw = NonNull::new(ctx).ok_or("no context")?;
        // SAFETY: the caller hands a live context on its thread.
        let rctx = unsafe { Ctx::from_raw(raw) };
        let sh = shared(&rctx);
        let call = sh.call.borrow();
        let call = call.as_ref().ok_or("no system call is running")?;
        // SAFETY: JavaScript is stopped (the caller's contract): nothing else uses the world now.
        let world: &World = unsafe { &*call.world };
        if self.serial.get() != call.serial || self.resolved.borrow().is_none() {
            let comp = resolve_comp(world, &self.component).map_err(|e| e.message)?;
            let mut slots = Vec::new();
            let mut strings = 0usize;
            for f in comp.schema.fields.iter() {
                let wanted = self.field.as_deref().is_none_or(|w| w == &*f.name);
                if f.ty == FieldType::Str {
                    if wanted {
                        slots.push((f.name.to_string(), Slot::Str(strings)));
                    }
                    strings += 1;
                    continue;
                }
                if !wanted {
                    continue;
                }
                let n = f.ty.slots();
                for i in 0..n {
                    let name = if n > 1 {
                        format!("{}.{}", f.name, ["x", "y", "z", "w"][usize::from(i)])
                    } else {
                        f.name.to_string()
                    };
                    slots.push((name, Slot::Num(f.first_slot + i)));
                }
            }
            if slots.is_empty() {
                return Err(format!(
                    "{} has no field {}",
                    self.component,
                    self.field.as_deref().unwrap_or("")
                ));
            }
            *self.resolved.borrow_mut() = Some(Resolved { comp, slots });
            self.serial.set(call.serial);
        }
        let resolved = self.resolved.borrow();
        let r = resolved.as_ref().ok_or("unresolved")?;
        let commanded = call
            .overlay
            .entities
            .get(&self.entity)
            .is_some_and(|o| !o.alive || o.spawned || o.comps.contains_key(&r.comp.id));
        let Some(mut values) = call.overlay.value(world, self.entity, &r.comp) else {
            return Ok(None);
        };
        if !commanded {
            // Column write-backs apply before the commands; with no command on this component of
            // this entity, a changed cell is the staged value.
            for q in &call.queries {
                let Ok(row) = q.ids.binary_search(&self.entity) else {
                    continue;
                };
                for (comp, cols) in &q.comps {
                    if comp.id != r.comp.id {
                        continue;
                    }
                    for col in cols {
                        if let Some(v) = column_value(&rctx, col, row)
                            && col
                                .original
                                .get(row)
                                .is_some_and(|o| o.to_bits() != v.to_bits())
                            && let Some(slot) = values.nums.get_mut(usize::from(col.slot))
                        {
                            *slot = v;
                        }
                    }
                }
            }
        }
        let out = r
            .slots
            .iter()
            .map(|(_, s)| match *s {
                Slot::Num(i) => {
                    WatchValue::Number(values.nums.get(usize::from(i)).copied().unwrap_or(0.0))
                }
                Slot::Str(i) => WatchValue::Text(values.strs.get(i).cloned().unwrap_or_default()),
            })
            .collect();
        Ok(Some(out))
    }
}

/// A column's value at `row` now, or `None` when its buffer is gone.
fn column_value(ctx: &Ctx<'_>, col: &crate::call::Column, row: usize) -> Option<f64> {
    let object = col.array.clone().restore(ctx).ok()?;
    let array = TypedArray::<f64>::from_object(object).ok()?;
    // SAFETY: no JavaScript runs while the bytes are read.
    let bytes = unsafe { array.as_bytes() }?;
    let chunk = bytes.get(row * 8..row * 8 + 8)?;
    Some(f64::from_ne_bytes(chunk.try_into().ok()?))
}
