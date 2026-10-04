//! What crosses between the game and its presenters (docs/spec/threads.md 4 and 5;
//! architecture.md 4.7): the published snapshot and its reader, the event stream, the command
//! envelope with its source and sequence, the queue and the clients that send into it, and the
//! reply. Presenters (the renderer, the editor, the MCP server) link this crate instead of the game,
//! so they cannot mutate the world by construction.
//!
//! Slice 1 leaves out the wire form of the web worker and network spectators (threads.md 7.2) and
//! `ReplyValue::Fork`, which only the runtime's worker pool would use.

pub mod client;
pub mod command;
pub mod events;
pub mod logs;
pub mod reader;
pub mod snapshot;

pub use client::{GameClient, Lease, QUEUE_CAPACITY, QueueReceiver, QueueSender, Received, queue};
pub use command::{
    Envelope, Kind, Reply, ReplyTo, ReplyValue, Source, canonical_order, game_stopped, queue_full,
    source_from_json, source_in_use, source_json, tick_passed,
};
pub use events::{
    EventBatch, EventCursor, EventRecord, EventRing, RING_RECORDS, events_since, events_why,
};
pub use logs::{LOG_RECORDS, LogRecord, LogRing};
pub use reader::{
    GameStatus, LoopState, Publisher, SnapshotReader, WorldInfo, WorldMode, publication,
};
pub use snapshot::{PacingStatus, RegistryInfo, SnapshotView, TimeStatus, WorldSnapshot};
