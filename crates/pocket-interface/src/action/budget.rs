//! The token budgets of `intents` and `affordances` (shared/contract/actions.md; perception.md,
//! Token budgets): the answer keeps the longest prefix of its list whose compact JSON, with the
//! count of what was left out and the 16 bytes reserved for `tokens`, fits `budget_tokens * 4`
//! bytes, and reports its `tokens`, the estimate of the answer without that member. A budget that
//! cannot hold the answer with an empty list is refused with `perception.budget_too_small`.

use bevy_ecs::prelude::World;
use pocket_contract::Problem;
use serde_json::{Map, Value};

use crate::projection::{TOKENS_RESERVE, tokens};

/// The default budget when a request names none: the seat's observer profile's, as perception's
/// queries default (400 for a seat without an observer).
pub fn default_for(world: &World, seat: &str) -> u32 {
    crate::perception::PerceptionView::for_seat(world, seat)
        .map_or(400, |v| v.profile().budget_tokens)
}

/// `head` with the first items of `items` under `key` and, when `always` or anything was left out,
/// their count under `omitted`, cut to `budget` (checked: 1 to 16000), with `tokens` added.
pub fn fit(
    head: Map<String, Value>,
    key: &str,
    items: Vec<Value>,
    budget: Option<u32>,
    default: u32,
    always: bool,
) -> Result<Value, Problem> {
    let budget = crate::perception::query::check_budget(budget, default)?;
    let limit = usize::try_from(budget)
        .unwrap_or(usize::MAX)
        .saturating_mul(4);
    let lens: Vec<usize> = items.iter().map(|v| v.to_string().len()).collect();
    let shell = |left: usize| {
        let mut m = head.clone();
        m.insert(key.to_owned(), Value::Array(Vec::new()));
        if always || left > 0 {
            m.insert("omitted".to_owned(), Value::from(left));
        }
        Value::Object(m).to_string().len()
    };
    // An item adds its own bytes and, after the first, a comma.
    let size =
        |k: usize| shell(items.len() - k) + lens[..k].iter().sum::<usize>() + k.saturating_sub(1);
    let Some(k) = (0..=items.len())
        .rev()
        .find(|k| size(*k) + TOKENS_RESERVE <= limit)
    else {
        let least = (0..=items.len()).map(size).min().unwrap_or(0) + TOKENS_RESERVE;
        return Err(pocket_contract::codes::budget_too_small(
            u64::from(budget),
            u64::from(tokens(least)),
        ));
    };
    let left = items.len() - k;
    let mut m = head;
    m.insert(
        key.to_owned(),
        Value::Array(items.into_iter().take(k).collect()),
    );
    if always || left > 0 {
        m.insert("omitted".to_owned(), Value::from(left));
    }
    let shown = Value::Object(m.clone()).to_string().len();
    m.insert("tokens".to_owned(), Value::from(tokens(shown)));
    Ok(Value::Object(m))
}
