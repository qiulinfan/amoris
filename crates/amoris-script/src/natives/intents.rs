//! Intents in scripts (docs/spec/script-host.md 5.5): what an executor is handed and may do, and
//! `ctx.intent`'s act.
//!
//! - **Handing.** Before an executor's call, [`hand`] collects the live instances of its kinds that
//!   have started, in `IntentId` order, with each one's seat and channels: [`ExecutorScope`] in the
//!   call state. An instance whose seat lost its body fails first, as `interface.intents` fails a
//!   Rust one.
//! - **Reading.** `ctx.intents` and `ctx.view(intent)` read the instances and the seat's
//!   perception (actions.md, Executors, rule 1); nothing else of the world is reachable, since
//!   `natives::with_call` refuses every other native during an executor's call.
//! - **Writing.** Control writes, progress, state, reached, succeed and reject are checked at the
//!   call and staged as [`IntentCommand`]s; [`apply`] commits them through
//!   `amoris_interface::action::lifecycle`, the one way an instance changes, so a script executor
//!   emits the events a Rust one does.
//! - **Acting.** `ctx.intent` (natives::events) validates its act with `action::check` at the
//!   call; [`apply_acts`] applies the call's acts first at the commit.

use amoris_contract::Problem;
use amoris_interface::action::catalog::{ActionCatalog, queue_pulse, write_control};
use amoris_interface::action::defs::{ControlKind, IntentDef};
use amoris_interface::action::executor::channels_of;
use amoris_interface::action::lifecycle::{self, End};
use amoris_interface::action::state::{
    ControlValue, IntentTable, SeatRow, StoredProblem, seats_with,
};
use amoris_interface::action::view::{ActorView, Known, KnownEvent};
use amoris_interface::action::{Caller, intent_view_in};
use amoris_sim::{EntityId, PlainData, Tick};
use bevy_ecs::prelude::World;
use rquickjs::{Array, Ctx, Value};
use serde_json::{Map, Value as Json, json};

use super::{NResult, arg, with_call};
use crate::call::{CallState, Command};
use crate::convert::{freeze_new, plain_from_js, plain_to_js};
use crate::error::{ErrorPhase, ScriptError};
use crate::js;

/// One instance an executor's call was handed.
pub struct Handed {
    pub id: u64,
    pub intent: String,
    pub seat: SeatRow,
    pub actor: EntityId,
    /// The channels it occupies: what `ctx.controls(intent)` may write.
    pub channels: Vec<String>,
}

/// What an executor's call may act on.
#[derive(Default)]
pub struct ExecutorScope {
    /// In `IntentId` order.
    pub handed: Vec<Handed>,
}

/// An executor's command on one of its instances, applied at the commit in call order.
pub enum IntentCommand {
    Set {
        id: u64,
        actor: EntityId,
        control: String,
        value: ControlValue,
    },
    Pulse {
        id: u64,
        actor: EntityId,
        control: String,
        target: Option<EntityId>,
    },
    Progress {
        id: u64,
        readings: Vec<(String, PlainData)>,
    },
    State {
        id: u64,
        state: PlainData,
    },
    Reached {
        id: u64,
    },
    Succeed {
        id: u64,
    },
    Reject {
        id: u64,
        problem: Problem,
    },
}

/// The live instances of `kinds` that have started by `tick`, in `IntentId` order.
fn due_ids(world: &World, kinds: &[String], tick: Tick) -> Vec<u64> {
    let Some(table) = world.get_resource::<IntentTable>() else {
        return Vec::new();
    };
    table
        .by_id
        .values()
        .filter(|i| i.status.is_live() && i.started_tick <= tick && kinds.contains(&i.intent))
        .map(|i| i.id)
        .collect()
}

/// Whether an executor of `kinds` has an instance to carry out in `tick`.
pub fn due(world: &World, kinds: &[String], tick: Tick) -> bool {
    !due_ids(world, kinds, tick).is_empty()
}

/// The instances an executor of `kinds` is handed in `tick`. One whose seat has no body any more
/// fails with `internal.error` before the call, as `interface.intents` fails a Rust one.
pub(crate) fn hand(world: &mut World, kinds: &[String], tick: Tick) -> ExecutorScope {
    let ids = due_ids(world, kinds, tick);
    let Some(catalog) = world.get_resource::<ActionCatalog>() else {
        return ExecutorScope::default();
    };
    let rows = seats_with(world, catalog).unwrap_or_default();
    let mut handed = Vec::new();
    let mut lost = Vec::new();
    let table = world.resource::<IntentTable>();
    for id in ids {
        let Some(i) = table.by_id.get(&id) else {
            continue;
        };
        match rows.iter().find(|r| r.id == i.seat && r.body == i.actor) {
            Some(row) => handed.push(Handed {
                id,
                intent: i.intent.clone(),
                seat: row.clone(),
                actor: i.actor,
                channels: channels_of(catalog, &i.intent, &i.params),
            }),
            None => lost.push(id),
        }
    }
    for id in lost {
        lifecycle::broke_contract(world, id, "the intent's seat has no body any more");
    }
    ExecutorScope { handed }
}

/// `script.restricted`.
pub fn restricted_error(what: &str, why: &str) -> ScriptError {
    ScriptError::new(
        "script.restricted",
        format!("{what} is not available to an intent executor: {why}."),
        ErrorPhase::Run,
    )
    .with("what", json!(what))
}

/// The natives an executor's call may reach: its own, its system stream's draws and
/// `Math.random` (script-host.md 5.5).
pub fn allowed_in_executor(what: &str) -> bool {
    what.starts_with("executor.") || matches!(what, "ctx.rng" | "a random stream" | "Math.random")
}

/// `ctx.world` and the other members an executor's context lacks: they throw.
pub fn restricted<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let what = match js::text(&arg(ctx, args, 0)) {
        Some(Ok(t)) => t,
        _ => "this".to_owned(),
    };
    Err(restricted_error(
        &what,
        "it sees only its seat's perception and acts only through the intent's controls",
    )
    .into())
}

fn bad_data(message: String) -> ScriptError {
    ScriptError::new("script.bad_data", message, ErrorPhase::Run)
}

fn intent_id(v: &Value<'_>) -> Option<u64> {
    let x = v.as_number()?;
    (x.fract() == 0.0 && (1.0..9.0e15).contains(&x)).then(|| {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // checked above
        let id = x as u64;
        id
    })
}

/// The handed instance `v` names, by its id.
fn handed<'c>(call: &'c CallState, v: &Value<'_>) -> Result<&'c Handed, ScriptError> {
    let scope = call
        .executor
        .as_ref()
        .ok_or_else(|| restricted_error("an executor's call", "this is not an executor"))?;
    let id = intent_id(v);
    scope
        .handed
        .iter()
        .find(|h| Some(h.id) == id)
        .ok_or_else(|| bad_data("An intent must be one of this call's ctx.intents.".into()))
}

fn catalog(world: &World) -> Result<&ActionCatalog, ScriptError> {
    world.get_resource::<ActionCatalog>().ok_or_else(|| {
        ScriptError::new(
            "sim.internal",
            "An executor runs in a world without an action catalog.",
            ErrorPhase::Run,
        )
    })
}

fn declared<'a>(catalog: &'a ActionCatalog, h: &Handed) -> Result<&'a IntentDef, ScriptError> {
    catalog.intent(&h.intent).map(|e| &e.def).ok_or_else(|| {
        ScriptError::new(
            "sim.internal",
            format!("The intent {} is not in the catalog.", h.intent),
            ErrorPhase::Run,
        )
    })
}

fn to_js<'js>(ctx: &Ctx<'js>, v: &Json) -> rquickjs::Result<Value<'js>> {
    plain_to_js(ctx, &PlainData::from_json(v))
}

/// `ctx.intents`: the handed instances as [contract]'s `IntentView` with `state`, frozen.
pub fn intents<'js>(ctx: &Ctx<'js>, _args: &[Value<'js>]) -> NResult<'js> {
    let views = with_call(ctx, "executor.intents", |call, world| {
        let scope = call
            .executor
            .as_ref()
            .ok_or_else(|| restricted_error("ctx.intents", "this is not an executor"))?;
        let catalog = catalog(world)?;
        let table = world.resource::<IntentTable>();
        let mut out = Vec::new();
        for h in &scope.handed {
            let Some(i) = table.by_id.get(&h.id) else {
                continue;
            };
            let view = (catalog.view)(world, &h.seat);
            let mut v = intent_view_in(world, Some(&*view), i);
            v["state"] = i.state.to_json();
            out.push(v);
        }
        Ok(out)
    })?;
    let a = Array::new(ctx.clone())?;
    for (i, v) in views.iter().enumerate() {
        a.set(i, to_js(ctx, v)?)?;
    }
    Ok(freeze_new(ctx, a.into_object())?)
}

fn known_json(k: &Known) -> Json {
    let mut facts = Map::new();
    for (n, v) in &k.facts {
        facts.insert(n.clone(), v.clone());
    }
    let mut v = json!({
        "id": k.id.get(),
        "kind": k.kind,
        "visibility": k.visibility.name(),
        "pos_m": {"x": k.pos_m[0], "y": k.pos_m[1], "z": k.pos_m[2]},
        "range_m": k.range_m,
        "bearing_deg": k.bearing_deg,
        "facts": facts,
    });
    if let Some(n) = &k.name {
        v["name"] = json!(n);
    }
    v
}

fn event_json(e: &KnownEvent) -> Json {
    let mut data = Map::new();
    for (n, v) in &e.data {
        data.insert(n.clone(), v.clone());
    }
    let mut v = json!({"seq": e.seq, "tick": e.tick.0, "kind": e.kind, "data": data});
    if let Some(s) = e.world_seq {
        v["world_seq"] = json!(s);
    }
    if let Some(s) = e.subject {
        v["subject"] = json!(s.get());
    }
    v
}

/// Runs `f` over the seat's view of the handed instance `args[0]` names.
fn with_view<'js, R>(
    ctx: &Ctx<'js>,
    args: &[Value<'js>],
    what: &str,
    f: impl FnOnce(&dyn ActorView) -> Result<R, ScriptError>,
) -> Result<R, ScriptError> {
    let id = arg(ctx, args, 0);
    with_call(ctx, what, |call, world| {
        let h = handed(call, &id)?;
        let catalog = catalog(world)?;
        let view = (catalog.view)(world, &h.seat);
        f(&*view)
    })
}

fn optional<'js>(ctx: &Ctx<'js>, v: Option<Json>) -> NResult<'js> {
    match v {
        Some(v) => Ok(to_js(ctx, &v)?),
        None => Ok(Value::new_undefined(ctx.clone())),
    }
}

/// `view.instrument(name)`.
pub fn view_instrument<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let name = match js::text(&arg(ctx, args, 1)) {
        Some(Ok(t)) => t,
        _ => return Err(bad_data("An instrument is named by a string.".into()).into()),
    };
    let v = with_view(
        ctx,
        args,
        "executor.view",
        |view| Ok(view.instrument(&name)),
    )?;
    optional(ctx, v)
}

fn entity_arg(v: &Value<'_>, what: &str) -> Result<EntityId, ScriptError> {
    v.as_number()
        .and_then(|x| EntityId::from_f64(x).ok())
        .ok_or_else(|| bad_data(format!("{what} is an entity.")))
}

/// `view.percept(e)`: the entity as the seat knows it, or `undefined`.
pub fn view_percept<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let e = entity_arg(&arg(ctx, args, 1), "view.percept's argument")?;
    let v = with_view(ctx, args, "executor.view", |view| {
        Ok(view.known(e).map(|k| known_json(&k)))
    })?;
    optional(ctx, v)
}

/// What `view.nearby` filters by.
#[derive(Default)]
struct Nearby {
    kinds: Option<Vec<String>>,
    within_m: Option<f64>,
    visibility: Option<Vec<String>>,
    limit: usize,
}

fn nearby_query<'js>(ctx: &Ctx<'js>, v: &Value<'js>) -> Result<Nearby, ScriptError> {
    let mut q = Nearby {
        limit: 100,
        ..Nearby::default()
    };
    if v.is_undefined() || v.is_null() {
        return Ok(q);
    }
    let Json::Object(m) = plain_from_js(ctx, v)?.to_json() else {
        return Err(bad_data(
            "view.nearby takes an object such as {kinds: [\"crate\"]}.".into(),
        ));
    };
    let names = |x: &Json, what: &str| -> Result<Vec<String>, ScriptError> {
        x.as_array()
            .and_then(|a| a.iter().map(|s| s.as_str().map(str::to_owned)).collect())
            .ok_or_else(|| bad_data(format!("view.nearby's {what} is a list of names.")))
    };
    for (k, x) in &m {
        match k.as_str() {
            "kinds" => q.kinds = Some(names(x, "kinds")?),
            "visibility" => q.visibility = Some(names(x, "visibility")?),
            "within_m" => {
                q.within_m = Some(x.as_f64().filter(|d| *d >= 0.0).ok_or_else(|| {
                    bad_data("view.nearby's within_m is a distance in metres.".into())
                })?);
            }
            "limit" => {
                let n = x
                    .as_f64()
                    .filter(|n| n.fract() == 0.0 && (1.0..=100.0).contains(n))
                    .ok_or_else(|| bad_data("view.nearby's limit is from 1 to 100.".into()))?;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // checked
                {
                    q.limit = n as usize;
                }
            }
            other => {
                return Err(ScriptError::new(
                    "script.unknown_key",
                    format!("view.nearby has no option '{other}'."),
                    ErrorPhase::Run,
                )
                .suggest(amoris_contract::suggest::suggest_names(
                    other,
                    ["kinds", "within_m", "visibility", "limit"],
                )));
            }
        }
    }
    Ok(q)
}

/// `view.nearby(query)`: the known entities matching, nearest first, then by id.
pub fn view_nearby<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let q = nearby_query(ctx, &arg(ctx, args, 1))?;
    let list = with_view(ctx, args, "executor.view", |view| {
        let mut all: Vec<Known> = view
            .all_known()
            .into_iter()
            .filter(|k| q.kinds.as_ref().is_none_or(|ks| ks.contains(&k.kind)))
            .filter(|k| q.within_m.is_none_or(|d| k.range_m <= d))
            .filter(|k| {
                q.visibility
                    .as_ref()
                    .is_none_or(|vs| vs.iter().any(|v| v == k.visibility.name()))
            })
            .collect();
        all.sort_by(|a, b| a.range_m.total_cmp(&b.range_m).then(a.id.cmp(&b.id)));
        all.truncate(q.limit);
        Ok(all.iter().map(known_json).collect::<Vec<_>>())
    })?;
    let a = Array::new(ctx.clone())?;
    for (i, v) in list.iter().enumerate() {
        a.set(i, to_js(ctx, v)?)?;
    }
    Ok(freeze_new(ctx, a.into_object())?)
}

/// `view.eventsSince(seq)`: the events the seat perceived after `seq`, oldest first.
pub fn view_events<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let seq = arg(ctx, args, 1)
        .as_number()
        .filter(|x| x.fract() == 0.0 && *x >= 0.0 && *x < 9.0e15)
        .ok_or_else(|| bad_data("view.eventsSince takes a sequence number, 0 or more.".into()))?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // checked above
    let seq = seq as u64;
    let list = with_view(ctx, args, "executor.view", |view| {
        Ok(view
            .events_since(seq)
            .iter()
            .map(event_json)
            .collect::<Vec<_>>())
    })?;
    let a = Array::new(ctx.clone())?;
    for (i, v) in list.iter().enumerate() {
        a.set(i, to_js(ctx, v)?)?;
    }
    Ok(freeze_new(ctx, a.into_object())?)
}

fn control_name(v: &Value<'_>) -> Result<String, ScriptError> {
    match js::text(v) {
        Some(Ok(t)) => Ok(t),
        _ => Err(bad_data("A control is named by a string.".into())),
    }
}

/// The control `name` when the handed instance may write it: a declared control on one of its
/// channels, else `script.restricted` with the controls it may write.
fn writable<'a>(
    catalog: &'a ActionCatalog,
    h: &Handed,
    name: &str,
) -> Result<&'a amoris_interface::action::defs::ControlDef, ScriptError> {
    let mine = || {
        catalog
            .controls
            .iter()
            .filter(|c| h.channels.contains(&c.channel))
            .map(|c| c.name.as_str())
    };
    match catalog.control(name) {
        Some(c) if h.channels.contains(&c.channel) => Ok(c),
        found => {
            let why = match found {
                Some(c) => format!(
                    "{name} drives the channel {}, and {} occupies {}",
                    c.channel,
                    h.intent,
                    h.channels.join(", ")
                ),
                None => format!("the game declares no control {name}"),
            };
            Err(restricted_error(&format!("The control {name}"), &why)
                .suggest(amoris_contract::suggest::suggest_names(name, mine())))
        }
    }
}

/// `controls(intent).set(control, value)`.
pub fn control_set<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let name = control_name(&arg(ctx, args, 1))?;
    let v = arg(ctx, args, 2);
    let value = if let Some(b) = v.as_bool() {
        ControlValue::Bool(b)
    } else if let Some(x) = v.as_number() {
        ControlValue::Number(x)
    } else if let Some(Ok(t)) = js::text(&v) {
        ControlValue::Name(t)
    } else {
        return Err(bad_data(format!("{name} takes a number, a boolean or a name.")).into());
    };
    let id = arg(ctx, args, 0);
    with_call(ctx, "executor.controls", |call, world| {
        let h = handed(call, &id)?;
        let def = writable(catalog(world)?, h, &name)?;
        def.fits(&value)
            .map_err(|why| bad_data(format!("{name} takes {why}.")).value(value.to_json()))?;
        let cmd = IntentCommand::Set {
            id: h.id,
            actor: h.actor,
            control: name.clone(),
            value: value.clone(),
        };
        call.commands.push(Command::Intent(cmd));
        Ok(())
    })?;
    Ok(Value::new_undefined(ctx.clone()))
}

/// `controls(intent).pulse(control, target)`: one pulse, acting in the next tick.
pub fn control_pulse<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let name = control_name(&arg(ctx, args, 1))?;
    let t = arg(ctx, args, 2);
    let target = if t.is_undefined() || t.is_null() {
        None
    } else {
        Some(entity_arg(&t, "A pulse's target")?)
    };
    let id = arg(ctx, args, 0);
    with_call(ctx, "executor.controls", |call, world| {
        let h = handed(call, &id)?;
        let catalog = catalog(world)?;
        let def = writable(catalog, h, &name)?;
        let ControlKind::Pulse { target_kinds } = &def.kind else {
            return Err(bad_data(format!(
                "{name} is latched, not a pulse: write it with set."
            )));
        };
        if let Some(e) = target {
            let view = (catalog.view)(world, &h.seat);
            let known = view.known(e).ok_or_else(|| {
                bad_data(format!(
                    "The pulse's target #{} is not known to its seat.",
                    e.get()
                ))
            })?;
            match target_kinds {
                Some(ks) if !ks.contains(&known.kind) => {
                    return Err(bad_data(format!(
                        "{name} targets a {}, not a {}.",
                        ks.join(" or "),
                        known.kind
                    )));
                }
                None => {
                    return Err(bad_data(format!("{name} takes no target.")));
                }
                _ => {}
            }
        }
        let cmd = IntentCommand::Pulse {
            id: h.id,
            actor: h.actor,
            control: name.clone(),
            target,
        };
        call.commands.push(Command::Intent(cmd));
        Ok(())
    })?;
    Ok(Value::new_undefined(ctx.clone()))
}

/// Readings as the intent declares them, in declared order (`script.unknown_key` for another).
fn readings<'js>(ctx: &Ctx<'js>, v: &Value<'js>) -> Result<Option<Map<String, Json>>, ScriptError> {
    if v.is_undefined() || v.is_null() {
        return Ok(None);
    }
    match plain_from_js(ctx, v)?.to_json() {
        Json::Object(m) => Ok(Some(m)),
        _ => Err(bad_data(
            "Progress readings are an object such as {distance_m: 4}.".into(),
        )),
    }
}

fn ordered(def: &IntentDef, m: Map<String, Json>) -> Result<Vec<(String, PlainData)>, ScriptError> {
    for k in m.keys() {
        if !def.progress.iter().any(|p| p.name == *k) {
            return Err(ScriptError::new(
                "script.unknown_key",
                format!("{} declares no progress reading {k}.", def.name),
                ErrorPhase::Run,
            )
            .suggest(amoris_contract::suggest::suggest_names(
                k,
                def.progress.iter().map(|p| p.name.as_str()),
            )));
        }
    }
    Ok(def
        .progress
        .iter()
        .filter_map(|p| Some((p.name.clone(), PlainData::from_json(m.get(&p.name)?))))
        .collect())
}

/// Stages `cmd` for the handed instance `args[0]` names, after the readings `args[1]` when given.
fn stage<'js>(
    ctx: &Ctx<'js>,
    args: &[Value<'js>],
    what: &str,
    readings_at: Option<usize>,
    cmd: impl FnOnce(&Handed, &ActionCatalog) -> Result<Option<IntentCommand>, ScriptError>,
) -> NResult<'js> {
    let r = match readings_at {
        Some(i) => readings(ctx, &arg(ctx, args, i))?,
        None => None,
    };
    let id = arg(ctx, args, 0);
    with_call(ctx, what, |call, world| {
        let h = handed(call, &id)?;
        let catalog = catalog(world)?;
        let mut staged = Vec::new();
        if let Some(m) = r {
            let readings = ordered(declared(catalog, h)?, m)?;
            staged.push(IntentCommand::Progress { id: h.id, readings });
        }
        staged.extend(cmd(h, catalog)?);
        call.commands
            .extend(staged.into_iter().map(Command::Intent));
        Ok(())
    })?;
    Ok(Value::new_undefined(ctx.clone()))
}

/// `ctx.progress(intent, readings)`.
pub fn progress<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    stage(ctx, args, "executor.progress", Some(1), |_, _| Ok(None))
}

/// `ctx.setState(intent, state)`.
pub fn set_state<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let s = arg(ctx, args, 1);
    let state = if s.is_undefined() {
        PlainData::Null
    } else {
        plain_from_js(ctx, &s)?
    };
    stage(ctx, args, "executor.setState", None, |h, _| {
        Ok(Some(IntentCommand::State { id: h.id, state }))
    })
}

/// `ctx.reached(intent, readings)`.
pub fn reached<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    stage(ctx, args, "executor.reached", Some(1), |h, _| {
        Ok(Some(IntentCommand::Reached { id: h.id }))
    })
}

/// `ctx.succeed(intent, readings)`.
pub fn succeed<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    stage(ctx, args, "executor.succeed", Some(1), |h, _| {
        Ok(Some(IntentCommand::Succeed { id: h.id }))
    })
}

/// `ctx.reject(intent, code, detail)`: the code among the intent's failures with the `Fail` use.
pub fn reject<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let code = match js::text(&arg(ctx, args, 1)) {
        Some(Ok(t)) => t,
        _ => {
            return Err(
                bad_data("reject's code is a string such as \"errand.blocked\".".into()).into(),
            );
        }
    };
    let d = arg(ctx, args, 2);
    let detail = if d.is_undefined() || d.is_null() {
        Map::new()
    } else {
        match plain_from_js(ctx, &d)?.to_json() {
            Json::Object(m) => m,
            _ => {
                return Err(
                    bad_data("reject's detail is an object of the code's fields.".into()).into(),
                );
            }
        }
    };
    stage(ctx, args, "executor.reject", None, |h, catalog| {
        let def = declared(catalog, h)?;
        let undeclared = |why: String| {
            ScriptError::new(
                "script.undeclared_code",
                format!("{} cannot fail with {code}: {why}.", def.name),
                ErrorPhase::Run,
            )
            .with("code", json!(code))
            .suggest(amoris_contract::suggest::suggest_names(
                &code,
                def.failures.iter().map(String::as_str),
            ))
        };
        if !def.failures.contains(&code) {
            return Err(undeclared("it is not among the intent's failures".into()));
        }
        let fails = match catalog.code(&code) {
            Some(c) => {
                if let Some(k) = detail.keys().find(|k| !c.detail.contains(k)) {
                    return Err(bad_data(format!(
                        "{code} has no detail field {k}; its fields are {}.",
                        c.detail.join(", ")
                    )));
                }
                c.uses.contains(&amoris_contract::Use::Fail)
            }
            None => crate::intent_def::CONTRACT_FAIL_CODES.contains(&code.as_str()),
        };
        if !fails {
            return Err(undeclared(
                "the game does not declare it with the fail use".into(),
            ));
        }
        let stored = StoredProblem {
            code: code.clone(),
            detail: PlainData::from_json(&Json::Object(detail)),
        };
        Ok(Some(IntentCommand::Reject {
            id: h.id,
            problem: catalog.render(&stored),
        }))
    })
}

fn live(world: &World, id: u64) -> bool {
    world
        .resource::<IntentTable>()
        .by_id
        .get(&id)
        .is_some_and(|i| i.status.is_live())
}

/// Applies one executor command at the commit (script-host.md 5.5). A control write the actor's
/// body can no longer take fails the intent with `internal.error`, never the system; one for an
/// instance the commit has already ended is dropped.
pub(crate) fn apply(world: &mut World, catalog: Option<&ActionCatalog>, cmd: IntentCommand) {
    let Some(catalog) = catalog else {
        return;
    };
    match cmd {
        IntentCommand::Set {
            id,
            actor,
            control,
            value,
        } => {
            let Some(def) = catalog.control(&control).filter(|_| live(world, id)) else {
                return;
            };
            if !def.binding.writable(world, actor) {
                let report = format!("its body cannot take the control {control}");
                lifecycle::broke_contract(world, id, &report);
            } else if let Err(p) = write_control(world, def, actor, &value) {
                let report = format!("writing {control} on its body failed: {}", p.message);
                lifecycle::broke_contract(world, id, &report);
            }
        }
        IntentCommand::Pulse {
            id,
            actor,
            control,
            target,
        } => {
            let Some(def) = catalog.control(&control).filter(|_| live(world, id)) else {
                return;
            };
            if !def.binding.writable(world, actor) {
                let report = format!("its body cannot take the control {control}");
                lifecycle::broke_contract(world, id, &report);
            } else if let Err(p) = queue_pulse(world, def, actor, target) {
                let report = format!("pulsing {control} on its body failed: {}", p.message);
                lifecycle::broke_contract(world, id, &report);
            }
        }
        IntentCommand::Progress { id, readings } => {
            if live(world, id) {
                lifecycle::set_progress_with(world, catalog, id, readings);
            }
        }
        IntentCommand::State { id, state } => {
            if let Some(i) = world.resource_mut::<IntentTable>().by_id.get_mut(&id)
                && i.status.is_live()
            {
                i.state = state;
            }
        }
        IntentCommand::Reached { id } => lifecycle::reached(world, id),
        IntentCommand::Succeed { id } => lifecycle::end(world, id, End::Succeeded),
        IntentCommand::Reject { id, problem } => lifecycle::end(world, id, End::Failed(problem)),
    }
}

/// Applies the call's acts in call order. A refusal puts the intent table back and fails the
/// call; the caller's rollback takes the acts' events.
pub(crate) fn apply_acts(world: &mut World, acts: Vec<Command>) -> Result<(), ScriptError> {
    let before = world.get_resource::<IntentTable>().cloned();
    for a in acts {
        let Command::Act(req) = a else { continue };
        if let Err(p) = amoris_interface::action::act(world, &Caller::Developer, &req) {
            if let Some(t) = before {
                world.insert_resource(t);
            }
            return Err(act_refused(&p, ErrorPhase::WriteBack));
        }
    }
    Ok(())
}

/// An act's refusal as the script sees it: the act's own code and message, its problem in
/// `detail.act`.
pub fn act_refused(p: &Problem, phase: ErrorPhase) -> ScriptError {
    ScriptError::new(&p.code, p.message.clone(), phase)
        .with("act", serde_json::to_value(p).unwrap_or(Json::Null))
}

/// `ctx.intent(actor, kind, params, {target, tag})` (script-host.md 5.5): an act of one `start`
/// for the actor's seat, checked now and applied at the commit.
pub fn act<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let (a, k, p, o) = (
        arg(ctx, args, 0),
        arg(ctx, args, 1),
        arg(ctx, args, 2),
        arg(ctx, args, 3),
    );
    let actor = entity_arg(&a, "ctx.intent's actor")?;
    let kind = match js::text(&k) {
        Some(Ok(t)) => t,
        _ => return Err(bad_data("ctx.intent's kind is an intent's name.".into()).into()),
    };
    let params = if p.is_undefined() || p.is_null() {
        Json::Object(Map::new())
    } else {
        match plain_from_js(ctx, &p)?.to_json() {
            v @ Json::Object(_) => v,
            _ => return Err(bad_data("ctx.intent's params are an object.".into()).into()),
        }
    };
    let mut action = json!({"do": "start", "intent": kind, "params": params});
    if !(o.is_undefined() || o.is_null()) {
        let Json::Object(m) = plain_from_js(ctx, &o)?.to_json() else {
            return Err(
                bad_data("ctx.intent's options are an object such as {target: e}.".into()).into(),
            );
        };
        for (key, v) in m {
            match key.as_str() {
                "target" | "tag" => {
                    action[key.as_str()] = v;
                }
                other => {
                    return Err(ScriptError::new(
                        "script.unknown_key",
                        format!("ctx.intent has no option '{other}'."),
                        ErrorPhase::Run,
                    )
                    .suggest(amoris_contract::suggest::suggest_names(
                        other,
                        ["target", "tag"],
                    ))
                    .into());
                }
            }
        }
    }
    with_call(ctx, "ctx.intent", |call, world| {
        if !call.overlay.alive(world, actor) {
            return Err(ScriptError::new(
                "script.entity_missing",
                format!("ctx.intent's actor #{} does not exist.", actor.get()),
                ErrorPhase::Run,
            )
            .entity(actor.to_f64()));
        }
        let seat = world
            .get_resource::<ActionCatalog>()
            .and_then(|c| seats_with(world, c).ok())
            .and_then(|rows| rows.into_iter().find(|r| r.body == actor))
            .ok_or_else(|| {
                ScriptError::new(
                    "script.no_seat",
                    format!(
                        "#{} is no seat's body: ctx.intent acts for an entity whose Observer names \
                         a seat.",
                        actor.get()
                    ),
                    ErrorPhase::Run,
                )
                .entity(actor.to_f64())
            })?;
        let req = json!({"seat": seat.id, "actions": [action]});
        amoris_interface::action::check(world, &Caller::Developer, &req)
            .map_err(|p| act_refused(&p, ErrorPhase::Run))?;
        call.commands.push(Command::Act(req));
        Ok(())
    })?;
    Ok(Value::new_undefined(ctx.clone()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn executors_reach_only_their_own_natives_and_the_system_stream() {
        assert!(super::allowed_in_executor("executor.controls"));
        assert!(super::allowed_in_executor("ctx.rng"));
        assert!(super::allowed_in_executor("a random stream"));
        for what in [
            "world.get",
            "ctx.query",
            "ctx.emit",
            "ctx.intent",
            "ctx.rngFor",
        ] {
            assert!(!super::allowed_in_executor(what), "{what}");
        }
    }
}
