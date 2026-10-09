//! A host whose game thread the script debugger holds (docs/spec/server.md 3.4; the first finding
//! of docs/bench/debug-eval.md): no call waits on a paused game. What presenters can answer is
//! answered here, from the last published snapshot (the world at the boundary before the tick the
//! game stands in) and from the project's files: `status`, `world.get/tree/query/schema`,
//! `scripts.list/read/status`, `snapshot` and `time.control {}` (the status), each marked
//! `paused_at`. Every other call that needs the game thread is refused at once with
//! `debug.paused`, which says where the game stands and how to go on, except `play.stop` in Play,
//! which the debugger lets through (`dispatch.rs`). A `time.step` the debugger stops answers at the
//! stop (`stopped_by`, pocket-link's `StateHandle::stopped`).
//!
//! The snapshot's JSON is the registry's format of each section, which is what the game's own
//! `world.get` answers (the paused-reads test compares them); the parameters are checked as
//! strictly as the runtime checks them.

use std::collections::BTreeMap;

use pocket_contract::{CheckOptions, Problem, detail};
use pocket_link::{SnapshotView, WorldSnapshot};
use pocket_sim::EntityId;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use crate::Host;
use crate::local::{inside, walk};

/// What a held host answers itself (besides `catalog.list`, `events.*`, `log.since`,
/// `assets.list`, `docs.search`, `scripts.guide` and `debug.*`, which never need the game).
pub const PAUSED_READS: &[&str] = &[
    "status",
    "world.get",
    "world.tree",
    "world.query",
    "world.schema",
    "scripts.list",
    "scripts.read",
    "scripts.status",
    "snapshot",
];

/// An entity as a command names it: its id, or a name (`Sloop`, or `Sloop#3` to say which).
#[derive(Deserialize, JsonSchema)]
#[serde(untagged)]
enum EntityRef {
    Id(u64),
    Name(String),
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoParams {}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetParams {
    entity: EntityRef,
    #[serde(default)]
    components: Vec<String>,
    #[serde(default)]
    fields: Vec<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct TreeParams {
    #[serde(default)]
    root: Option<EntityRef>,
    #[serde(default)]
    depth: Option<u32>,
    #[serde(default)]
    filter: Option<String>,
    #[serde(default)]
    with: Vec<String>,
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct QueryParams {
    with: Vec<String>,
    #[serde(default)]
    fields: Vec<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SchemaParams {
    #[serde(default)]
    component: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadParams {
    path: String,
    #[serde(default)]
    lines: Option<String>,
    #[serde(default)]
    numbered: bool,
}

fn decode<T: DeserializeOwned + JsonSchema>(v: &Value, owner: &str) -> Result<T, Problem> {
    let empty = json!({});
    let v = if v.is_null() { &empty } else { v };
    pocket_contract::decode::<T>(v, &CheckOptions::new(owner)).map(|d| d.value)
}

/// `scripts/helm.ts:67` of a stop's location, or `where it stopped` without one.
pub(crate) fn place(stop: &Value) -> String {
    let l = &stop["location"];
    match (l["file"].as_str(), l["line"].as_u64()) {
        (Some(f), Some(n)) => format!("{f}:{n}"),
        _ => "a script statement".to_owned(),
    }
}

/// `debug.paused`: `method` needs the game thread, which the debugger holds; or, `queued`, the call
/// was sent (before the stop, or a `time.control` while held) and runs at the boundary after the
/// held tick.
pub(crate) fn refusal(method: &str, stop: &Value, queued: bool) -> Problem {
    let at = place(stop);
    let reason = stop["reason"].as_str().unwrap_or("pause");
    let tick = &stop["tick"];
    let message = if queued {
        format!(
            "The game stands at {at} ({reason}, tick {tick}); {method} is queued and runs at the \
             boundary after this tick, before any other tick, once debug.continue (or \
             debug.step) lets the tick finish."
        )
    } else {
        format!(
            "The game stands at {at} ({reason}, tick {tick}); {method} needs the game thread, \
             which waits for debug.continue (or debug.step). Meanwhile status, world \
             get/tree/query/schema, scripts list/read/status, events, logs and debug.* answer. \
             A breakpoint the next tick hits holds the game again right after a continue: to act \
             between ticks, clear it (debug.breakpoints.clear) first; play.stop ends Play \
             wherever it stands."
        )
    };
    Problem::new(
        "debug.paused",
        message,
        detail([
            ("method", json!(method)),
            ("location", stop["location"].clone()),
            ("reason", json!(reason)),
            ("tick", tick.clone()),
            ("system", stop["system"].clone()),
            ("queued", json!(queued)),
            ("stop", stop.clone()),
            ("answers", json!(PAUSED_READS)),
        ]),
    )
}

/// A field inside a component's JSON: `speed`, `position.1`, `position.y` (as the runtime's
/// `inspect::field_at`).
fn field_at<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut at = value;
    for part in path.split('.').filter(|p| !p.is_empty()) {
        at = match at {
            Value::Object(m) => m.get(part)?,
            Value::Array(a) => {
                let i = match part {
                    "x" => 0,
                    "y" => 1,
                    "z" => 2,
                    "w" => 3,
                    n => n.parse().ok()?,
                };
                a.get(i)?
            }
            _ => return None,
        };
    }
    Some(at)
}

fn no_field(spec: &str, component: &str, path: &str, value: &Value) -> Problem {
    let keys: Vec<&str> = value
        .as_object()
        .map(|m| m.keys().map(String::as_str).collect())
        .unwrap_or_default();
    let first = path.split('.').next().unwrap_or(path);
    let suggestions = pocket_contract::suggest_names(first, keys.iter().copied());
    Problem::new(
        "request.invalid_value",
        format!(
            "{component} has no field '{path}'; it has {}.",
            if keys.is_empty() {
                "no fields".to_owned()
            } else {
                keys.join(", ")
            }
        ),
        detail([
            ("path", json!("/fields")),
            ("field", json!(spec)),
            ("suggestions", json!(suggestions)),
            ("allowed", json!(keys)),
        ]),
    )
}

/// The lines `spec` of a text, numbered if asked (as the runtime's `files::slice_lines`).
fn slice_lines(
    text: &str,
    spec: Option<&str>,
    numbered: bool,
) -> Result<(String, usize, usize, usize), Problem> {
    let all: Vec<&str> = text.lines().collect();
    let total = all.len();
    let bad = |why: String| {
        Problem::new(
            "request.invalid_value",
            why,
            detail([("path", json!("/lines")), ("lines", json!(total))]),
        )
    };
    let (first, last) = match spec.map(str::trim) {
        None | Some("") => (1, total),
        Some(s) => {
            let num = |t: &str| -> Result<usize, Problem> {
                t.trim()
                    .parse::<usize>()
                    .map_err(|_| bad(format!("'{s}' is not a line range (50-80, 50-, -20, 67).")))
            };
            match s.split_once('-') {
                Some((a, b)) => (
                    if a.trim().is_empty() { 1 } else { num(a)? },
                    if b.trim().is_empty() { total } else { num(b)? },
                ),
                None => {
                    let n = num(s)?;
                    (n, n)
                }
            }
        }
    };
    if first == 0 || first > last.max(1) || (first > total && total > 0) {
        return Err(bad(format!(
            "Lines {first} to {last} are not in the file, which has {total} lines."
        )));
    }
    let last = last.min(total);
    let width = last.to_string().len();
    let mut out = String::new();
    for (i, line) in all.iter().enumerate().take(last).skip(first - 1) {
        if numbered {
            out.push_str(&format!("{:>width$}| ", i + 1));
        }
        out.push_str(line);
        out.push('\n');
    }
    Ok((out, first, last, total))
}

/// The world a publication shows, read as the runtime's readers read a live one.
struct Shown<'a> {
    snap: &'a WorldSnapshot,
    view: SnapshotView<'a>,
    ids: Vec<u64>,
    names: BTreeMap<u64, String>,
    /// Component names in the registry's order (ascending), `Name` included.
    components: Vec<String>,
    rows: BTreeMap<String, BTreeMap<u64, Value>>,
}

impl<'a> Shown<'a> {
    fn new(snap: &'a WorldSnapshot) -> Result<Shown<'a>, Problem> {
        let view = SnapshotView::new(snap);
        let ids = view.entities()?.into_iter().map(EntityId::get).collect();
        let names = view
            .rows_json("Name")
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(id, v)| Some((id, v.as_str()?.to_owned())))
            .collect();
        let components = snap
            .registry
            .components
            .iter()
            .map(|c| c.name.clone())
            .collect();
        Ok(Shown {
            snap,
            view,
            ids,
            names,
            components,
            rows: BTreeMap::new(),
        })
    }

    fn known(&self, name: &str) -> Result<(), Problem> {
        if self.components.iter().any(|c| c == name) {
            return Ok(());
        }
        let suggestions =
            pocket_contract::suggest_names(name, self.components.iter().map(String::as_str));
        Err(Problem::new(
            "sim.component_unknown",
            format!("There is no component '{name}'; did you mean {suggestions:?}?"),
            detail([
                ("component", json!(name)),
                ("suggestions", json!(suggestions)),
            ]),
        ))
    }

    /// Every row of a component, decoded once.
    fn rows(&mut self, component: &str) -> Result<&BTreeMap<u64, Value>, Problem> {
        if !self.rows.contains_key(component) {
            let rows = self.view.rows_json(component)?.into_iter().collect();
            self.rows.insert(component.to_owned(), rows);
        }
        Ok(&self.rows[component])
    }

    fn value(&mut self, id: u64, component: &str) -> Result<Option<Value>, Problem> {
        self.known(component)?;
        Ok(self.rows(component)?.get(&id).cloned())
    }

    /// The registry's components `id` has, `Name` left out (as `world.tree` lists them).
    fn components_of(&mut self, id: u64) -> Result<Vec<String>, Problem> {
        let names: Vec<String> = self
            .components
            .iter()
            .filter(|c| c.as_str() != "Name")
            .cloned()
            .collect();
        let mut out = Vec::new();
        for c in names {
            if self.rows(&c)?.contains_key(&id) {
                out.push(c);
            }
        }
        Ok(out)
    }

    fn resolve(&self, r: &EntityRef) -> Result<u64, Problem> {
        match r {
            EntityRef::Id(n) => {
                if self.ids.binary_search(n).is_ok() {
                    Ok(*n)
                } else {
                    Err(Problem::new(
                        "sim.entity_not_found",
                        format!("Entity {n} no longer exists."),
                        detail([("id", json!(n))]),
                    ))
                }
            }
            EntityRef::Name(s) => {
                let (name, want) = match s.rsplit_once('#') {
                    Some((name, id)) if id.parse::<u64>().is_ok() => (name, id.parse::<u64>().ok()),
                    _ => (s.as_str(), None),
                };
                let found: Vec<u64> = self
                    .names
                    .iter()
                    .filter(|(i, n)| n.as_str() == name && want.is_none_or(|w| w == **i))
                    .map(|(i, _)| *i)
                    .collect();
                match found.as_slice() {
                    [id] => Ok(*id),
                    [] => {
                        let suggestions = pocket_contract::suggest_names(
                            s,
                            self.names.values().map(String::as_str),
                        );
                        Err(Problem::new(
                            "sim.entity_not_found",
                            format!("No live entity is named '{s}'; did you mean {suggestions:?}?"),
                            detail([("ref", json!(s)), ("suggestions", json!(suggestions))]),
                        ))
                    }
                    many => {
                        let c: Vec<(u64, &str, &str)> =
                            many.iter().map(|i| (*i, name, "entity")).collect();
                        Err(pocket_contract::codes::ambiguous_ref(
                            &pocket_contract::Pointer::root().key("entity"),
                            s,
                            &c,
                        ))
                    }
                }
            }
        }
    }

    fn name_of(&self, id: u64) -> Value {
        self.names.get(&id).map_or(Value::Null, |n| json!(n))
    }

    fn get(&mut self, p: &GetParams) -> Result<Value, Problem> {
        let id = self.resolve(&p.entity)?;
        let (named, dotted): (Vec<String>, Vec<String>) =
            p.components.iter().cloned().partition(|c| !c.contains('.'));
        let fields: Vec<String> = dotted.into_iter().chain(p.fields.clone()).collect();
        let all = named.is_empty() && fields.is_empty();
        let wanted = if all { self.components.clone() } else { named };
        let mut out = Map::new();
        for c in &wanted {
            if let Some(v) = self.value(id, c)? {
                out.insert(c.clone(), v);
            }
        }
        let mut answer = json!({"id": id, "name": self.name_of(id), "components": out});
        if !fields.is_empty() {
            let mut got = Map::new();
            for spec in &fields {
                let (c, path) = spec.split_once('.').unwrap_or((spec, ""));
                let v = match self.value(id, c)? {
                    None => Value::Null,
                    Some(v) => field_at(&v, path)
                        .cloned()
                        .ok_or_else(|| no_field(spec, c, path, &v))?,
                };
                got.insert(spec.clone(), v);
            }
            answer["fields"] = Value::Object(got);
        }
        Ok(answer)
    }

    fn has_all(&mut self, id: u64, with: &[String]) -> Result<bool, Problem> {
        for c in with {
            if !self.rows(c)?.contains_key(&id) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn name_matches(&self, id: u64, filter: Option<&str>) -> bool {
        match filter {
            None => true,
            Some(f) => self
                .names
                .get(&id)
                .is_some_and(|n| n.to_lowercase().contains(&f.to_lowercase())),
        }
    }

    fn tree(&mut self, p: &TreeParams, paused: &Value) -> Result<Value, Problem> {
        for c in &p.with {
            self.known(c)?;
        }
        let ids = match &p.root {
            Some(r) => vec![self.resolve(r)?],
            None => self.ids.clone(),
        };
        let limit = p
            .limit
            .map_or(usize::MAX, |l| usize::try_from(l).unwrap_or(usize::MAX));
        let mut out = Vec::new();
        for id in ids {
            if out.len() >= limit {
                break;
            }
            if !self.has_all(id, &p.with)? || !self.name_matches(id, p.filter.as_deref()) {
                continue;
            }
            out.push(json!({"id": id, "name": self.name_of(id),
                            "components": self.components_of(id)?, "children": [],
                            "paused_at": paused}));
        }
        Ok(Value::Array(out))
    }

    fn query(&mut self, p: &QueryParams, paused: &Value) -> Result<Value, Problem> {
        if p.with.is_empty() {
            return Err(Problem::new(
                "request.invalid_value",
                "world.query needs at least one component in 'with'.",
                detail([("path", json!("/with"))]),
            ));
        }
        for c in &p.with {
            self.known(c)?;
        }
        for f in &p.fields {
            self.known(f.split_once('.').map_or(f.as_str(), |(c, _)| c))?;
        }
        let limit = usize::try_from(p.limit.unwrap_or(100)).unwrap_or(usize::MAX);
        let mut out = Vec::new();
        for id in self.ids.clone() {
            if out.len() >= limit {
                break;
            }
            if !self.has_all(id, &p.with)? || !self.name_matches(id, p.name.as_deref()) {
                continue;
            }
            let mut row = Map::new();
            row.insert("id".into(), json!(id));
            row.insert("name".into(), self.name_of(id));
            for spec in &p.fields {
                let (c, path) = spec.split_once('.').unwrap_or((spec, ""));
                let v = self
                    .value(id, c)?
                    .and_then(|v| field_at(&v, path).cloned())
                    .unwrap_or(Value::Null);
                row.insert(spec.clone(), v);
            }
            row.insert("paused_at".into(), paused.clone());
            out.push(Value::Object(row));
        }
        Ok(Value::Array(out))
    }

    fn schema(&self, p: &SchemaParams) -> Result<Value, Problem> {
        let all = &self.snap.registry.components;
        match &p.component {
            Some(name) => {
                self.known(name)?;
                Ok(all
                    .iter()
                    .find(|c| &c.name == name)
                    .map(|c| serde_json::to_value(c).unwrap_or(Value::Null))
                    .unwrap_or(Value::Null))
            }
            None => Ok(serde_json::to_value(all).unwrap_or(Value::Null)),
        }
    }
}

impl Host {
    /// Where the script debugger holds the game thread, while it does: the stop's summary
    /// (`{reason, tick, system, location, breakpoint?, watch?, exception?}`).
    pub fn debug_stop(&self) -> Option<Value> {
        self.0.reader.debug_stop()
    }

    /// The status of a held game, from the last publication and the reader.
    fn paused_status(&self, snap: &WorldSnapshot, stop: &Value) -> Result<Value, Problem> {
        let reader = &self.0.reader;
        let world = reader.world();
        let entities = SnapshotView::new(snap).entities()?.len();
        let t = &snap.time;
        let h = snap.snapshot.header();
        Ok(json!({
            "tick": t.tick,
            "t_s": t.t_s,
            "pacing": t.pacing,
            "paused": t.paused,
            "halted": t.halted,
            "behind_ms": t.behind_ms,
            "mode": world.mode.as_str(),
            "epoch": world.epoch,
            "world_hash": snap.snapshot.world_hash().to_string(),
            "entities": entities,
            "bundle": h.bundle.to_hex(),
            "writes": h.writes,
            "state": "breakpoint",
            "paused_at": stop,
        }))
    }

    /// Answers a read while the debugger holds the game, or `None` when the game is running or
    /// the method needs the game thread.
    pub(crate) fn paused_read(
        &self,
        method: &str,
        params: &Value,
    ) -> Option<Result<Value, Problem>> {
        let stop = self.0.reader.debug_stop()?;
        let unchanged = params.is_null() || params.as_object().is_some_and(|m| m.is_empty());
        let method = match method {
            "world_get" => "world.get",
            // `time.control {}` changes nothing and answers the status (the editor's Stop reads
            // the Play world's bundle with it).
            "time.control" | "time_control" if unchanged => "status",
            m => m,
        };
        if !PAUSED_READS.contains(&method) {
            return None;
        }
        let snap = self.0.reader.latest();
        let snapshot_tick = snap.snapshot.header().tick.0;
        let mut marker = stop.clone();
        marker["snapshot_tick"] = json!(snapshot_tick);
        let tick = stop["tick"].clone();
        Some((|| -> Result<Value, Problem> {
            match method {
                "status" => {
                    decode::<NoParams>(params, method)?;
                    self.paused_status(&snap, &marker)
                }
                "world.get" => {
                    let p: GetParams = decode(params, method)?;
                    let mut v = Shown::new(&snap)?.get(&p)?;
                    v["paused_at"] = marker;
                    Ok(v)
                }
                "world.tree" => {
                    let p: TreeParams = decode(params, method)?;
                    Shown::new(&snap)?.tree(&p, &tick)
                }
                "world.query" => {
                    let p: QueryParams = decode(params, method)?;
                    Shown::new(&snap)?.query(&p, &tick)
                }
                "world.schema" => {
                    let p: SchemaParams = decode(params, method)?;
                    Shown::new(&snap)?.schema(&p)
                }
                "scripts.list" => {
                    decode::<NoParams>(params, method)?;
                    let root = &self.0.project;
                    let mut files = Vec::new();
                    walk(root, &root.join("scripts"), &mut files);
                    Ok(Value::Array(
                        files
                            .into_iter()
                            .map(|(path, bytes)| {
                                json!({"path": path, "bytes": bytes, "diagnostics": []})
                            })
                            .collect(),
                    ))
                }
                "scripts.read" => {
                    let p: ReadParams = decode(params, method)?;
                    self.read_script(&p)
                }
                "scripts.status" => {
                    decode::<NoParams>(params, method)?;
                    let bundle = snap.snapshot.header().bundle.to_hex();
                    Ok(json!({"bundle": bundle, "ran_last_tick": null, "paused_at": marker}))
                }
                "snapshot" => {
                    decode::<NoParams>(params, method)?;
                    let h = snap.snapshot.header();
                    Ok(json!({"tick": h.tick.0, "writes": h.writes,
                              "world_hash": snap.snapshot.world_hash().to_string(),
                              "paused_at": marker}))
                }
                _ => Err(refusal(method, &stop, false)),
            }
        })())
    }

    /// `scripts.read` from the project's files.
    fn read_script(&self, p: &ReadParams) -> Result<Value, Problem> {
        let rel = p.path.strip_prefix("scripts/").unwrap_or(&p.path);
        if !inside(rel) {
            return Err(Problem::new(
                "request.invalid_value",
                format!("'{}' is not a path inside scripts/.", p.path),
                detail([("path", json!(p.path))]),
            ));
        }
        let rel = format!("scripts/{rel}");
        let full = self.0.project.join(&rel);
        let text = std::fs::read_to_string(&full).map_err(|e| {
            let code = if e.kind() == std::io::ErrorKind::NotFound {
                "project.missing_file"
            } else {
                "project.unreadable"
            };
            Problem::new(
                code,
                format!("{}: {e}.", full.display()),
                detail([
                    ("path", json!(full.display().to_string())),
                    ("error", json!(e.to_string())),
                ]),
            )
        })?;
        if p.lines.is_none() && !p.numbered {
            return Ok(json!({"path": rel, "text": text}));
        }
        let (text, first, last, total) = slice_lines(&text, p.lines.as_deref(), p.numbered)?;
        Ok(json!({"path": rel, "text": text, "first": first, "last": last, "total": total}))
    }

    /// An entity named by id, name or `Name#id`, resolved in the last publication: the host's
    /// `debug.watch` takes names as every other command does and hands the debugger the id.
    pub(crate) fn entity_id(&self, entity: &Value) -> Result<u64, Problem> {
        let r: EntityRef = serde_json::from_value(entity.clone()).map_err(|_| {
            Problem::new(
                "request.wrong_type",
                "'entity' must be an entity id, a name or Name#id.",
                detail([("path", json!("/entity")), ("got", entity.clone())]),
            )
        })?;
        let snap = self.0.reader.latest();
        Shown::new(&snap)?.resolve(&r)
    }
}
