//! A world without physics for the lifecycle's own tests: one body, `Body`, the seat `hand`'s, a
//! latched `lever` on channel `arm`, a `button` pulse on `hands`, a `mode` choice and a `lamp`
//! toggle, and the intent `hold_for`, whose executor pulls the lever one notch a tick and reaches
//! its goal at its `ticks`-th run, so each transition happens at a tick the test chooses.

use std::sync::Arc;

use bevy_ecs::prelude::World;
use pocket_contract::{Problem, Shape, Use};
use pocket_interface::action::catalog::{ActionCatalog, IntentEntry, SeatDecl};
use pocket_interface::action::defs::{
    CodeDef, ControlBinding, ControlDef, ControlKind, IntentDef, ProgressDef, TargetSpec,
};
use pocket_interface::action::executor::{AcceptCx, Accepted, IntentExecutor, Step, TickCx};
use pocket_interface::action::view::Nothing;
use pocket_interface::action::{Caller, IntentStatus, IntentTable};
use pocket_interface::perception::Unit;
use pocket_sim::{EntityId, Name, NoHooks, PlainData, Sim, SimConfig, TickRate};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

/// `hold_for`'s parameters.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct HoldForParams {
    /// The run at which the goal is met.
    #[schemars(range(min = 1, max = 100000))]
    pub ticks: u32,
    /// Hold once reached.
    #[serde(default)]
    pub keep: Option<bool>,
    /// Seconds until it must be reached.
    #[serde(default)]
    #[schemars(range(min = 0.01, max = 600.0))]
    pub timeout_s: Option<f64>,
    /// Refuse at acceptance (the executor's own refusal).
    #[serde(default)]
    pub refuse: Option<bool>,
}

/// `hold_for`'s executor.
pub struct HoldFor {
    pub def: IntentDef,
}

fn count(state: &PlainData) -> f64 {
    match state.get("n") {
        Some(PlainData::Number(x)) => *x,
        _ => 0.0,
    }
}

impl IntentExecutor for HoldFor {
    fn def(&self) -> &IntentDef {
        &self.def
    }

    fn accept(&self, cx: &AcceptCx<'_>) -> Result<Accepted, Vec<Problem>> {
        if matches!(cx.params.get("refuse"), Some(PlainData::Bool(true))) {
            return Err(vec![cx.catalog.problem(
                "fixture.refused",
                pocket_contract::detail([("why", json!("asked to"))]),
            )]);
        }
        Ok(Accepted {
            state: PlainData::object(vec![("n".to_owned(), PlainData::Number(0.0))]).unwrap(),
            progress: vec![("count".into(), PlainData::Number(0.0))],
            warnings: Vec::new(),
        })
    }

    fn tick(&self, cx: &mut TickCx<'_, '_>) -> Step {
        let n = count(cx.state) + 1.0;
        *cx.state = PlainData::object(vec![("n".to_owned(), PlainData::Number(n))]).unwrap();
        cx.controls.axis("lever", pocket_sim::math::min(n, 100.0));
        let target = match cx.instance.params.get("ticks") {
            Some(PlainData::Number(x)) => *x,
            _ => 1.0,
        };
        let progress = vec![("count".into(), PlainData::Number(n))];
        if n >= target && cx.instance.status == IntentStatus::Active {
            Step::Reached { progress }
        } else {
            Step::Continue { progress }
        }
    }
}

fn control(name: &str, kind: ControlKind, channel: &str) -> ControlDef {
    ControlDef {
        name: name.into(),
        doc: format!("The fixture's {name}."),
        kind,
        channel: channel.into(),
        binding: ControlBinding::Unbound,
    }
}

/// `Body` is the seat `hand`'s, `Other` the seat `foot`'s.
fn bodies(w: &World) -> Vec<(String, EntityId)> {
    let index = w.resource::<pocket_sim::EntityIndex>();
    index
        .iter()
        .filter_map(|(id, e)| match w.get::<Name>(e).map(Name::as_str) {
            Some("Body") => Some(("hand".to_owned(), id)),
            Some("Other") => Some(("foot".to_owned(), id)),
            _ => None,
        })
        .collect()
}

/// The fixture's catalog.
pub fn catalog() -> ActionCatalog {
    let mut c = ActionCatalog::empty(
        "fixture",
        bodies,
        Arc::new(|w: &World, row: &pocket_interface::action::SeatRow| {
            Box::new(Nothing::new(w, row.body))
                as Box<dyn pocket_interface::action::view::ActorView>
        }),
    );
    c.seats = vec![SeatDecl::any("hand"), SeatDecl::any("foot")];
    c.controls = vec![
        control(
            "lever",
            ControlKind::Axis {
                min: 0.0,
                max: 100.0,
                default: 0.0,
            },
            "arm",
        ),
        control("button", ControlKind::Pulse { target_kinds: None }, "hands"),
        control(
            "mode",
            ControlKind::Choice {
                values: vec!["slow".into(), "fast".into()],
                default: "slow".into(),
            },
            "dial",
        ),
        control("lamp", ControlKind::Toggle { default: false }, "dial"),
    ];
    let def = IntentDef {
        name: "hold_for".into(),
        doc: "Pull the lever a notch a tick; reached at the ticks-th run.".into(),
        params: Shape::of::<HoldForParams>(),
        defaults: json!({"keep": false, "timeout_s": 10, "refuse": false})
            .as_object()
            .cloned()
            .unwrap(),
        target: TargetSpec::None,
        channels: vec!["arm".into()],
        can_hold: true,
        default_timeout_s: 10.0,
        failures: vec!["intent.timeout".into()],
        progress: vec![ProgressDef {
            name: "count".into(),
            doc: "Runs so far.".into(),
            unit: Unit::Count,
            precision: 0,
        }],
    };
    let exec: Arc<dyn IntentExecutor> = Arc::new(HoldFor { def: def.clone() });
    c.intents = vec![IntentEntry {
        def,
        executor: Some(exec),
    }];
    c.codes = vec![CodeDef {
        code: "fixture.refused".into(),
        uses: vec![Use::Refuse],
        when: "Asked to refuse.".into(),
        detail: vec!["why".into()],
        template: "Refused because {why}.".into(),
    }];
    c
}

/// The fixture world at tick 0 and the body.
pub fn world() -> (Sim, EntityId) {
    with_rules(Default::default(), false)
}

/// The fixture world under `rules`, with the second seat's body `Other` when `two`.
pub fn with_rules(rules: pocket_interface::time::turns::TimeRules, two: bool) -> (Sim, EntityId) {
    let mut sim = Sim::new(SimConfig {
        rate: TickRate::DEFAULT,
        seed: 3,
    })
    .unwrap();
    pocket_interface::action::plugin(&mut sim, catalog()).unwrap();
    pocket_interface::time::turns::plugin(&mut sim, rules).unwrap();
    let id = sim.boundary().spawn((Name::new("Body").unwrap(),)).unwrap();
    if two {
        sim.boundary()
            .spawn((Name::new("Other").unwrap(),))
            .unwrap();
    }
    (sim, id)
}

/// An act for `seat` as its player.
pub fn act_as(
    sim: &mut Sim,
    seat: &str,
    req: Value,
) -> Result<pocket_interface::action::ActDone, Problem> {
    let caller = Caller::Player { seat: seat.into() };
    pocket_interface::action::act(sim.world_mut(), &caller, &req)
}

/// An act for the seat `hand` as its player.
pub fn act(sim: &mut Sim, req: Value) -> Result<pocket_interface::action::ActDone, Problem> {
    let caller = Caller::Player {
        seat: "hand".into(),
    };
    pocket_interface::action::act(sim.world_mut(), &caller, &req)
}

/// Starts `hold_for` with `params` and answers its id.
pub fn hold(sim: &mut Sim, params: Value) -> u64 {
    let done = act(
        sim,
        json!({"actions": [{"do": "start", "intent": "hold_for", "params": params}]}),
    )
    .unwrap();
    done.outcomes[0]["intent_id"].as_u64().unwrap()
}

pub fn step(sim: &mut Sim) {
    sim.step(&mut NoHooks).unwrap();
}

pub fn status(sim: &Sim, id: u64) -> IntentStatus {
    sim.world().resource::<IntentTable>().by_id[&id].status
}

/// The kinds of the events appended in the last tick and the boundary writes since.
pub fn kinds(sim: &Sim) -> Vec<String> {
    sim.world()
        .resource::<pocket_sim::EventInbox>()
        .events()
        .iter()
        .map(|e| e.kind.as_str().to_owned())
        .collect()
}

/// The lever's latched value.
pub fn lever(sim: &Sim, body: EntityId) -> Option<f64> {
    let e = pocket_sim::entity::require(sim.world(), body).unwrap();
    match sim
        .world()
        .get::<pocket_interface::action::Controls>(e)?
        .values
        .get("lever")?
    {
        pocket_interface::action::state::ControlValue::Number(x) => Some(*x),
        _ => None,
    }
}
