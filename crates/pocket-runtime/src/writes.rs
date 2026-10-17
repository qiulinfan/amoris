//! The write trace (docs/spec/debugger.md 13): who changed `Entity.Component.field` over the ticks
//! of a `time.step {trace}` (the host's `debug.writes`), without stopping the game. For each tick
//! it names the boundary's recorded writes (`world.edit`, `player.act` with its controls and
//! intents, a `scripts.swap`) and every system that changed the field, engine or script, with its
//! phase and the values before and after; for a script system also the statements whose staged
//! writes changed it (the script host's recorder, [`pocket_script::debug::record_writes`]).
//!
//! The engine side reads the field in the step hooks' view of the world after each system
//! ([`StepHooks::world`]); the script side probes the call's staged value at every statement of an
//! instrumented program, which a trace asks for as the debugger does (a reload at the boundary,
//! debugger.md 2), and only for a field scripts can read. Both only read: the world hash of a
//! traced tick is the hash without the trace.

use std::ops::Range;
use std::sync::Arc;

use bevy_ecs::component::ComponentId;
use bevy_ecs::prelude::World;
use pocket_contract::{Pointer, Problem};
use pocket_persist::replay::{Applied, Source};
use pocket_script::ScriptAccess;
use pocket_script::debug::{FieldWatch, StatementWrite, WatchValue};
use pocket_sim::registry::{ComponentSchema, FieldType};
use pocket_sim::{ComponentRegistry, EntityId, StepHooks, StepReport, SystemKey, TickPhase};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::Game;
use crate::edit::{component_json, no_field};
use crate::inspect::field_at;
use crate::scene::unknown_component;
use crate::values::{EntityRef, resolve};

/// The most rows a trace answers; later ones are counted in `omitted`.
pub const MAX_TRACE_ROWS: usize = 2000;

/// The most writes of one boundary a row lists; the rest are counted.
const MAX_BOUNDARY_WRITES: usize = 16;

/// The most statements of one script system's call a row lists: the first seven and the last
/// (the rest are counted).
const MAX_STATEMENTS: usize = 8;

/// `time.step`'s write trace: the field whose writers each tick of the step names.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Trace {
    /// The entity: its id, a name or `Name#id`.
    pub entity: EntityRef,
    /// The component, such as `Boat`.
    pub component: String,
    /// A field or a path into it (`rudder`, `position.y`); the whole component when absent.
    #[serde(default)]
    pub field: Option<String>,
}

/// A change of the field, or a boundary's writes.
struct Row {
    tick: u64,
    /// A system key, or `boundary`.
    by: String,
    phase: Option<TickPhase>,
    /// The values before and after (`None` for a boundary that did not change the field).
    change: Option<(Value, Value)>,
    /// A boundary's recorded writes.
    writes: Vec<Value>,
    /// A script system's statements that changed the staged value, in order (at most
    /// [`MAX_STATEMENTS`]: the first ones and the last), and how many more did.
    statements: Vec<Value>,
    statements_more: u64,
}

/// The schema scripts see of `component` (its id too), when scripts can read it.
fn script_schema(world: &World, component: &str) -> Option<(ComponentId, Arc<ComponentSchema>)> {
    let entry = world.resource::<ComponentRegistry>().get(component)?;
    let access = world.get_resource::<ScriptAccess>()?;
    access.get(entry.id)?;
    let schema = access.schema(entry.id).or(entry.script.as_ref())?.clone();
    Some((entry.id, schema))
}

/// The top-level fields of `component`'s JSON (its JSON Schema's properties, or the fields scripts
/// see), or `None` when neither lists them.
fn field_names(world: &World, component: &str) -> Option<Vec<String>> {
    let entry = world.resource::<ComponentRegistry>().get(component)?;
    let schema = &*entry.json_schema;
    let props = schema.get("properties").or_else(|| {
        let name = schema.get("$ref")?.as_str()?.rsplit('/').next()?;
        schema
            .get("$defs")
            .or_else(|| schema.get("definitions"))?
            .get(name)?
            .get("properties")
    });
    match props.and_then(Value::as_object) {
        Some(p) => Some(p.keys().cloned().collect()),
        None => script_schema(world, component)
            .map(|(_, s)| s.fields.iter().map(|f| f.name.to_string()).collect()),
    }
}

/// Where the traced path lives among the slots of a component scripts can read: a read of those
/// slots alone (the scripts' accessor, no JSON) tells whether the path can have changed, so the
/// JSON read runs only when it may have (most systems never touch the component).
struct Slots {
    id: ComponentId,
    nums: Range<usize>,
    strs: Range<usize>,
}

/// The path's slots as last read, numbers by their bits; `None` where the entity lacks the
/// component.
type SlotValues = Option<(Vec<u64>, Vec<Arc<str>>)>;

impl Slots {
    /// The slots of `path` in `component`, when scripts can read it and the path is a field or one
    /// element of a vector field (not the whole component: an engine component's JSON may hold
    /// fields scripts do not see).
    fn resolve(world: &World, component: &str, path: &str) -> Option<Slots> {
        let (id, schema) = script_schema(world, component)?;
        let (name, rest) = path.split_once('.').unwrap_or((path, ""));
        let at = schema.fields.iter().position(|f| &*f.name == name)?;
        let f = &schema.fields[at];
        if f.ty == FieldType::Str {
            let i = pocket_script::access::str_index(&schema, at);
            return rest.is_empty().then_some(Slots {
                id,
                nums: 0..0,
                strs: i..i + 1,
            });
        }
        let first = usize::from(f.first_slot);
        let n = usize::from(f.ty.slots());
        let nums = match rest {
            "" => first..first + n,
            r => {
                let i = ["x", "y", "z", "w"].iter().position(|c| *c == r)?;
                if i >= n {
                    return None;
                }
                first + i..first + i + 1
            }
        };
        Some(Slots {
            id,
            nums,
            strs: 0..0,
        })
    }

    fn read(&self, world: &World, entity: EntityId) -> SlotValues {
        let e = pocket_sim::entity::entity(world, entity)?;
        let access = world.get_resource::<ScriptAccess>()?.get(self.id)?;
        let v = access.read(&world.entity(e))?;
        let nums = v.nums.get(self.nums.clone())?.iter().map(|x| x.to_bits());
        let strs = v.strs.get(self.strs.clone())?.to_vec();
        Some((nums.collect(), strs))
    }
}

/// A write trace over the ticks of one step ([`Trace`]): resolved when the step is asked for,
/// armed at its first tick, answered (`finish`) or dropped (`cancel`) at its end.
pub struct WriteTrace {
    id: EntityId,
    name: Option<String>,
    component: String,
    path: String,
    /// The tick the trace was asked at, and whether its first tick ran.
    asked_at: u64,
    armed: bool,
    /// The script host's recorder is armed for this trace: only while scripts can read the field
    /// (a probe of anything else fails at every statement and finds nothing).
    recording: bool,
    /// The field now, as far as the trace has read it (`null`: the entity lacks the component or
    /// is gone).
    value: Value,
    start: Value,
    first: Option<u64>,
    last: u64,
    traced: u64,
    changes: u64,
    rows: Vec<Row>,
    omitted: u64,
    /// The tick running.
    tick: u64,
    /// Rows of the tick running, before their statements are attached.
    pending: Vec<Row>,
    statements_dropped: u64,
    /// The path's slots (resolved again before each tick, a hot update may change a layout) and
    /// their values at the last read of `value`.
    slots: Option<Slots>,
    slot_values: Option<SlotValues>,
}

fn source_label(s: Source) -> String {
    match s {
        Source::Host => "host".into(),
        Source::Editor => "editor".into(),
        Source::Developer(n) => format!("developer:{n}"),
        Source::Player(n) => format!("player:{n}"),
    }
}

/// An integer-valued number as an integer, as the debugger shows numbers.
fn number_json(f: f64) -> Value {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 9_007_199_254_740_992.0 {
        // Exact: an integer below 2^53.
        #[allow(clippy::cast_possible_truncation)]
        let i = f as i64;
        json!(i)
    } else {
        json!(f)
    }
}

fn watch_json(v: &WatchValue) -> Value {
    match v {
        WatchValue::Number(f) => number_json(*f),
        WatchValue::Text(s) => json!(&**s),
    }
}

impl WriteTrace {
    /// The trace `t` asks for, checked against the world now: the entity, the component and the
    /// field (a wrong one refused with the component's fields and a suggestion: the whole path
    /// when the entity has the component, its first name otherwise). Refusals point into
    /// `time.step`'s `trace`.
    pub fn new(game: &Game, t: &Trace) -> Result<WriteTrace, Problem> {
        let world = game.world();
        let at = Pointer::root().key("trace");
        let id = resolve(world, &t.entity, &at.key("entity"))?;
        if world
            .resource::<pocket_sim::ComponentRegistry>()
            .get(&t.component)
            .is_none()
        {
            let mut p = unknown_component(world, &t.component);
            p.detail.insert("path".into(), json!("/trace/component"));
            return Err(p);
        }
        let path = t.field.clone().unwrap_or_default();
        let e = pocket_sim::entity::entity(world, id);
        let spec = format!("{}.{path}", t.component);
        let refused = match e.and_then(|e| component_json(world, e, &t.component)) {
            Some(v) => field_at(&v, &path)
                .is_none()
                .then(|| no_field(&spec, &t.component, &path, &v)),
            // The entity lacks the component now: its first name against the component's fields.
            None => field_names(world, &t.component).and_then(|names| {
                let first = path.split('.').next().unwrap_or_default();
                (!first.is_empty() && !names.iter().any(|n| n == first)).then(|| {
                    let fields: Map<String, Value> =
                        names.into_iter().map(|n| (n, Value::Null)).collect();
                    no_field(&spec, &t.component, &path, &Value::Object(fields))
                })
            }),
        };
        if let Some(mut p) = refused {
            p.detail.insert("path".into(), json!("/trace/field"));
            return Err(p);
        }
        let name = e.and_then(|e| {
            world
                .get::<pocket_sim::Name>(e)
                .map(|n| n.as_str().to_owned())
        });
        let mut trace = WriteTrace {
            id,
            name,
            component: t.component.clone(),
            path,
            asked_at: game.tick().0,
            armed: false,
            recording: false,
            value: Value::Null,
            start: Value::Null,
            first: None,
            last: game.tick().0,
            traced: 0,
            changes: 0,
            rows: Vec::new(),
            omitted: 0,
            tick: 0,
            pending: Vec::new(),
            statements_dropped: 0,
            slots: None,
            slot_values: None,
        };
        trace.value = trace.read(world);
        Ok(trace)
    }

    /// The field in `world` now.
    fn read(&self, world: &World) -> Value {
        pocket_sim::entity::entity(world, self.id)
            .and_then(|e| component_json(world, e, &self.component))
            .and_then(|v| field_at(&v, &self.path).cloned())
            .unwrap_or(Value::Null)
    }

    /// Whether the field may differ from `value` in `world`: its slots changed since they were
    /// last read (then they are kept), or it has no slots.
    fn may_have_changed(&mut self, world: &World) -> bool {
        let Some(slots) = &self.slots else {
            return true;
        };
        let now = slots.read(world, self.id);
        if self.slot_values.as_ref() == Some(&now) {
            return false;
        }
        self.slot_values = Some(now);
        true
    }

    /// `Entity.Component.field` as asked.
    fn label(&self) -> String {
        let entity = self
            .name
            .clone()
            .unwrap_or_else(|| self.id.get().to_string());
        if self.path.is_empty() {
            format!("{entity}.{}", self.component)
        } else {
            format!("{entity}.{}.{}", self.component, self.path)
        }
    }

    /// Whether scripts can read the traced field now (the component, and the field's first name
    /// among the fields they see).
    fn scripted(&self, world: &World) -> bool {
        let field = self.path.split('.').next().filter(|f| !f.is_empty());
        script_schema(world, &self.component).is_some_and(|(_, schema)| {
            field.is_none_or(|f| schema.fields.iter().any(|x| &*x.name == f))
        })
    }

    /// Arms the script host's recorder when scripts can read the field and disarms it when they
    /// no longer can (a hot update may change that between ticks).
    fn arm_recorder(&mut self, game: &mut Game) {
        let wanted = self.scripted(game.world());
        if wanted == self.recording {
            return;
        }
        let watch = wanted
            .then(|| {
                let field = self.path.split('.').next().filter(|f| !f.is_empty());
                FieldWatch::new(self.id.get(), &self.component, field)
            })
            .flatten();
        let armed = watch.is_some();
        self.recording = game.record_script_writes(watch) && armed;
    }

    /// Before a tick: arms the script host's recorder; the boundary's recorded writes, and the
    /// field's change across the boundary.
    fn begin_tick(&mut self, game: &mut Game) {
        let now = game.tick().0;
        self.arm_recorder(game);
        if !self.armed {
            self.armed = true;
            if now != self.asked_at {
                // Queued behind another step: the trace starts here.
                self.value = self.read(game.world());
            }
            self.start = self.value.clone();
        }
        self.tick = now + 1;
        self.slots = Slots::resolve(game.world(), &self.component, &self.path);
        self.slot_values = self.slots.as_ref().map(|s| s.read(game.world(), self.id));
        let value = self.read(game.world());
        let (applied, beyond) = game.boundary_writes();
        let mut writes: Vec<Value> = applied
            .iter()
            .take(MAX_BOUNDARY_WRITES)
            .map(|a: &Applied| {
                json!({"name": a.name, "source": source_label(a.source), "seq": a.seq,
                       "params": a.params})
            })
            .collect();
        let more = u64::try_from(applied.len().saturating_sub(writes.len()))
            .unwrap_or(u64::MAX)
            .saturating_add(beyond);
        if more > 0 {
            writes.push(json!({"more": more}));
        }
        let change = (value != self.value).then(|| (self.value.clone(), value.clone()));
        if change.is_some() || !writes.is_empty() {
            if change.is_some() {
                self.changes += 1;
            }
            self.pending.push(Row {
                tick: self.tick,
                by: "boundary".into(),
                phase: None,
                change,
                writes,
                statements: Vec::new(),
                statements_more: 0,
            });
        }
        self.value = value;
    }

    /// After a system: a change of the field is that system's.
    fn after_system(&mut self, key: &SystemKey, phase: TickPhase, world: &World) {
        if !self.may_have_changed(world) {
            return;
        }
        let value = self.read(world);
        if value == self.value {
            return;
        }
        self.changes += 1;
        let before = std::mem::replace(&mut self.value, value.clone());
        self.pending.push(Row {
            tick: self.tick,
            by: key.as_str().to_owned(),
            phase: Some(phase),
            change: Some((before, value)),
            writes: Vec::new(),
            statements: Vec::new(),
            statements_more: 0,
        });
    }

    /// After a tick: the script statements recorded in it go to their systems' rows.
    fn end_tick(&mut self, game: &mut Game) {
        let (statements, dropped) = game.take_script_writes();
        self.statements_dropped += dropped;
        for s in statements {
            let Some(v) = self.statement_json(&s) else {
                continue;
            };
            // No row: the call changed the staged value and then back, so nothing landed.
            if let Some(row) = self
                .pending
                .iter_mut()
                .find(|r| r.tick == s.tick && r.by == s.system)
            {
                row.statements.push(v);
            }
        }
        for row in &mut self.pending {
            // The first ones and the last, which wrote the value the row ends with.
            if let Some(more) = row.statements.len().checked_sub(MAX_STATEMENTS)
                && more > 0
            {
                row.statements
                    .drain(MAX_STATEMENTS - 1..MAX_STATEMENTS - 1 + more);
                row.statements_more = u64::try_from(more).unwrap_or(u64::MAX);
            }
        }
        self.traced += 1;
        self.first.get_or_insert(self.tick);
        self.last = self.tick;
        for row in self.pending.drain(..) {
            if self.rows.len() < MAX_TRACE_ROWS {
                self.rows.push(row);
            } else {
                self.omitted += 1;
            }
        }
    }

    /// A statement's place and the traced path's staged value after it, or `None` when the
    /// statement changed other slots of the field only.
    fn statement_json(&self, s: &StatementWrite) -> Option<Value> {
        let keep: Vec<usize> = s
            .names
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                self.path.is_empty()
                    || n.as_str() == self.path
                    || n.strip_prefix(self.path.as_str())
                        .is_some_and(|rest| rest.starts_with('.'))
                    || self
                        .path
                        .strip_prefix(n.as_str())
                        .is_some_and(|rest| rest.starts_with('.'))
            })
            .map(|(i, _)| i)
            .collect();
        let pick = |v: &Option<Vec<WatchValue>>| -> Value {
            let Some(v) = v else { return Value::Null };
            if keep.len() == 1 {
                return v.get(keep[0]).map_or(Value::Null, watch_json);
            }
            let mut m = Map::new();
            for &i in &keep {
                if let (Some(n), Some(x)) = (s.names.get(i), v.get(i)) {
                    m.insert(n.clone(), watch_json(x));
                }
            }
            Value::Object(m)
        };
        let (before, after) = (pick(&s.before), pick(&s.after));
        if before == after {
            return None;
        }
        let place = |(f, l, c): &(String, u32, u32)| json!(format!("{f}:{l}:{c}"));
        let mut v = json!({"at": s.at.as_ref().map(place), "value": after});
        if let Some(via) = &s.via {
            v["via"] = place(via);
        }
        if s.after_return {
            v["returned"] = json!(true);
        }
        Some(v)
    }

    /// Runs one tick of `game` under the trace, with `inner`'s hooks (a profiler) around its
    /// systems as without it.
    pub fn step(
        &mut self,
        game: &mut Game,
        inner: &mut dyn StepHooks,
    ) -> Result<StepReport, Problem> {
        self.begin_tick(game);
        let r = game.step_with(&mut TraceHooks { trace: self, inner });
        if r.is_ok() {
            self.end_tick(game);
        } else {
            self.pending.clear();
        }
        r
    }

    /// Stops the script host's recorder.
    pub fn cancel(self, game: &mut Game) {
        if self.recording {
            game.record_script_writes(None);
        }
    }

    /// Stops the recorder and answers the trace: `{entity, name, component, field, ticks: [first,
    /// last], traced, start, end, changes, rows, omitted?, statements_dropped?}`; a row is `{tick,
    /// by, phase?, before?, after?, writes?, at?, via?, statements?, statements_more?}`.
    pub fn finish(self, game: &mut Game) -> Value {
        let label = self.label();
        let end = self.value.clone();
        let Self {
            id,
            name,
            component,
            path,
            start,
            first,
            last,
            traced,
            changes,
            rows,
            omitted,
            statements_dropped,
            recording,
            ..
        } = self;
        if recording {
            game.record_script_writes(None);
        }
        let rows: Vec<Value> = rows
            .into_iter()
            .map(|r| {
                let mut v = json!({"tick": r.tick, "by": r.by});
                if let Some(p) = r.phase {
                    v["phase"] = json!(p.name());
                }
                if let Some((before, after)) = r.change {
                    v["before"] = before;
                    v["after"] = after;
                }
                if !r.writes.is_empty() {
                    v["writes"] = Value::Array(r.writes);
                }
                if let Some(last) = r.statements.last() {
                    v["at"] = last["at"].clone();
                    if let Some(via) = last.get("via") {
                        v["via"] = via.clone();
                    }
                    if r.statements.len() > 1 {
                        v["statements"] = Value::Array(r.statements);
                    }
                    if r.statements_more > 0 {
                        v["statements_more"] = json!(r.statements_more);
                    }
                }
                v
            })
            .collect();
        let mut out = json!({
            "trace": label,
            "entity": id.get(),
            "name": name,
            "component": component,
            "field": if path.is_empty() { Value::Null } else { json!(path) },
            "ticks": first.map(|f| json!([f, last])).unwrap_or(Value::Null),
            "traced": traced,
            "start": start,
            "end": end,
            "changes": changes,
            "rows": rows,
        });
        if omitted > 0 {
            out["omitted"] = json!(omitted);
        }
        if statements_dropped > 0 {
            out["statements_dropped"] = json!(statements_dropped);
        }
        out
    }
}

/// The step hooks of a traced tick: `inner`'s, and the field read after each system.
struct TraceHooks<'a> {
    trace: &'a mut WriteTrace,
    inner: &'a mut dyn StepHooks,
}

impl StepHooks for TraceHooks<'_> {
    fn phase(&mut self, phase: TickPhase, begin: bool) {
        self.inner.phase(phase, begin);
    }

    fn system(&mut self, key: &SystemKey, begin: bool) {
        self.inner.system(key, begin);
    }

    fn world(&mut self, key: &SystemKey, phase: TickPhase, begin: bool, world: &World) {
        self.inner.world(key, phase, begin, world);
        if !begin {
            self.trace.after_system(key, phase, world);
        }
    }
}
