//! Projections of perception's answers (shared/contract/projection.md): the rounding every
//! projection shares ([`round`]), the text an LLM reads ([`text`], shared byte for byte between
//! the two lines), the JSON ([`json`]) and the tensors of an RL observer ([`tensor`]). What is
//! perceived is perception's (`crate::perception`); these modules only write it.
//!
//! The JSON projection is written field by field in declaration order rather than through
//! `serde`'s derive, so a value is rounded where it is written and parts other layers render (an
//! active intent, a pending decision) are spliced in as they wrote them.

pub mod json;
pub mod round;
pub mod tensor;
pub mod text;

use std::collections::BTreeMap;

use pocket_sim::Tick;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::perception::state::Named;

/// How an answer is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Projection {
    Text,
    Json,
    Tensor,
}

/// A part of an observation another layer renders, in both projections: an active intent
/// (actions.md, `IntentView`, as its `intent` text line) or a pending decision (time.md,
/// `DecisionPoint`, as its `decision` line). `json` is one JSON value, `text` one line without
/// its line end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rendered {
    pub json: String,
    pub text: String,
}

/// An observation's head.
#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    pub tick: Tick,
    pub t_s: f64,
    pub seat: Option<String>,
    pub observer: Option<Named>,
    pub omniscient: bool,
}

/// What an answer left out (perception.md, Token budgets, step 5).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Omitted {
    /// Kind to how many ranked percepts were left out.
    pub entities: BTreeMap<String, u32>,
    /// Perceived events after `cursor` not included.
    pub events: u32,
    /// Events after `since` already overwritten in the ring.
    pub lost: u32,
    /// The cursor the `events` hint continues from.
    pub cursor: u64,
}

impl Omitted {
    /// Whether anything was left out.
    pub fn any(&self) -> bool {
        !self.entities.is_empty() || self.events > 0 || self.lost > 0
    }
}

/// The deterministic token estimate: `ceil(bytes / 4)` (README, Token estimates).
pub fn tokens(bytes: usize) -> u32 {
    u32::try_from(bytes.div_ceil(4)).unwrap_or(u32::MAX)
}

/// The bytes the JSON projection reserves for its `tokens` member (perception.md, Token budgets).
pub const TOKENS_RESERVE: usize = 16;
