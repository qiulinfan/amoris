//! Validating an `act` call whole before anything of it is applied (shared/contract/actions.md,
//! Validation and atomicity): its shape, then its names, then its semantics, stopping after the
//! first phase that finds a problem and reporting every problem that phase found. What comes out is
//! a plan: every reference resolved, every intent's parameters canonical and accepted.

use pocket_contract::codes::{self, Driven, Range};
use pocket_contract::{CheckOptions, Pointer, Problem, Shape};
use pocket_sim::PlainData;
use serde_json::{Map, Value, json};

use super::affordance;
use super::catalog::ActionCatalog;
use super::defs::{ControlKind, Effect, IntentDef};
use super::executor::{AcceptCx, Accepted, channels_of};
use super::request::{ActRequest, Action, Caller, Target};
use super::state::{ControlValue, IntentTable, ResolvedTarget, SeatRow};
use super::view::{ActorView, Known};

pub use super::refs::{check_params, resolve, seat_for};
use super::refs::{param_names, value_problem, wire_value};

/// One action, resolved.
#[derive(Clone, Debug)]
pub enum Planned {
    Set {
        controls: Vec<(String, ControlValue)>,
    },
    Pulse {
        control: String,
        target: Option<Known>,
    },
    Start(StartPlan),
    Use {
        entity: Known,
        verb: String,
        then: Box<Planned>,
    },
    Cancel {
        intent_id: u64,
    },
    EndTurn,
}

/// A start, resolved and accepted.
#[derive(Clone, Debug)]
pub struct StartPlan {
    pub intent: String,
    /// Canonical: defaults filled, aliases resolved.
    pub params: Map<String, Value>,
    pub target: Option<ResolvedTarget>,
    pub target_known: Option<Known>,
    pub tag: Option<String>,
    pub accepted: Accepted,
    pub channels: Vec<String>,
}

/// A validated call: the seat, its actions resolved in order, and the warnings to answer with.
#[derive(Clone, Debug)]
pub struct Validated {
    pub seat: SeatRow,
    pub steps: Vec<Planned>,
    pub warnings: Vec<Problem>,
}

/// A problem and the further ones riding in its `detail.also`, as one list.
pub fn flatten(p: Problem) -> Vec<Problem> {
    let also = p.also();
    let mut head = p;
    head.detail.remove("also");
    head.detail.remove("also_more");
    let mut out = vec![head];
    out.extend(also);
    out
}

fn phase(problems: Vec<Problem>) -> Result<(), Problem> {
    match Problem::combine(problems) {
        Some(p) => Err(p),
        None => Ok(()),
    }
}

const START_FIELDS: [&str; 4] = ["intent", "params", "target", "tag"];

/// Phase 1: the request against its schema, intent parameters against the intent's, tags.
fn shape(catalog: &ActionCatalog, raw: &Value) -> Result<(ActRequest, Vec<Problem>), Problem> {
    let mut problems = Vec::new();
    if let Some(obj) = raw.as_object()
        && obj.contains_key("do")
        && !obj.contains_key("actions")
    {
        return Err(codes::misplaced_field(
            &Pointer::root().key("do"),
            &Pointer::root().key("actions").index(0),
        ));
    }
    if let Some(actions) = raw.get("actions").and_then(Value::as_array) {
        for (i, a) in actions.iter().enumerate() {
            let (Some(obj), Some("start")) = (a.as_object(), a.get("do").and_then(Value::as_str))
            else {
                continue;
            };
            let Some(entry) = a
                .get("intent")
                .and_then(Value::as_str)
                .and_then(|n| catalog.intent(n))
            else {
                continue;
            };
            let takes = param_names(&entry.def);
            for k in obj.keys() {
                if k != "do" && !START_FIELDS.contains(&k.as_str()) && takes.contains(k) {
                    let at = Pointer::root().key("actions").index(i);
                    problems.push(codes::misplaced_field(&at.key(k), &at.key("params").key(k)));
                }
            }
        }
    }
    if !problems.is_empty() {
        phase(std::mem::take(&mut problems))?;
    }
    let opts = CheckOptions::new("the act request");
    let decoded = Shape::of::<ActRequest>().decode::<ActRequest>(raw, &opts)?;
    let mut warnings = decoded.warnings;
    let req = decoded.value;
    for (i, a) in req.actions.iter().enumerate() {
        let at = Pointer::root().key("actions").index(i);
        let (tag, params) = match a {
            Action::Start {
                intent,
                params,
                tag,
                ..
            } => {
                if let Some(entry) = catalog.intent(intent) {
                    match check_params(&entry.def, params, &at.key("params")) {
                        Ok((_, w)) => warnings.extend(w),
                        Err(p) => problems.extend(flatten(p)),
                    }
                }
                (tag, Some(params))
            }
            Action::Use { tag, .. } => (tag, None),
            _ => continue,
        };
        let _ = params;
        if let Some(t) = tag
            && t.len() > 64
        {
            let range = Range {
                max: Some(64.into()),
                ..Range::default()
            };
            problems.push(codes::out_of_range(
                &at.key("tag"),
                &json!(t.len()),
                &range,
                Some("A tag is at most 64 bytes."),
            ));
        }
    }
    phase(problems)?;
    Ok((req, warnings))
}

/// What phase 2 resolves an action into before its semantics are checked.
enum Named {
    Set(Vec<(String, ControlValue)>),
    Pulse(String, Option<Known>),
    Start {
        intent: String,
        params: Map<String, Value>,
        target: Option<(ResolvedTarget, Option<Known>)>,
        tag: Option<String>,
    },
    Use {
        entity: Known,
        verb: String,
        params: Map<String, Value>,
        tag: Option<String>,
    },
    Cancel(u64),
    EndTurn,
}

/// Validates `raw` for `caller`; on success the call can be applied whole.
pub fn validate(
    world: &bevy_ecs::prelude::World,
    catalog: &ActionCatalog,
    caller: &Caller,
    raw: &Value,
) -> Result<(ActRequest, Validated), Problem> {
    let (req, mut warnings) = shape(catalog, raw)?;
    let seat = seat_for(world, caller, req.seat.as_deref(), "act")?;
    let view = (catalog.view)(world, &seat);
    let named = names(world, catalog, &*view, &seat, &req)?;
    let steps = semantics(world, catalog, &*view, &seat, named, &mut warnings)?;
    Ok((
        req,
        Validated {
            seat,
            steps,
            warnings,
        },
    ))
}

/// Phase 2: seats, controls, intents, verbs, entity references, values and targets. A seat's
/// controls and intents are those its body can take (a sailing control needs a `Boat`).
fn names(
    world: &bevy_ecs::prelude::World,
    catalog: &ActionCatalog,
    view: &dyn ActorView,
    seat: &SeatRow,
    req: &ActRequest,
) -> Result<Vec<Named>, Problem> {
    let mut problems = Vec::new();
    let mut out = Vec::new();
    for (i, a) in req.actions.iter().enumerate() {
        let at = Pointer::root().key("actions").index(i);
        match a {
            Action::Set { controls } => {
                let mut set = Vec::new();
                for (name, v) in controls {
                    let path = at.key("controls").key(name);
                    match catalog.seat_control(world, seat, name) {
                        None => problems.push(codes::unknown_control(
                            &path,
                            &seat.id,
                            name,
                            &catalog.seat_controls(world, seat),
                        )),
                        Some(def) => {
                            let cv = wire_value(v);
                            if def.fits(&cv).is_ok() {
                                set.push((name.clone(), cv));
                            } else {
                                problems.push(value_problem(&def.kind, v, &path));
                            }
                        }
                    }
                }
                out.push(Named::Set(set));
            }
            Action::Pulse { control, target } => {
                let path = at.key("control");
                let Some(def) = catalog.seat_control(world, seat, control) else {
                    problems.push(codes::unknown_control(
                        &path,
                        &seat.id,
                        control,
                        &catalog.seat_controls(world, seat),
                    ));
                    continue;
                };
                let ControlKind::Pulse { target_kinds } = &def.kind else {
                    problems.push(codes::not_applicable(
                        &path,
                        "it is a latched control; set it with {\"do\": \"set\"}",
                    ));
                    continue;
                };
                let resolved = match (target, target_kinds) {
                    (None, Some(_)) => {
                        problems.push(codes::target_required(
                            &at.key("target"),
                            control,
                            "an entity",
                        ));
                        continue;
                    }
                    (Some(_), None) => {
                        problems.push(codes::target_not_taken(&at.key("target"), control));
                        continue;
                    }
                    (None, None) => None,
                    (Some(r), Some(kinds)) => match resolve(view, r, &at.key("target")) {
                        Err(p) => {
                            problems.push(p);
                            continue;
                        }
                        Ok(k) if !kinds.contains(&k.kind) => {
                            let ks: Vec<&str> = kinds.iter().map(String::as_str).collect();
                            problems.push(codes::wrong_target_kind(
                                &at.key("target"),
                                control,
                                &k.entity_name(),
                                &k.kind,
                                &ks,
                            ));
                            continue;
                        }
                        Ok(k) => Some(k),
                    },
                };
                out.push(Named::Pulse(control.clone(), resolved));
            }
            Action::Start {
                intent,
                params,
                target,
                tag,
            } => {
                let Some(entry) = catalog.seat_intent(world, seat, intent) else {
                    problems.push(codes::unknown_intent(
                        &at.key("intent"),
                        intent,
                        &catalog.seat_intents(world, seat),
                        None,
                    ));
                    continue;
                };
                match target_of(view, &entry.def, target.as_ref(), &at.key("target")) {
                    Ok(t) => out.push(Named::Start {
                        intent: intent.clone(),
                        params: params.clone(),
                        target: t,
                        tag: tag.clone(),
                    }),
                    Err(p) => problems.push(p),
                }
            }
            Action::Use {
                entity,
                verb,
                params,
                tag,
            } => {
                let k = match resolve(view, entity, &at.key("entity")) {
                    Ok(k) => k,
                    Err(p) => {
                        problems.push(p);
                        continue;
                    }
                };
                if !catalog
                    .affordances_of(&k.kind)
                    .iter()
                    .any(|a| a.verb == *verb)
                {
                    let c: Vec<pocket_contract::Candidate<'_>> = catalog
                        .affordances_of(&k.kind)
                        .iter()
                        .map(|a| pocket_contract::Candidate::new(&a.verb))
                        .collect();
                    problems.push(codes::unknown_verb(
                        &at.key("verb"),
                        &k.entity_name(),
                        verb,
                        &c,
                    ));
                    continue;
                }
                out.push(Named::Use {
                    entity: k,
                    verb: verb.clone(),
                    params: params.clone(),
                    tag: tag.clone(),
                });
            }
            Action::Cancel { intent_id } => match intent_id.id() {
                Some(id) => out.push(Named::Cancel(id)),
                None => problems.push(codes::wrong_type(
                    &at.key("intent_id"),
                    "an intent id: a number or \"#n\"",
                    "string",
                )),
            },
            Action::EndTurn => out.push(Named::EndTurn),
        }
    }
    phase(problems)?;
    Ok(out)
}

/// An intent's target checked against its `TargetSpec` and resolved.
fn target_of(
    view: &dyn ActorView,
    def: &IntentDef,
    target: Option<&Target>,
    path: &Pointer,
) -> Result<Option<(ResolvedTarget, Option<Known>)>, Problem> {
    let spec = &def.target;
    match target {
        None if matches!(spec, super::defs::TargetSpec::None) => Ok(None),
        None => Err(codes::target_required(path, &def.name, &spec.takes())),
        Some(_) if matches!(spec, super::defs::TargetSpec::None) => {
            Err(codes::target_not_taken(path, &def.name))
        }
        Some(Target::Point(p)) => {
            if spec.takes_point() {
                Ok(Some((
                    ResolvedTarget::Point([p.x, p.y.unwrap_or(0.0), p.z]),
                    None,
                )))
            } else {
                Err(codes::wrong_type(path, &spec.takes(), "object"))
            }
        }
        Some(Target::Entity(r)) => {
            if !spec.takes_entity() {
                return Err(codes::wrong_type(path, &spec.takes(), "entity"));
            }
            let k = resolve(view, r, path)?;
            if !spec.kinds().contains(&k.kind) {
                let ks: Vec<&str> = spec.kinds().iter().map(String::as_str).collect();
                return Err(codes::wrong_target_kind(
                    path,
                    &def.name,
                    &k.entity_name(),
                    &k.kind,
                    &ks,
                ));
            }
            Ok(Some((ResolvedTarget::Entity(k.id), Some(k))))
        }
    }
}

/// `action.unknown_control` or `action.unknown_intent` when the seat's body cannot carry out a
/// verb's effect (its control is not the body's, or its intent's controls are not).
fn effect_untaken(
    world: &bevy_ecs::prelude::World,
    catalog: &ActionCatalog,
    seat: &SeatRow,
    effect: &Effect,
    path: &Pointer,
) -> Option<Problem> {
    match effect {
        Effect::Pulse { control } => {
            catalog
                .seat_control(world, seat, control)
                .is_none()
                .then(|| {
                    codes::unknown_control(
                        path,
                        &seat.id,
                        control,
                        &catalog.seat_controls(world, seat),
                    )
                })
        }
        Effect::Intent { intent, .. } => catalog
            .seat_intent(world, seat, intent)
            .is_none()
            .then(|| codes::unknown_intent(path, intent, &catalog.seat_intents(world, seat), None)),
    }
}

/// The channels and controls each named action drives, for the conflict rule.
fn drives(catalog: &ActionCatalog, n: &Named) -> (Vec<String>, Vec<String>, bool) {
    match n {
        Named::Set(cs) => (
            cs.iter()
                .filter_map(|(c, _)| catalog.control(c).map(|d| d.channel.clone()))
                .collect(),
            cs.iter().map(|(c, _)| c.clone()).collect(),
            false,
        ),
        Named::Pulse(c, _) => (
            catalog
                .control(c)
                .map(|d| d.channel.clone())
                .into_iter()
                .collect(),
            Vec::new(),
            false,
        ),
        Named::Start { intent, params, .. } => {
            let p = PlainData::from_json(&Value::Object(params.clone()));
            (channels_of(catalog, intent, &p), Vec::new(), true)
        }
        _ => (Vec::new(), Vec::new(), false),
    }
}

/// Phase 3: conflicts, affordance requirements, cancels of unknown ids, and each intent's accept.
fn semantics(
    world: &bevy_ecs::prelude::World,
    catalog: &ActionCatalog,
    view: &dyn ActorView,
    seat: &SeatRow,
    named: Vec<Named>,
    warnings: &mut Vec<Problem>,
) -> Result<Vec<Planned>, Problem> {
    let mut problems = Vec::new();
    // Uses resolve to their effect first, so their channels count in the conflict rule.
    let mut resolved: Vec<(Named, Option<(Known, String)>)> = Vec::new();
    for (i, n) in named.into_iter().enumerate() {
        let at = Pointer::root().key("actions").index(i);
        match n {
            Named::Use {
                entity,
                verb,
                params,
                tag,
            } => {
                let Some(aff) = catalog
                    .affordances_of(&entity.kind)
                    .iter()
                    .find(|a| a.verb == verb)
                else {
                    continue;
                };
                if let Err(p) = affordance::check(view, seat, &entity, aff, &at.key("entity")) {
                    problems.push(p);
                }
                if let Some(p) = effect_untaken(world, catalog, seat, &aff.effect, &at.key("verb"))
                {
                    problems.push(p);
                    continue;
                }
                let inner = match &aff.effect {
                    Effect::Pulse { control } => {
                        Named::Pulse(control.clone(), Some(entity.clone()))
                    }
                    Effect::Intent {
                        intent,
                        params: fixed,
                    } => {
                        let mut merged = fixed.clone();
                        for (k, v) in params {
                            if fixed.contains_key(&k) {
                                problems.push(codes::action_conflict(
                                    &[at.key("params").key(&k)],
                                    Driven::Control(&k),
                                ));
                            }
                            merged.insert(k, v);
                        }
                        Named::Start {
                            intent: intent.clone(),
                            params: merged,
                            target: Some((ResolvedTarget::Entity(entity.id), Some(entity.clone()))),
                            tag,
                        }
                    }
                };
                resolved.push((inner, Some((entity, verb))));
            }
            Named::Pulse(control, Some(target)) => {
                if let Some(aff) = catalog.pulse_affordance(&target.kind, &control)
                    && let Err(p) = affordance::check(view, seat, &target, aff, &at.key("target"))
                {
                    problems.push(p);
                }
                resolved.push((Named::Pulse(control, Some(target)), None));
            }
            other => resolved.push((other, None)),
        }
    }
    // Conflicts: an intent and a control on one channel, two intents sharing one, a control twice.
    let driven: Vec<(Vec<String>, Vec<String>, bool)> =
        resolved.iter().map(|(n, _)| drives(catalog, n)).collect();
    for i in 0..driven.len() {
        for j in i + 1..driven.len() {
            let (ci, ki, si) = &driven[i];
            let (cj, kj, sj) = &driven[j];
            let paths = [
                Pointer::root().key("actions").index(i),
                Pointer::root().key("actions").index(j),
            ];
            if let Some(k) = ki.iter().find(|k| kj.contains(k)) {
                problems.push(codes::action_conflict(&paths, Driven::Control(k)));
            } else if (*si || *sj)
                && let Some(c) = ci.iter().find(|c| cj.contains(c))
            {
                problems.push(codes::action_conflict(&paths, Driven::Channel(c)));
            }
        }
    }
    let table = world.resource::<IntentTable>();
    let mut steps = Vec::new();
    for (i, (n, used)) in resolved.into_iter().enumerate() {
        let at = Pointer::root().key("actions").index(i);
        let plan = match n {
            Named::Set(c) => Planned::Set { controls: c },
            Named::Pulse(c, t) => Planned::Pulse {
                control: c,
                target: t,
            },
            Named::Cancel(id) => {
                if !table.by_id.get(&id).is_some_and(|x| x.seat == seat.id) {
                    problems.push(codes::unknown_intent_id(&at.key("intent_id"), id));
                }
                Planned::Cancel { intent_id: id }
            }
            Named::EndTurn => Planned::EndTurn,
            Named::Start {
                intent,
                params,
                target,
                tag,
            } => match start(
                catalog,
                view,
                &intent,
                params,
                target,
                tag,
                &at,
                used.is_some(),
            ) {
                Ok((plan, w)) => {
                    warnings.extend(w);
                    Planned::Start(plan)
                }
                Err(ps) => {
                    problems.extend(ps);
                    continue;
                }
            },
            Named::Use { .. } => continue,
        };
        steps.push(match used {
            Some((entity, verb)) => Planned::Use {
                entity,
                verb,
                then: Box::new(plan),
            },
            None => plan,
        });
    }
    phase(problems)?;
    Ok(steps)
}

#[allow(clippy::too_many_arguments)]
fn start(
    catalog: &ActionCatalog,
    view: &dyn ActorView,
    intent: &str,
    params: Map<String, Value>,
    target: Option<(ResolvedTarget, Option<Known>)>,
    tag: Option<String>,
    at: &Pointer,
    from_use: bool,
) -> Result<(StartPlan, Vec<Problem>), Vec<Problem>> {
    let Some(entry) = catalog.intent(intent) else {
        return Err(vec![codes::unknown_intent(
            &at.key("intent"),
            intent,
            &[],
            None,
        )]);
    };
    let (canonical, mut warnings) = if from_use {
        check_params(&entry.def, &params, &at.key("params")).map_err(flatten)?
    } else {
        // Checked in phase 1 already; fill the defaults again for the canonical form.
        check_params(&entry.def, &params, &at.key("params"))
            .map(|(c, _)| (c, Vec::new()))
            .map_err(flatten)?
    };
    let plain = PlainData::from_json(&Value::Object(canonical.clone()));
    let (resolved, known) = match target {
        Some((r, k)) => (Some(r), k),
        None => (None, None),
    };
    let accepted = match &entry.executor {
        Some(x) => {
            let cx = AcceptCx {
                params: &plain,
                target: resolved.as_ref(),
                view,
                catalog,
                dt: 0.0,
            };
            x.accept(&cx)?
        }
        None => Accepted::default(),
    };
    warnings.extend(accepted.warnings.iter().cloned());
    let channels = channels_of(catalog, intent, &plain);
    Ok((
        StartPlan {
            intent: intent.to_owned(),
            params: canonical,
            target: resolved,
            target_known: known,
            tag,
            accepted,
            channels,
        },
        warnings,
    ))
}
