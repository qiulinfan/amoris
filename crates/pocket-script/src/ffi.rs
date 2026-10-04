//! The vendored QuickJS-ng's own functions (script-sandbox.md 4.2), declared by hand: the crate's
//! pregenerated bindings predate the patches.

use rquickjs::qjs::{JSContext, JSRuntime};

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
