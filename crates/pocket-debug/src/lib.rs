//! The script debugger (charter 4.3; docs/spec/debugger.md): one core, three frontends.
//!
//! - The core is a [`DebugHub`]: breakpoints (TypeScript or JavaScript lines, conditions,
//!   logpoints), `debugger;` statements, pause and step, pause on exceptions, data breakpoints on
//!   component fields, console capture. Its game-thread half is a hook the script host calls at
//!   every statement of an instrumented program ([`DebugHub::hook`], attached with
//!   `pocket_runtime::Game::set_script_debugger`); a pause stops only the game thread, inside that
//!   call, while rendering and the servers go on. Scripts are instrumented only while a frontend
//!   is attached: the host instantiates them again at a tick boundary when that changes, so a game
//!   nobody debugs runs the stock engine's bytecode.
//! - Chrome DevTools and VS Code's js-debug attach to [`DebugHub::serve_cdp`]'s Chrome DevTools
//!   Protocol endpoint (127.0.0.1:9229 by default) and see the TypeScript through source maps.
//! - Agents call [`DebugHub::call`] with the host protocol's `debug.*` methods ([`methods`]) and
//!   hear [`DebugEvent`]s from [`DebugHub::subscribe`]; `pocket-server` and `pocket-mcp` carry
//!   both.
//!
//! Native only: the web build has the trace hook but no transport yet.

mod agent;
mod cdp;
mod game;
mod hub;
pub mod inspect;
mod model;
pub mod scripts;

pub use agent::{Method, methods};
pub use cdp::{CdpOptions, CdpServer, TARGET_ID, TARGET_PATH};
pub use hub::{DebugHub, HubOptions};
pub use model::{
    ConsoleLine, DebugEvent, ExceptionMode, Location, Pause, Reason, ScriptException, StepKind,
    WatchHit,
};
pub use scripts::SourceRoot;
