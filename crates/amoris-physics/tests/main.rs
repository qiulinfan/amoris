//! amoris-physics's integration tests as one binary (docs/spec/checks.md 6.2): each file under
//! `tests/` is a module of it, so a change links the crate's tests once, not once per file.

mod added_mass;
mod board;
mod bodies;
mod collider_bed;
mod determinism;
mod field_buoyancy;
mod field_hull;
mod joints;
mod light_floats;
mod resident_swimming;
mod sailing;
mod sensors;
mod third_person;
mod vessel;
mod volumes;
mod walker_colliders;
mod web;
