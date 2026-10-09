//! The intent lifecycle's transitions and their events (shared/contract/actions.md, The
//! lifecycle): active to holding, and to succeeded, failed, cancelled or superseded, each emitting
//! its `intent.*` event with the seat's body as subject. These functions are the one way an
//! instance's status changes, whether a boundary write, `interface.intents` or a script executor's
//! commit changes it (docs/spec/script-host.md 5.5), so every transition emits the same event.

use bevy_ecs::prelude::World;
use pocket_contract::Problem;
use pocket_sim::event::emit;
use pocket_sim::{EventKind, NewEvent, PlainData, SimClock};

use super::catalog::ActionCatalog;
use super::state::{IntentStatus, IntentTable, StoredProblem, internal};

/// How an intent ends.
#[derive(Clone, Debug, PartialEq)]
pub enum End {
    Succeeded,
    Failed(Problem),
    Cancelled,
    /// By the intent with this id, or `None` for a control set directly.
    Superseded(Option<u64>),
}

fn event_data(world: &World, id: u64, extra: Vec<(String, PlainData)>) -> Option<PlainData> {
    let i = world.resource::<IntentTable>().by_id.get(&id)?;
    let mut entries: Vec<(String, PlainData)> = vec![
        ("intent_id".into(), PlainData::Number(id_f64(id))),
        ("intent".into(), PlainData::String(i.intent.clone())),
    ];
    if let Some(tag) = &i.tag {
        entries.push(("tag".into(), PlainData::String(tag.clone())));
    }
    for (name, v) in &i.progress {
        if !entries.iter().any(|(n, _)| n == name) {
            entries.push((name.clone(), v.clone()));
        }
    }
    for (name, v) in extra {
        entries.retain(|(n, _)| *n != name);
        entries.push((name, v));
    }
    PlainData::object(entries).ok()
}

/// Intent ids are below 2^53 (the counter starts at 1 and counts one per start).
#[allow(clippy::cast_precision_loss)]
fn id_f64(id: u64) -> f64 {
    id as f64
}

fn emit_for(world: &mut World, id: u64, kind: &str, extra: Vec<(String, PlainData)>) {
    let Some(actor) = world
        .resource::<IntentTable>()
        .by_id
        .get(&id)
        .map(|i| i.actor)
    else {
        return;
    };
    let data = event_data(world, id, extra).unwrap_or(PlainData::Null);
    if let Ok(k) = EventKind::new(kind) {
        emit(world, NewEvent::new(k).subject(actor).data(data));
    }
}

/// `intent.started`, after an instance was added at a boundary.
pub fn started(world: &mut World, id: u64) {
    emit_for(world, id, "intent.started", Vec::new());
}

/// Replaces an instance's progress readings, each rounded at its declared precision (a reading the
/// intent does not declare is kept as given; executors report declared names only).
pub fn set_progress(world: &mut World, id: u64, progress: Vec<(String, PlainData)>) {
    match world.get_resource::<ActionCatalog>().cloned() {
        Some(c) => set_progress_with(world, &c, id, progress),
        None => set_progress_with(world, &ActionCatalog::none(), id, progress),
    }
}

/// [`set_progress`] with the catalog given (inside `interface.intents`, which holds it).
pub fn set_progress_with(
    world: &mut World,
    catalog: &ActionCatalog,
    id: u64,
    progress: Vec<(String, PlainData)>,
) {
    let mut table = world.resource_mut::<IntentTable>();
    if let Some(i) = table.by_id.get_mut(&id) {
        let defs = catalog
            .intent(&i.intent)
            .map(|e| e.def.progress.as_slice())
            .unwrap_or_default();
        i.progress = round_progress(defs, progress);
    }
}

/// `progress` rounded by `defs`.
pub fn round_progress(
    defs: &[super::defs::ProgressDef],
    progress: Vec<(String, PlainData)>,
) -> Vec<(String, PlainData)> {
    progress
        .into_iter()
        .map(|(n, v)| match defs.iter().find(|d| d.name == n) {
            Some(d) => (n, d.store(v)),
            None => (n, v),
        })
        .collect()
}

/// The goal is met: holding when the instance has `keep`, succeeded otherwise. A holding one stays
/// holding.
pub fn reached(world: &mut World, id: u64) {
    let (status, keep) = match world.resource::<IntentTable>().by_id.get(&id) {
        Some(i) => (
            i.status,
            matches!(i.params.get("keep"), Some(PlainData::Bool(true))),
        ),
        None => return,
    };
    match status {
        IntentStatus::Active if keep => {
            if let Some(i) = world.resource_mut::<IntentTable>().by_id.get_mut(&id) {
                i.status = IntentStatus::Holding;
            }
            emit_for(world, id, "intent.reached", Vec::new());
        }
        IntentStatus::Active => end(world, id, End::Succeeded),
        _ => {}
    }
}

/// Ends a live instance: sets its status, finishing tick and failure or superseder, emits its
/// event and prunes the seat's finished intents beyond the 32 kept. An instance already final is
/// left as it is.
pub fn end(world: &mut World, id: u64, how: End) {
    let tick = world.resource::<SimClock>().tick;
    let message = match &how {
        End::Failed(p) => Some((p.code.clone(), p.message.clone())),
        _ => None,
    };
    {
        let mut table = world.resource_mut::<IntentTable>();
        let Some(i) = table.by_id.get_mut(&id) else {
            return;
        };
        if !i.status.is_live() {
            return;
        }
        i.finished_tick = Some(tick);
        match &how {
            End::Succeeded => i.status = IntentStatus::Succeeded,
            End::Failed(p) => {
                i.status = IntentStatus::Failed;
                i.failure = Some(StoredProblem::of(p));
            }
            End::Cancelled => i.status = IntentStatus::Cancelled,
            End::Superseded(by) => {
                i.status = IntentStatus::Superseded;
                i.superseded_by = *by;
            }
        }
    }
    let (kind, extra) = match how {
        End::Succeeded => ("intent.succeeded", Vec::new()),
        End::Failed(_) => {
            let (code, msg) = message.unwrap_or_default();
            (
                "intent.failed",
                vec![
                    ("code".to_owned(), PlainData::String(code)),
                    ("message".to_owned(), PlainData::String(msg)),
                ],
            )
        }
        End::Cancelled => ("intent.cancelled", Vec::new()),
        End::Superseded(by) => (
            "intent.superseded",
            vec![(
                "by".to_owned(),
                by.map_or(PlainData::Null, |b| PlainData::Number(id_f64(b))),
            )],
        ),
    };
    emit_for(world, id, kind, extra);
    world.resource_mut::<IntentTable>().prune();
}

/// The problem an instance failed with, its message rendered from its code's template (the game's
/// codes in the catalog, the contract's own otherwise).
pub fn failure(world: &World, id: u64) -> Option<Problem> {
    let table = world.resource::<IntentTable>();
    let stored = table.by_id.get(&id)?.failure.as_ref()?;
    let catalog = world.get_resource::<ActionCatalog>();
    Some(match catalog {
        Some(c) => c.render(stored),
        None => super::state::shown(stored, String::new()),
    })
}

/// An executor broke its contract (it wrote a control outside its channels): the intent fails
/// with `internal.error`.
pub fn broke_contract(world: &mut World, id: u64, report: &str) {
    end(
        world,
        id,
        End::Failed(internal("interface.intents", report)),
    );
}
