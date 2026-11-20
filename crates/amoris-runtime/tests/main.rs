//! amoris-runtime's integration tests as one binary (docs/spec/checks.md 6.2): each file under
//! `tests/` is a module of it, so a change links the crate's tests once, not once per file.

mod common;

mod agent_reads;
mod edits;
mod game;
mod hierarchy;
mod history;
mod host;
mod ik;
mod joints;
mod lookahead;
mod morph;
mod player;
mod player_session;
mod present;
mod ragdoll;
mod save;
mod saves;
mod scenes;
mod schedule;
mod script_bodies;
mod script_intents;
mod script_profile;
mod sensors;
mod snapshot_cache;
mod terrain;
mod third_person;
mod thread;
mod thread_branches;
mod thread_player;
mod ui;
mod voxels;
mod water;
mod water_async;
mod water_checkpoint_codec;
mod water_feedback;
mod water_resident_async;
mod write_trace;
