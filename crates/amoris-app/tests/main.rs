//! amoris-app's integration tests as one binary (docs/spec/checks.md 6.2): each file under
//! `tests/` is a module of it, so a change links the crate's tests once, not once per file.

#[allow(dead_code)]
#[path = "../src/water_compute.rs"]
mod water_compute;

mod agent_sessions;
mod assets_upload;
mod debug_agent;
mod gpu_authoring;
mod gpu_boundaries;
mod gpu_collider_bed;
mod gpu_determinism;
mod gpu_energy;
mod gpu_feedback;
mod gpu_floaters;
mod gpu_frontier_scenes;
mod gpu_scale;
mod gpu_sea;
mod gpu_swimming;
mod gpu_water;
mod gpu_weather;
mod history_host;
mod locked_host;
mod paused_host;
mod player_host;
mod sdk_types;
mod serve_ready;
