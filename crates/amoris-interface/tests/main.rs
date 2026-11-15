//! amoris-interface's integration tests as one binary (docs/spec/checks.md 6.2): each file under
//! `tests/` is a module of it, so a change links the crate's tests once, not once per file.

mod common;
mod perception_support;

mod actions;
mod explore;
mod lifecycle;
mod pacing;
mod perception_checks;
mod perception_declarations;
mod perception_golden;
mod perception_noninterference;
mod perception_team;
mod perception_visibility;
mod player_view;
mod sailing;
mod time;
mod timing;
mod transparency;
