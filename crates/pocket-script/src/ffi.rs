//! The vendored QuickJS-ng's own functions (script-sandbox.md 4.2), declared by hand: the crate's
//! pregenerated bindings predate the patches. The debugger's (quickjs-ng PR #1421, P9, P10) are
//! documented in docs/spec/debugger.md.

use std::os::raw::{c_char, c_int, c_void};

use rquickjs::qjs::{JSAtom, JSContext, JSRuntime, JSValue};

unsafe extern "C" {
    /// P2: how often memory ran out in the runtime (wrapping), including errors a builtin
    /// swallowed.
    pub fn JS_GetOutOfMemoryCount(rt: *mut JSRuntime) -> u32;
    /// P8: frees every pending job without running it.
    pub fn JS_DiscardPendingJobs(rt: *mut JSRuntime);
    /// P1: the polls left before the interrupt handler runs.
    pub fn JS_SetInterruptCounter(ctx: *mut JSContext, counter: i32);
    pub fn JS_GetInterruptCounter(ctx: *mut JSContext) -> i32;
    /// P5: nested calls past `limit` (0: none) throw an uncatchable InternalError.
    pub fn JS_SetCallDepthLimit(ctx: *mut JSContext, limit: u32);
    pub fn JS_GetCallDepth(ctx: *mut JSContext) -> u32;
}

/// P7: the C compiler that built QuickJS-ng.
pub const QJS_CC: &str = rquickjs::qjs::POCKET_QJS_CC;

/// Set in a trace call's `flags` when it fired on a `debugger;` statement (PR #1421).
pub const JS_DEBUG_TRACE_DEBUGGER_STMT: c_int = 1;
/// P10: the trace call reports an exception thrown at its position.
pub const JS_DEBUG_TRACE_EXCEPTION: c_int = 1 << 1;
/// P10: with `JS_DEBUG_TRACE_EXCEPTION`, a `try` is predicted to catch it.
pub const JS_DEBUG_TRACE_EXCEPTION_CAUGHT: c_int = 1 << 2;

/// The trace handler (PR #1421): called at every statement of code compiled while a handler was
/// set, at `debugger;` statements, and (P10) when a frame first sees a catchable exception.
/// Non-zero aborts the statement (ignored for exceptions).
pub type JSDebugTraceFunc = unsafe extern "C" fn(
    ctx: *mut JSContext,
    filename: JSAtom,
    funcname: JSAtom,
    line: c_int,
    col: c_int,
    flags: c_int,
    opaque: *mut c_void,
) -> c_int;

/// One variable of a frame (PR #1421).
#[repr(C)]
pub struct JSDebugLocalVar {
    pub name: *const c_char,
    pub value: JSValue,
    pub is_arg: bool,
    pub is_closure: bool,
    pub scope_level: c_int,
}

// quickjs-ng PR #1421's debugging interface, and P10's additions.
unsafe extern "C" {
    pub fn JS_SetDebugTraceHandler(
        ctx: *mut JSContext,
        cb: Option<JSDebugTraceFunc>,
        opaque: *mut c_void,
    );
    /// Frames on the stack, native ones included.
    pub fn JS_GetStackDepth(ctx: *mut JSContext) -> c_int;
    /// Arguments, locals, then closure variables of the frame at `level` (0: current).
    pub fn JS_GetLocalVariablesAtLevel(
        ctx: *mut JSContext,
        level: c_int,
        pvars: *mut *mut JSDebugLocalVar,
        pcount: *mut c_int,
    ) -> c_int;
    pub fn JS_FreeLocalVariables(ctx: *mut JSContext, vars: *mut JSDebugLocalVar, count: c_int);
    /// Sets an argument, local or closure variable of a frame: 0, or -1 (none), -2 (const), -3.
    pub fn JS_SetVariableAtLevel(
        ctx: *mut JSContext,
        level: c_int,
        name: *const c_char,
        value: JSValue,
    ) -> c_int;
    /// Evaluates `input` with the frame's arguments, locals and closure variables in scope.
    pub fn JS_EvalInStackFrame(
        ctx: *mut JSContext,
        level: c_int,
        input: *const c_char,
        input_len: usize,
        filename: *const c_char,
    ) -> JSValue;
    /// P10: the exception a `JS_DEBUG_TRACE_EXCEPTION` call reports (a new reference).
    pub fn JS_GetDebugTraceException(ctx: *mut JSContext) -> JSValue;
    /// P10: the file, function (new atoms, freed by the caller) and position of the frame at
    /// `level`: 0 for a bytecode frame, 1 for a native one, -1 past the stack.
    pub fn JS_GetStackFrameInfo(
        ctx: *mut JSContext,
        level: c_int,
        filename: *mut JSAtom,
        funcname: *mut JSAtom,
        line: *mut c_int,
        col: *mut c_int,
    ) -> c_int;
}
