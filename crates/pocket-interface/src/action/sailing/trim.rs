//! `trim_sail` (shared/contract/sailing.md, `trim_sail`): set the sail and trim the sheet to a
//! value or to the best trim for the apparent wind, and with `keep` go on trimming as it changes.

use pocket_contract::{Problem, detail};
use pocket_physics::best_sheet;
use pocket_sim::PlainData;
use serde_json::{Value, json};

use super::sail_problem;
use crate::action::catalog::{param_f64, param_str};
use crate::action::defs::IntentDef;
use crate::action::executor::{AcceptCx, Accepted, IntentExecutor, Step, TickCx};
use crate::action::view::{self, ActorView};

/// `trim_sail`.
pub struct TrimSail {
    pub def: IntentDef,
}

/// The sheet the parameters ask for now: a number, or `best_sheet` of the apparent wind.
fn target(params: &PlainData, v: &dyn ActorView) -> f64 {
    match param_f64(params, "sheet") {
        Some(x) => x,
        None => best_sheet(view::number(v, "awa_deg").unwrap_or(180.0)),
    }
}

fn progress(v: &dyn ActorView, target: f64) -> Vec<(String, PlainData)> {
    let text = |name: &str| {
        v.instrument(name)
            .and_then(|x| x.as_str().map(str::to_owned))
            .unwrap_or_default()
    };
    vec![
        (
            "sheet".into(),
            PlainData::Number(view::number(v, "sheet").unwrap_or(0.0)),
        ),
        ("target_sheet".into(), PlainData::Number(target)),
        ("trim".into(), PlainData::String(text("trim"))),
        (
            "drive".into(),
            PlainData::Number(view::number(v, "drive").unwrap_or(0.0)),
        ),
    ]
}

/// `sail.not_set` with its suggestion: the sail is furled and no `hoist` is given.
pub fn not_set(catalog: &crate::action::ActionCatalog) -> Problem {
    sail_problem(
        catalog,
        "sail.not_set",
        detail([("hoist", json!(0)), ("suggest", json!({"hoist": 1}))]),
    )
}

impl IntentExecutor for TrimSail {
    fn def(&self) -> &IntentDef {
        &self.def
    }

    fn accept(&self, cx: &AcceptCx<'_>) -> Result<Accepted, Vec<Problem>> {
        let furled = view::number(cx.view, "hoist").unwrap_or(0.0) <= 0.0;
        if furled && param_f64(cx.params, "hoist").is_none() {
            return Err(vec![not_set(cx.catalog)]);
        }
        let t = target(cx.params, cx.view);
        Ok(Accepted {
            state: PlainData::Object(Vec::new()),
            progress: progress(cx.view, t),
            warnings: Vec::new(),
        })
    }

    fn tick(&self, cx: &mut TickCx<'_, '_>) -> Step {
        let p = &cx.instance.params;
        let tolerance = param_f64(p, "tolerance").unwrap_or(0.02);
        let hoist = param_f64(p, "hoist");
        if let Some(h) = hoist {
            cx.controls.axis("hoist", h);
        }
        let t = target(p, cx.view);
        cx.controls.axis("sheet", t);
        let prog = progress(cx.view, t);
        let sheet = view::number(cx.view, "sheet").unwrap_or(0.0);
        let hoisted =
            hoist.is_none_or(|h| (view::number(cx.view, "hoist").unwrap_or(0.0) - h).abs() <= 0.02);
        let _ = param_str(p, "sheet");
        if cx.instance.status == crate::action::state::IntentStatus::Active
            && (sheet - t).abs() <= tolerance
            && hoisted
        {
            return Step::Reached { progress: prog };
        }
        Step::Continue { progress: prog }
    }
}

/// `trim_sail`'s parameters' defaults.
pub fn defaults() -> serde_json::Map<String, Value> {
    let v = json!({"sheet": "best", "tolerance": 0.02, "keep": false, "timeout_s": 20});
    v.as_object().cloned().unwrap_or_default()
}
