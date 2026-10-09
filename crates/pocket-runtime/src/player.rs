//! The player layer in the game (charter 2, proof target 2; shared/contract/perception.md,
//! actions.md, time.md, mcp.md 5; docs/spec/player.md): what a game declares for its players, the
//! world state that carries it, the systems every game installs, and the `player.*` commands of
//! the catalog, through which an agent (over MCP or the CLI), a scripted policy or a test plays.
//!
//! - **Declaration.** A project opts in with `[player]` in `project.toml` (its perception file,
//!   the engine's action catalog, the default decision filter, the event that ends an episode).
//!   [`PlayerSpec`] holds it as a persisted resource: it is hashed, snapshotted, forked and
//!   restored with the world, and [`rebuild`] turns it back into the perception declarations, the
//!   action catalog and the time rules after a restore, so a replay rebuilds the player layer from
//!   its start snapshot alone (charter 5.1, Pioneer 2026-10-09).
//! - **Systems.** `interface.intents` (Control), `interface.perception` and `interface.turns`
//!   (Finish) are installed in every game; without a [`PlayerSpec`] they find nothing to do.
//! - **Callers.** A command's source is its caller: `Source::Player(i)` is a player at the
//!   declared seat of index `i`, restricted to its own perception; every other source is a
//!   developer, who names a seat or asks for the marked omniscient view.
//! - **Sessions.** The time controller (`pocket_interface::time::control::Controller`) is session
//!   state kept beside the world by [`crate::Game`]: each seat's decision points, push cursors,
//!   pacing and thinking clocks. It decides where a run stops, never what a tick computes.

use std::collections::BTreeMap;

use bevy_ecs::prelude::{Resource, World};
use pocket_contract::{CheckOptions, Pointer, Problem, codes, detail};
use pocket_interface::action::{self, ActionCatalog, Caller, Controls, IntentTable};
use pocket_interface::perception::{
    self, DescribeRequest, EventsRequest, NearbyRequest, ObserveRequest, PerceptionDefs,
    VisibilityScale,
};
use pocket_interface::projection::Projection;
use pocket_interface::time::control::Controller;
use pocket_interface::time::play::{DecisionFilter, PlayPacing};
use pocket_interface::time::turns::{self, Episode, TimeRules, TurnStructure};
use pocket_link::Source;
use pocket_sim::persisted::{Persisted, RegisterPersisted};
use pocket_sim::{
    ComponentRegistry, EntityId, EventOutbox, PlainData, RunCondition, Sim, SimClock, TickPhase,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// The contract version the player tools speak (shared/contract/README.md, Versioning).
pub const CONTRACT: &str = "0.2";

/// The engine's action catalogs a project can name in `[player] actions`.
pub const CATALOGS: &[&str] = &["sailing", "none"];

/// `[player]` in `project.toml`: what the game declares for its players.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlayerManifest {
    /// The perception declarations (observer profiles, instruments, kinds, events), a JSON file
    /// in the project (default `perception.json`).
    #[serde(default = "default_perception")]
    pub perception: String,
    /// The engine's action catalog: `sailing` (the sailing controls, intents with Rust executors,
    /// the crate's affordance and the `sail.*` codes) or `none`.
    #[serde(default = "default_actions")]
    pub actions: String,
    /// The default decision filter of every seat (time.md, Decision points).
    #[serde(default)]
    pub decisions: Option<DecisionFilter>,
    /// The episode ends in the tick an event of this kind is emitted; its data are the result
    /// (time.md, Episodes). None: the episode never ends by itself.
    #[serde(default)]
    pub done_event: Option<String>,
}

fn default_perception() -> String {
    "perception.json".to_owned()
}

fn default_actions() -> String {
    "sailing".to_owned()
}

/// The player declarations as world state: the perception declarations' JSON text, the engine
/// action catalog's name, the seats' default decision filter and the event that ends an episode.
/// A persisted resource, so every snapshot and replay carries them (charter 5.1, Pioneer).
#[derive(Resource, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlayerSpec {
    pub perception: String,
    pub actions: String,
    pub decisions: DecisionFilter,
    pub done_event: Option<String>,
}

impl Persisted for PlayerSpec {
    const NAME: &'static str = "PlayerSpec";
    const VERSION: u32 = 1;

    fn trace(
        t: &mut serde_reflection::Tracer,
        s: &serde_reflection::Samples,
    ) -> serde_reflection::Result<()> {
        t.trace_type::<DecisionFilter>(s)?;
        t.trace_type::<Self>(s).map(|_| ())
    }
}

/// The sailing game's default decision filter (shared/contract/sailing.md, Time): an intent that
/// finished or started holding, a sighting, running aground, a mark rounded, the course finished,
/// a crate aboard; ten seconds with nothing driving the seat; a heartbeat every two minutes.
pub fn sailing_filter() -> DecisionFilter {
    DecisionFilter {
        events: [
            "intent.succeeded",
            "intent.failed",
            "intent.reached",
            "sighted",
            "boat.aground",
            "mark.rounded",
            "course.finished",
            "crate.taken",
            "interact.ignored",
        ]
        .map(String::from)
        .to_vec(),
        idle_s: Some(10.0),
        every_s: Some(120.0),
    }
}

impl PlayerSpec {
    /// The spec of a project's `[player]`, with its perception file's text; checked by building
    /// what it declares (`definition.invalid` at the first fault).
    pub fn new(m: &PlayerManifest, perception: String) -> Result<PlayerSpec, Problem> {
        let spec = PlayerSpec {
            perception,
            actions: m.actions.clone(),
            decisions: m.decisions.clone().unwrap_or_else(sailing_filter),
            done_event: m.done_event.clone(),
        };
        spec.parts()?;
        Ok(spec)
    }

    /// What the spec declares: the perception declarations and the action catalog.
    fn parts(&self) -> Result<(PerceptionDefs, ActionCatalog), Problem> {
        let defs = perception::load(&self.perception)?;
        let catalog = match self.actions.as_str() {
            "sailing" => action::sailing::game_catalog(&defs)?,
            "none" => ActionCatalog::none(),
            other => {
                let valid: Vec<pocket_contract::Candidate<'_>> = CATALOGS
                    .iter()
                    .map(|c| pocket_contract::Candidate::new(c))
                    .collect();
                return Err(codes::invalid_value(
                    &Pointer::root().key("player").key("actions"),
                    other,
                    &valid,
                ));
            }
        };
        Ok((defs, catalog))
    }
}

/// Declares the player layer's persistence classes (persistence.md 2): perception's, the
/// actions', the time state's and the spec, and the rebuild that installs what the spec declares
/// after a restore.
pub fn declare<R: RegisterPersisted>(r: &mut R) {
    perception::declare(r);
    action::declare(r);
    turns::declare(r);
    r.resource::<PlayerSpec>()
        .rebuild("player.declarations", rebuild);
}

/// Installs the player layer into a new world: its components in the component registry, the
/// three interface systems at their rows (simulation.md 4.4), and, with a spec, the spec and the
/// world state it starts with.
pub fn install(sim: &mut Sim, spec: Option<&PlayerSpec>) -> Result<(), Problem> {
    let w = sim.world_mut();
    perception::register(w)?;
    let known = w
        .get_resource::<ComponentRegistry>()
        .is_some_and(|r| r.get(<Controls as Persisted>::NAME).is_some());
    if !known {
        ComponentRegistry::register::<Controls>(w, None)?;
    }
    if let Some(spec) = spec {
        w.insert_resource(spec.clone());
        w.insert_resource(IntentTable::default());
        w.insert_resource(VisibilityScale::default());
        w.insert_resource(Episode::default());
        derive(w)?;
    }
    sim.add_exclusive(
        "interface.intents",
        TickPhase::Control,
        RunCondition::Always,
        |world, ctx| {
            if let Err(p) = action::executor::run_intents(world) {
                ctx.fault(p);
            }
        },
    )?;
    sim.add_exclusive(
        perception::SYSTEM,
        TickPhase::Finish,
        RunCondition::Always,
        perception::update::run,
    )?;
    sim.add_exclusive(
        "interface.turns",
        TickPhase::Finish,
        RunCondition::Always,
        |world, _ctx| turns::turns_system(world),
    )
}

/// Installs what the world's spec declares: the perception declarations, the action catalog with
/// its field writer, and the time rules. Game data, rebuilt from the spec rather than persisted.
fn derive(world: &mut World) -> Result<(), Problem> {
    let Some(spec) = world.get_resource::<PlayerSpec>().cloned() else {
        world.remove_resource::<PerceptionDefs>();
        world.remove_resource::<ActionCatalog>();
        world.remove_resource::<TimeRules>();
        return Ok(());
    };
    let (defs, catalog) = spec.parts()?;
    world.insert_resource(defs);
    world.insert_resource(catalog);
    world.insert_resource(action::FieldWriter(write_field));
    world.insert_resource(TimeRules {
        structure: TurnStructure::Continuous,
        done: spec
            .done_event
            .as_ref()
            .map(|_| done_by_event as turns::DoneFn),
        quiet: None,
        result: Vec::new(),
        doc: match &spec.done_event {
            Some(k) => format!("The episode ends in the tick a {k} event is emitted."),
            None => "The episode does not end by itself.".to_owned(),
        },
    });
    Ok(())
}

/// After a restore: the declarations of the restored world's spec, or none.
fn rebuild(world: &mut World) {
    // The spec was checked when the world was made; a refusal here leaves the layer inert.
    if derive(world).is_err() {
        world.remove_resource::<PerceptionDefs>();
        world.remove_resource::<ActionCatalog>();
    }
}

/// The game's `done` predicate for `done_event`: the data of the first event of that kind the
/// tick emitted, a pure function of the world at the end of the tick (its events are in the
/// outbox until `sim.finish`).
fn done_by_event(world: &World) -> Option<Vec<(String, PlainData)>> {
    let kind = world.get_resource::<PlayerSpec>()?.done_event.clone()?;
    let outbox = world.get_resource::<EventOutbox>()?;
    let ev = outbox.events().iter().find(|e| e.kind.as_str() == kind)?;
    let mut result = vec![("event".to_owned(), PlainData::String(kind))];
    if let PlainData::Object(entries) = &ev.data {
        result.extend(entries.iter().cloned());
    }
    Some(result)
}

/// The action layer's field writer: a pulse bound to a project component's field (the sailing
/// `interact` to `Crew.take`) writes its target's id into that field of the body at the start of
/// the tick it acts in; the game's rule reads and clears it. A body without the component gets it,
/// its other fields at their defaults; a game that declares no such component has no rule to read
/// the pulse, which then acts on nothing.
fn write_field(
    world: &mut World,
    body: EntityId,
    component: &str,
    field: &str,
    value: &Value,
) -> Result<(), Problem> {
    let Some((schema, access)) = crate::scene::project_component(world, component) else {
        return Ok(());
    };
    let e = pocket_sim::entity::entity(world, body)
        .ok_or_else(|| pocket_sim::entity::entity_not_found(body))?;
    let base = access
        .read(&world.entity(e))
        .unwrap_or_else(|| pocket_script::access::defaults(&schema));
    let entity_slot = schema
        .fields
        .iter()
        .find(|f| &*f.name == field && f.ty == pocket_sim::registry::FieldType::Entity)
        .map(|f| usize::from(f.first_slot));
    let v = match (entity_slot, value) {
        (Some(slot), Value::Null | Value::Number(_)) => {
            let mut v = base;
            let mut nums = v.nums.to_vec();
            nums[slot] = value.as_f64().unwrap_or(0.0);
            v.nums = nums.into_boxed_slice();
            v
        }
        _ => {
            let patch = Map::from_iter([(field.to_owned(), value.clone())]);
            let at = Pointer::root().key(component);
            crate::values::patch(world, &schema, &base, &patch, &at)?
        }
    };
    let mut ent = world.entity_mut(e);
    if !access.write(&mut ent, &v) {
        access.insert(&mut ent, v);
    }
    Ok(())
}

/// Whether the world declares a player layer.
pub fn declared(world: &World) -> bool {
    world.contains_resource::<PlayerSpec>()
}

/// Whether a player may send `command` (a catalog name with aliases resolved): the player tools
/// alone (`player.pacing` among them refuses a player by itself).
pub fn player_command(command: &str) -> bool {
    command.starts_with("player.")
}

/// Whether `source` may send `command` (a catalog name with aliases resolved, or a game's own) to
/// this world. In a game that declares players, `Source::Player(i)` is the player at seat `i` and
/// sends the player tools alone: every other command (reading the world as it is, editing it,
/// running time, the kept snapshots, a game's own commands) is a developer's and is refused with
/// `permission.denied`, so a player never gets the world beyond its seat's perception (charter
/// 3, principle 2; docs/spec/player.md 7). A game without players keeps slice 1's player source:
/// an input source ordered after developers (threads.md 5.2), as the web form's human player and
/// the `sailing` sample's checks use it; it has no seat and no perception to keep it to.
/// A replay is not asked: it applies what was recorded.
pub fn permitted(world: &World, command: &str, source: Source) -> Result<(), Problem> {
    if !matches!(source, Source::Player(_)) || player_command(command) || !declared(world) {
        return Ok(());
    }
    Err(codes::permission_denied(command, "player", "developer"))
}

/// The caller a command's source stands for: a player by its seat's index; the editor, the host
/// and developers as a developer.
pub fn caller(world: &World, source: Source) -> Result<Caller, Problem> {
    match source {
        Source::Player(i) => match action::state::seat_by_index(world, i)? {
            Some(row) => Ok(Caller::Player { seat: row.id }),
            None => Err(codes::seat_unknown(
                &Pointer::root().key("seat"),
                &format!("#{i}"),
                &[],
            )),
        },
        _ => Ok(Caller::Developer),
    }
}

/// Perception's form of a caller.
pub fn perception_caller(caller: &Caller) -> perception::Caller<'_> {
    use perception::Role;
    match caller {
        Caller::Player { seat } => perception::Caller {
            role: Role::Player,
            seat: Some(seat),
        },
        Caller::Developer => perception::Caller {
            role: Role::Developer,
            seat: None,
        },
        Caller::Checker => perception::Caller {
            role: Role::Checker,
            seat: None,
        },
    }
}

/// `player.not_declared {command}`: a player command in a game that declares no player layer.
pub fn no_layer(command: &str) -> Problem {
    Problem::new(
        "player.not_declared",
        format!(
            "{command} needs a game with players; this project declares none ([player] in \
             project.toml with a perception file)."
        ),
        detail([("command", json!(command))]),
    )
}

/// The seat a read follows: the player's own, or the one a developer names (or the only one).
fn seat_named(
    world: &World,
    caller: &Caller,
    named: Option<&str>,
    command: &str,
) -> Result<Option<String>, Problem> {
    pocket_interface::time::session::seat_of(world, caller, named, command)
}

/// A projected answer as the catalog answers it: JSON as a value, text and tensors as a string.
fn answer_value(a: &perception::Answer) -> Value {
    match a.projection {
        Projection::Json => serde_json::from_str(&a.body).unwrap_or(Value::String(a.body.clone())),
        _ => Value::String(a.body.clone()),
    }
}

/// `player.observe` (perception.md, Queries: `observe`): the seat's instruments, active intents,
/// pending decision, ranked percepts and perceived events within the token budget.
pub fn observe(
    world: &World,
    ctl: &mut Controller,
    source: Source,
    raw: &Value,
) -> Result<Value, Problem> {
    let who = caller(world, source)?;
    let req: ObserveRequest =
        pocket_contract::decode(raw, &CheckOptions::new("the observe request"))?.value;
    ctl.attach(world);
    let seat = if req.omniscient == Some(true) {
        req.seat.clone()
    } else {
        seat_named(world, &who, req.seat.as_deref(), "observe")?
    };
    pocket_interface::time::session::observation(world, ctl, &who, seat.as_deref(), &req)
}

/// `player.nearby` (perception.md, Queries).
pub fn nearby(world: &World, source: Source, raw: &Value) -> Result<Value, Problem> {
    let who = caller(world, source)?;
    let mut req: NearbyRequest =
        pocket_contract::decode(raw, &CheckOptions::new("the nearby request"))?.value;
    if req.omniscient != Some(true) && req.seat.is_none() && !matches!(who, Caller::Player { .. }) {
        req.seat = seat_named(world, &who, None, "nearby")?;
    }
    let aff = action::observe::CatalogAffordances::new(world);
    // The targets of the seat's live intents rank first, as in an observation.
    let targets = match req.seat.as_deref().or(match &who {
        Caller::Player { seat } => Some(seat.as_str()),
        _ => None,
    }) {
        Some(s) => action::observe::seat_parts(world, s, None).targets,
        None => Vec::new(),
    };
    perception::nearby(world, &perception_caller(&who), &req, &targets, &aff)
        .map(|a| answer_value(&a))
}

/// `player.events` (perception.md, Queries).
pub fn events(world: &World, source: Source, raw: &Value) -> Result<Value, Problem> {
    let who = caller(world, source)?;
    let mut req: EventsRequest =
        pocket_contract::decode(raw, &CheckOptions::new("the events request"))?.value;
    if req.omniscient != Some(true) && req.seat.is_none() && !matches!(who, Caller::Player { .. }) {
        req.seat = seat_named(world, &who, None, "events")?;
    }
    perception::events(world, &perception_caller(&who), &req).map(|a| answer_value(&a))
}

/// `player.affordances` (actions.md, Affordances).
pub fn affordances(world: &World, source: Source, raw: &Value) -> Result<Value, Problem> {
    let who = caller(world, source)?;
    action::affordances(world, &who, raw)
}

/// `player.intents` (actions.md, Intent instances).
pub fn intents(world: &World, source: Source, raw: &Value) -> Result<Value, Problem> {
    let who = caller(world, source)?;
    action::intents(world, &who, raw)
}

/// One part of the game definition `describe` can answer alone (mcp.md 5.2, `DescribePart`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DescribePart {
    Seats,
    Observers,
    Instruments,
    Controls,
    Intents,
    Kinds,
    Events,
    Codes,
    Structure,
    Pacing,
    Decisions,
    Episode,
}

impl DescribePart {
    const ALL: [DescribePart; 12] = [
        DescribePart::Seats,
        DescribePart::Observers,
        DescribePart::Instruments,
        DescribePart::Controls,
        DescribePart::Intents,
        DescribePart::Kinds,
        DescribePart::Events,
        DescribePart::Codes,
        DescribePart::Structure,
        DescribePart::Pacing,
        DescribePart::Decisions,
        DescribePart::Episode,
    ];

    fn name(self) -> &'static str {
        match self {
            DescribePart::Seats => "seats",
            DescribePart::Observers => "observers",
            DescribePart::Instruments => "instruments",
            DescribePart::Controls => "controls",
            DescribePart::Intents => "intents",
            DescribePart::Kinds => "kinds",
            DescribePart::Events => "events",
            DescribePart::Codes => "codes",
            DescribePart::Structure => "structure",
            DescribePart::Pacing => "pacing",
            DescribePart::Decisions => "decisions",
            DescribePart::Episode => "episode",
        }
    }
}

/// `player.describe` (mcp.md 5.2, `DescribeParams`): one entity as the seat knows it, or the game
/// definition filtered for the caller, whole or one part (and one item of it by `name`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DescribeParams {
    #[serde(default)]
    pub seat: Option<String>,
    /// One entity: perception's `describe` (its facts, affordances and latest events).
    #[serde(default)]
    pub entity: Option<perception::EntityRef>,
    /// One part of the definition; neither `entity` nor `part`: the whole definition.
    #[serde(default)]
    pub part: Option<DescribePart>,
    /// With `part`: one item of it (`sail_to`).
    #[serde(default)]
    pub name: Option<String>,
    /// Default 2000 for the definition; the profile's for an entity.
    #[serde(default)]
    pub budget_tokens: Option<u32>,
    #[serde(default)]
    pub projection: Option<Projection>,
    #[serde(default)]
    pub omniscient: Option<bool>,
}

/// The definition's default budget (mcp.md 8.3).
pub const DESCRIBE_BUDGET: u32 = 2000;

/// `player.describe`.
pub fn describe(
    world: &World,
    ctl: &Controller,
    source: Source,
    raw: &Value,
) -> Result<Value, Problem> {
    let who = caller(world, source)?;
    let p: DescribeParams =
        pocket_contract::decode(raw, &CheckOptions::new("the describe request"))?.value;
    if p.entity.is_some() && p.part.is_some() {
        return Err(Problem::new(
            "request.conflict",
            "describe takes an entity or a part of the definition, not both.",
            detail([("fields", json!(["entity", "part"]))]),
        ));
    }
    if p.name.is_some() && p.part.is_none() {
        return Err(Problem::new(
            "request.not_applicable",
            "name selects one item of a part; give part too, as {\"part\": \"intents\", \"name\": \"sail_to\"}.",
            detail([("field", json!("name")), ("needs", json!("part"))]),
        ));
    }
    if let Some(entity) = p.entity {
        let mut req = DescribeRequest {
            seat: p.seat,
            entity,
            budget_tokens: p.budget_tokens,
            projection: p.projection,
            omniscient: p.omniscient,
        };
        if req.omniscient != Some(true)
            && req.seat.is_none()
            && !matches!(who, Caller::Player { .. })
        {
            req.seat = seat_named(world, &who, None, "describe")?;
        }
        let aff = action::observe::CatalogAffordances::new(world);
        return perception::describe(world, &perception_caller(&who), &req, &aff)
            .map(|a| answer_value(&a));
    }
    if matches!(who, Caller::Player { .. }) && p.omniscient == Some(true) {
        return Err(codes::omniscient_forbidden("player"));
    }
    let seat = seat_named(world, &who, p.seat.as_deref(), "describe")?;
    let budget = perception::query::check_budget(p.budget_tokens, DESCRIBE_BUDGET)?;
    definition(
        world,
        ctl,
        &who,
        seat.as_deref(),
        p.part,
        p.name.as_deref(),
        budget,
    )
}

/// The game definition (README, The game definition) as the caller may see it: a player its own
/// seat, its profile, the instruments it reads, the kinds, events, controls and intents it can
/// know and use; a developer everything. Beyond `budget` tokens the parts list names only, with
/// the call that gives each in full.
fn definition(
    world: &World,
    ctl: &Controller,
    who: &Caller,
    seat: Option<&str>,
    part: Option<DescribePart>,
    name: Option<&str>,
    budget: u32,
) -> Result<Value, Problem> {
    let (Some(defs), Some(catalog)) = (
        world.get_resource::<PerceptionDefs>(),
        world.get_resource::<ActionCatalog>(),
    ) else {
        return Err(no_layer("describe"));
    };
    let player = matches!(who, Caller::Player { .. });
    let rows = action::state::seats(world)?;
    let row = seat.and_then(|s| rows.iter().find(|r| r.id == s));
    let profile_name = row.and_then(|r| {
        let e = pocket_sim::entity::entity(world, r.body)?;
        world
            .get::<perception::Observer>(e)
            .map(|o| o.profile.clone())
    });
    let profile = profile_name.as_deref().and_then(|n| defs.profile(n));
    let mut parts: BTreeMap<&'static str, Vec<(String, Value)>> = BTreeMap::new();
    let seats: Vec<(String, Value)> = rows
        .iter()
        .filter(|r| !player || Some(r.id.as_str()) == seat)
        .map(|r| {
            let body = pocket_sim::entity::entity(world, r.body);
            let observer = body
                .and_then(|e| world.get::<perception::Observer>(e))
                .map(|o| o.profile.clone());
            let body_name = body
                .and_then(|e| world.get::<pocket_sim::Name>(e))
                .map(|n| n.as_str().to_owned());
            let controls: Vec<&str> = catalog
                .controls
                .iter()
                .filter(|c| catalog.seat_control(world, r, &c.name).is_some())
                .map(|c| c.name.as_str())
                .collect();
            let intents: Vec<&str> = catalog
                .intents
                .iter()
                .filter(|i| catalog.seat_intent(world, r, &i.def.name).is_some())
                .map(|i| i.def.name.as_str())
                .collect();
            (
                r.id.clone(),
                json!({"id": r.id, "observer": observer, "body": body_name, "controls": controls,
                       "intents": intents, "takers": "any"}),
            )
        })
        .collect();
    parts.insert("seats", seats);
    let observers: Vec<(String, Value)> = defs
        .decl
        .observers
        .iter()
        .filter(|o| !player || Some(o.name.as_str()) == profile_name.as_deref())
        .map(|o| {
            (
                o.name.clone(),
                serde_json::to_value(o).unwrap_or(Value::Null),
            )
        })
        .collect();
    parts.insert("observers", observers);
    let instruments: Vec<(String, Value)> = defs
        .decl
        .instruments
        .iter()
        .filter(|i| !player || profile.is_some_and(|p| p.instruments.contains(&i.name)))
        .map(|i| {
            let mut v = serde_json::to_value(i).unwrap_or(Value::Null);
            if let Value::Object(m) = &mut v {
                m.remove("source");
            }
            (i.name.clone(), v)
        })
        .collect();
    parts.insert("instruments", instruments);
    let kinds: Vec<(String, Value)> = defs
        .decl
        .kinds
        .iter()
        .map(|k| {
            let facts: Vec<Value> = k
                .facts
                .iter()
                .filter(|f| !player || f.exposure != perception::Exposure::Hidden)
                .map(|f| {
                    let mut v = serde_json::to_value(f).unwrap_or(Value::Null);
                    if let Value::Object(m) = &mut v {
                        m.remove("source");
                    }
                    v
                })
                .collect();
            (
                k.kind.clone(),
                json!({"kind": k.kind, "doc": k.doc, "facts": facts, "affordances": k.affordances}),
            )
        })
        .collect();
    parts.insert("kinds", kinds);
    let events: Vec<(String, Value)> = defs
        .decl
        .events
        .iter()
        .filter(|e| !player || !matches!(e.scope, perception::Scope::Hidden))
        .map(|e| {
            let data: Vec<&str> = e.data.iter().map(|f| f.name.as_str()).collect();
            (
                e.kind.clone(),
                json!({"kind": e.kind, "doc": e.doc, "scope": e.scope, "data": data}),
            )
        })
        .collect();
    parts.insert("events", events);
    let controls: Vec<(String, Value)> = catalog
        .controls
        .iter()
        .filter(|c| {
            !player || row.is_some_and(|r| catalog.seat_control(world, r, &c.name).is_some())
        })
        .map(|c| (c.name.clone(), c.to_json()))
        .collect();
    parts.insert("controls", controls);
    let intents: Vec<(String, Value)> = catalog
        .intents
        .iter()
        .filter(|i| {
            !player || row.is_some_and(|r| catalog.seat_intent(world, r, &i.def.name).is_some())
        })
        .map(|i| (i.def.name.clone(), i.def.to_json()))
        .collect();
    parts.insert("intents", intents);
    let codes_part: Vec<(String, Value)> = catalog
        .codes
        .iter()
        .map(|c| {
            (
                c.code.clone(),
                serde_json::to_value(c).unwrap_or(Value::Null),
            )
        })
        .collect();
    parts.insert("codes", codes_part);
    let rules = world.get_resource::<TimeRules>();
    let structure = rules.map_or(json!({"structure": "continuous"}), |r| {
        serde_json::to_value(&r.structure).unwrap_or(Value::Null)
    });
    parts.insert("structure", vec![("structure".to_owned(), structure)]);
    parts.insert(
        "pacing",
        vec![
            ("stepped".to_owned(), json!({"pacing": "stepped", "doc": "Ticks run only on wait (or a developer's time step); the player holds the clock."})),
            ("real_time".to_owned(), json!({"pacing": "real_time", "doc": "Ticks follow the wall clock; with pause_on_decision the world pauses at each decision point until the seat acts with resume or continues."})),
            ("current".to_owned(), serde_json::to_value(&ctl.pacing).unwrap_or(Value::Null)),
        ],
    );
    let filter = seat
        .and_then(|s| ctl.seats.get(s))
        .map_or_else(|| ctl.default_filter.clone(), |st| st.filter.clone());
    parts.insert(
        "decisions",
        vec![(
            "filter".to_owned(),
            serde_json::to_value(&filter).unwrap_or(Value::Null),
        )],
    );
    let episode = rules.map_or(
        json!({"doc": "The episode does not end by itself."}),
        |r| json!({"doc": r.doc}),
    );
    parts.insert("episode", vec![("episode".to_owned(), episode)]);
    let tick = world.resource::<SimClock>().tick.0;
    let head = |m: &mut Map<String, Value>| {
        m.insert("tick".into(), json!(tick));
        m.insert("omniscient".into(), json!(false));
        m.insert("contract".into(), json!(CONTRACT));
        m.insert("game".into(), json!(catalog.game));
    };
    let mut out = Map::new();
    head(&mut out);
    let limit = usize::try_from(budget).unwrap_or(usize::MAX) * 4;
    match part {
        Some(part) => {
            let items = parts.remove(part.name()).unwrap_or_default();
            let chosen: Vec<Value> = match name {
                Some(n) => {
                    let found: Vec<Value> = items
                        .iter()
                        .filter(|(k, _)| k == n)
                        .map(|(_, v)| v.clone())
                        .collect();
                    if found.is_empty() {
                        let valid: Vec<pocket_contract::Candidate<'_>> = items
                            .iter()
                            .map(|(k, _)| pocket_contract::Candidate::new(k))
                            .collect();
                        return Err(codes::invalid_value(
                            &Pointer::root().key("name"),
                            n,
                            &valid,
                        ));
                    }
                    found
                }
                None => items.iter().map(|(_, v)| v.clone()).collect(),
            };
            out.insert(part.name().into(), Value::Array(chosen.clone()));
            if serde_json::to_string(&out).map_or(0, |s| s.len()) > limit {
                let names: Vec<&str> = items.iter().map(|(k, _)| k.as_str()).collect();
                out.insert(part.name().into(), json!(names));
                out.insert(
                    "more".into(),
                    json!(format!(
                        "describe {{\"part\": \"{}\", \"name\": ...}} gives one item in full",
                        part.name()
                    )),
                );
            }
        }
        None => {
            for p in DescribePart::ALL {
                let items = parts.get(p.name()).cloned().unwrap_or_default();
                out.insert(
                    p.name().into(),
                    Value::Array(items.into_iter().map(|(_, v)| v).collect()),
                );
            }
            if serde_json::to_string(&out).map_or(0, |s| s.len()) > limit {
                // Names only, with the call that gives each part (README, The game definition).
                let mut short = Map::new();
                head(&mut short);
                for p in DescribePart::ALL {
                    let names: Vec<String> = parts
                        .get(p.name())
                        .map(|items| items.iter().map(|(k, _)| k.clone()).collect())
                        .unwrap_or_default();
                    short.insert(p.name().into(), json!(names));
                }
                short.insert(
                    "more".into(),
                    json!("describe {\"part\": \"intents\"} (or any part) gives that part in full"),
                );
                out = short;
            }
        }
    }
    let bytes = serde_json::to_string(&out).map_or(0, |s| s.len());
    out.insert(
        "tokens".into(),
        json!(pocket_interface::projection::tokens(bytes)),
    );
    Ok(Value::Object(out))
}

/// `player.act`'s Write (actions.md, The act request): the call validated whole and applied at
/// the boundary. Its answer's world parts and the canonical call the recorder keeps, which a replay
/// applies through the same path as a developer naming the recorded seat (actions.md, What a
/// replay records: session-level checks are skipped there).
pub fn act_apply(
    world: &mut World,
    source: Source,
    replaying: bool,
    raw: &Value,
) -> Result<(Value, Value), Problem> {
    if !declared(world) {
        return Err(no_layer("player.act"));
    }
    let who = if replaying {
        Caller::Developer
    } else {
        caller(world, source)?
    };
    let done = action::act(world, &who, raw)?;
    let answer = json!({"seat": done.seat, "applied_at": done.applied_at.0,
                        "outcomes": done.outcomes, "warnings": done.warnings});
    Ok((answer, done.canonical))
}

/// The session's part of an applied `player.act`: the seat was driven (idle restarts), `resume`
/// answers its pending decision, its later warnings, and the events delta since its push cursor
/// (perception.md, Push). The answer is actions.md's `ActResult`.
pub fn act_answer(
    world: &World,
    ctl: &mut Controller,
    applied: &Value,
    raw: &Value,
    now_ms: f64,
) -> Value {
    ctl.attach(world);
    let seat = applied["seat"].as_str().unwrap_or_default().to_owned();
    let applied_at = pocket_sim::Tick(applied["applied_at"].as_u64().unwrap_or(0));
    let resume = raw.get("resume").and_then(Value::as_bool) == Some(true);
    let mut warnings: Vec<Problem> =
        serde_json::from_value(applied["warnings"].clone()).unwrap_or_default();
    warnings.extend(pocket_interface::time::session::acted(
        ctl, &seat, applied_at, resume, now_ms,
    ));
    warnings.extend(ctl.take_warnings(&seat));
    let budget = raw
        .get("budget_tokens")
        .and_then(Value::as_u64)
        .and_then(|b| u32::try_from(b).ok());
    let tick = world.resource::<SimClock>().tick.0;
    let mut v = json!({"tick": tick, "applied_at": applied_at.0, "pending": false,
                       "outcomes": applied["outcomes"], "warnings": warnings});
    push_events(world, ctl, &seat, budget, &mut v);
    v
}

/// The events delta of `seat` since its push cursor within `budget` (default 400) tokens, into
/// `v` as `events` and `cursor`; the cursor moves past what was shown.
fn push_events(
    world: &World,
    ctl: &mut Controller,
    seat: &str,
    budget: Option<u32>,
    v: &mut Value,
) {
    let since = ctl.push.get(seat).copied().unwrap_or(0);
    let limit = usize::try_from(budget.unwrap_or(400)).unwrap_or(usize::MAX) * 4;
    if let Ok(d) = perception::delta(world, seat, since, Projection::Json, limit) {
        v["events"] = Value::Array(
            d.events
                .iter()
                .filter_map(|e| serde_json::from_str(e).ok())
                .collect(),
        );
        v["cursor"] = json!(d.cursor);
        ctl.push.insert(seat.to_owned(), d.cursor);
    }
}

/// `player.wait` (time.md, Requests, `wait`; charter 5.1, Pioneer): let time pass for the seat
/// until `until` holds, by default until its next decision point. In stepped pacing, where the
/// player holds the clock, it runs up to `ticks` ticks as a `step` would; in real time it is the
/// long poll that answers when the decision comes (the game thread's).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaitParams {
    /// A player's own; a developer names one, or follows the only seat.
    #[serde(default)]
    pub seat: Option<String>,
    /// Stepped pacing: at most this many ticks (default and largest 36000, ten minutes at 60 Hz).
    #[serde(default)]
    #[schemars(range(min = 1, max = 36000))]
    pub ticks: Option<u64>,
    /// When to answer: "decision" (the default), {"event": "crate.taken"}, {"intent": 3},
    /// {"fact": {...}} or {"any": [...]}.
    #[serde(default)]
    pub until: Option<pocket_interface::time::play::Until>,
    /// Answer at a boundary after this much wall time (default 30000, at most 600000).
    #[serde(default)]
    #[schemars(range(min = 1, max = 600000))]
    pub max_wall_ms: Option<u32>,
    /// An observation at the stop, in the same answer.
    #[serde(default)]
    pub observe: Option<ObserveRequest>,
    /// For the events delta.
    #[serde(default)]
    pub budget_tokens: Option<u32>,
}

/// The most ticks one `wait` runs (time.md, `StepRequest.ticks`).
pub const WAIT_TICKS: u64 = 36_000;

/// `player.wait`'s parameters checked, and the `step` request it runs in stepped pacing.
pub fn wait_as_step(raw: &Value) -> Result<Value, Problem> {
    let p: WaitParams = pocket_contract::decode(raw, &CheckOptions::new("the wait request"))?.value;
    let ticks = p.ticks.unwrap_or(WAIT_TICKS);
    if ticks == 0 || ticks > WAIT_TICKS {
        return Err(codes::out_of_range(
            &Pointer::root().key("ticks"),
            &json!(ticks),
            &codes::Range::inclusive(1, WAIT_TICKS),
            None,
        ));
    }
    let mut step = match raw {
        Value::Object(m) => m.clone(),
        _ => Map::new(),
    };
    step.insert("ticks".into(), json!(ticks));
    if !step.contains_key("until") {
        step.insert("until".into(), json!("decision"));
    }
    Ok(Value::Object(step))
}

/// `player.session {seat?}` (mcp.md 5.1): who this caller is and what the world waits for.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionParams {
    /// A developer's: the seat to report on (default: the only seat).
    #[serde(default)]
    pub seat: Option<String>,
}

/// `player.session`: the caller's role and seat, the seat's index (what `Source::Player` names),
/// the contract version, the pacing, the time status filtered for the caller, the seat's pending
/// decision and push cursor, and its decision statistics.
pub fn session(
    world: &World,
    ctl: &mut Controller,
    source: Source,
    raw: &Value,
    model: &pocket_interface::TimeModel,
    now_ms: f64,
) -> Result<Value, Problem> {
    let who = caller(world, source)?;
    let p: SessionParams =
        pocket_contract::decode(raw, &CheckOptions::new("the session request"))?.value;
    if !declared(world) {
        return Err(no_layer("player.session"));
    }
    ctl.attach(world);
    let seat = seat_named(world, &who, p.seat.as_deref(), "session")?;
    let rows = action::state::seats(world)?;
    let row = seat
        .as_deref()
        .and_then(|s| rows.iter().find(|r| r.id == s));
    let only = match &who {
        Caller::Player { seat } => Some(seat.as_str()),
        _ => None,
    };
    let st = seat.as_deref().and_then(|s| ctl.seats.get(s));
    let holds_clock = match &who {
        Caller::Player { .. } => ctl.players_hold_clock,
        _ => true,
    };
    let game = world
        .get_resource::<ActionCatalog>()
        .map(|c| c.game.clone())
        .unwrap_or_default();
    let seats: Vec<Value> = rows
        .iter()
        .map(|r| json!({"seat": r.id, "index": r.index}))
        .collect();
    Ok(json!({
        "tick": world.resource::<SimClock>().tick.0,
        "world": "main",
        "role": who.role(),
        "seat": seat,
        "index": row.and_then(|r| r.index),
        "seats": seats,
        "contract": CONTRACT,
        "game": game,
        "clock": holds_clock,
        "pacing": ctl.pacing,
        "status": ctl.status(world, model, only, now_ms),
        "decision": seat.as_deref().and_then(|s| ctl.pending(s)),
        "cursor": seat.as_deref().and_then(|s| ctl.push.get(s)).copied().unwrap_or(0),
        "stats": {
            "decision_points": st.map_or(0, |s| s.arisen),
            "decisions_answered": st.map_or(0, |s| s.answered),
        },
    }))
}

/// `player.pacing {pacing}` (time.md, Pacing): a developer changes the session's pacing at a
/// boundary: stepped (the player holds the clock), or real time at a speed, with pause-on-decision
/// and a thinking clock.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PacingParams {
    pub pacing: PlayPacing,
}

/// Checks `player.pacing` for the caller: a developer's, and a pacing the runtime runs.
pub fn pacing_params(world: &World, source: Source, raw: &Value) -> Result<PlayPacing, Problem> {
    if !declared(world) {
        return Err(no_layer("player.pacing"));
    }
    let who = caller(world, source)?;
    let p: PacingParams =
        pocket_contract::decode(raw, &CheckOptions::new("the pacing request"))?.value;
    if !matches!(who, Caller::Developer) {
        return Err(codes::permission_denied(
            "player.pacing",
            who.role(),
            "developer",
        ));
    }
    match &p.pacing {
        PlayPacing::Lockstep { .. } => Err(codes::time_wrong_mode(
            "player.pacing",
            "lockstep",
            "continuous",
            &["stepped", "real_time"],
        )),
        PlayPacing::RealTime { speed, .. } => {
            pocket_interface::Pacing::RealTime { speed: *speed }.check()?;
            Ok(p.pacing)
        }
        PlayPacing::Stepped => Ok(p.pacing),
    }
}
