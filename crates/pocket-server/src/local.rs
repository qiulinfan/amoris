//! Methods the server answers itself (docs/spec/server.md, methods): the merged catalog, the event
//! and log streams `pocket-link` keeps for presenters, the project's asset files, the type check
//! stage of `scripts.apply` and `scripts.check`, the script guide (`scripts.guide`), and the
//! plug-ins (`debug.*`, `capture`), which answer `*.not_available` until the integrator installs
//! them.

use std::path::{Component, Path};

use pocket_contract::{CheckOptions, Problem, detail};
use schemars::JsonSchema;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::{Host, Via, typecheck};

/// docs/sdk.md, the agent's guide to game scripts, served by `scripts.guide` so that an agent with
/// only the host (MCP, the CLI outside the repository) can read it.
const GUIDE: &str = include_str!("../../../docs/sdk.md");

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoParams {}

/// `events.since`'s parameters.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EventsSince {
    /// Events after this stream number; default: the newest.
    #[serde(default)]
    seq: Option<u64>,
    /// At most this many, default 100.
    #[serde(default)]
    limit: Option<u32>,
    /// One event name, or a prefix written `crate.*`.
    #[serde(default)]
    name: Option<String>,
}

/// `events.why`'s parameters.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EventsWhy {
    /// The event's stream number.
    seq: u64,
}

/// `log.since`'s parameters.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LogSince {
    /// Lines after this stream number; default: the newest.
    #[serde(default)]
    seq: Option<u64>,
    /// At most this many, default 100.
    #[serde(default)]
    limit: Option<u32>,
}

/// `assets.list`'s parameters.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AssetsList {
    /// A directory inside the project; default: the whole project.
    #[serde(default)]
    dir: Option<String>,
}

/// `scripts.apply`'s parameters as the server sees them (the runtime checks them strictly).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct ScriptsApply {
    #[serde(default)]
    files: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default)]
    force: bool,
    #[serde(default)]
    dry_run: bool,
}

/// `docs.search`'s parameters.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DocsSearch {
    /// Words to look for in command and component names and docs.
    query: String,
    #[serde(default)]
    limit: Option<u32>,
}

fn schema<T: JsonSchema>() -> Value {
    schemars::schema_for!(T).to_value()
}

/// The server's own entries of the catalog.
pub fn server_catalog() -> Vec<Value> {
    let e = |name: &str, kind: &str, doc: &str, params: Value| {
        json!({"name": name, "kind": kind, "doc": doc, "aliases": [], "thread": false,
               "params": params, "server": true})
    };
    let any = json!({"type": "object"});
    vec![
        e(
            "events.since",
            "read",
            "Game events after a stream number (the newest without one), filtered by name.",
            schema::<EventsSince>(),
        ),
        e(
            "events.why",
            "read",
            "An event and the chain of events that caused it.",
            schema::<EventsWhy>(),
        ),
        e(
            "log.since",
            "read",
            "Script console lines, failed system runs and host messages.",
            schema::<LogSince>(),
        ),
        e(
            "assets.list",
            "read",
            "The project's files with their kind and size (served at /assets/<path>).",
            schema::<AssetsList>(),
        ),
        e(
            "scripts.check",
            "read",
            "Compiles and loads the scripts without swapping, writes their declarations under .pocket/types and type checks them with tsc (TypeScript 7) when installed.",
            schema::<NoParams>(),
        ),
        e(
            "scripts.guide",
            "read",
            "How game scripts are written (docs/sdk.md): the pocket SDK, one example per concept, common mistakes and their messages.",
            schema::<NoParams>(),
        ),
        e(
            "docs.search",
            "read",
            "Searches command and component names and docs, and the sections of the script guide.",
            schema::<DocsSearch>(),
        ),
        e(
            "subscribe",
            "read",
            "WebSocket only: the topics pushed to this socket (status is always on).",
            json!({"type": "object", "properties": {"topics": {"type": "array",
                   "items": {"type": "string"}}}}),
        ),
        e(
            "capture",
            "read",
            "A rendered image or the id buffer's summary of a camera view.",
            any,
        ),
    ]
}

/// `debug.rewind`'s parameters: the host's own debugger method (it keeps the snapshots).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RewindParams {
    /// The tick to stand at: the kept snapshot at or before it is restored and stepped to it.
    pub tick: u64,
    /// The scripts the rewound world runs: `applied` (default, the bundle running now) or
    /// `snapshot` (the bundle the snapshot was kept with).
    #[serde(default)]
    pub bundle: Option<String>,
}

/// The catalog entries of the debugger installed as `hub` (its methods, and the host's
/// `debug.rewind`), or a placeholder that says no debugger is installed.
pub fn debug_catalog(hub: Option<&dyn crate::DebugHub>) -> Vec<Value> {
    let entry = |m: &Value| {
        json!({"name": m["name"], "kind": m.get("kind").cloned().unwrap_or(json!("read")),
               "doc": m["doc"], "aliases": m.get("aliases").cloned().unwrap_or(json!([])),
               "thread": false, "params": m["params"], "server": true})
    };
    let Some(hub) = hub else {
        return vec![
            json!({"name": "debug.state", "kind": "read", "aliases": [], "thread": false,
            "server": true, "params": {"type": "object"},
            "doc": "The script debugger (debug.*): not installed in this host."}),
        ];
    };
    let mut out: Vec<Value> = hub.methods().iter().map(entry).collect();
    out.push(entry(&json!({
        "name": "debug.rewind",
        "kind": "control",
        "doc": "Time travel: restores the kept snapshot at or before tick (one every 60 ticks) \
                under the applied scripts (bundle: snapshot for the snapshot's own) and steps \
                the world to tick; a breakpoint on the way stops it.",
        "params": schema::<RewindParams>(),
    })));
    out
}

fn decode<T: DeserializeOwned + JsonSchema>(v: &Value, owner: &str) -> Result<T, Problem> {
    pocket_contract::decode::<T>(v, &CheckOptions::new(owner)).map(|d| d.value)
}

fn not_available(what: &str, method: &str) -> Problem {
    Problem::new(
        &format!("{what}.not_available"),
        format!("{method} is not available yet: this host has no {what} installed."),
        detail([("method", json!(method))]),
    )
}

fn usize_of(n: Option<u32>, default: usize) -> usize {
    n.map_or(default, |n| usize::try_from(n).unwrap_or(usize::MAX))
}

/// A file's kind by its extension.
fn kind_of(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "gltf" | "glb" | "obj" | "fbx" => "model",
        "png" | "jpg" | "jpeg" | "ktx2" | "hdr" | "exr" | "webp" => "texture",
        "ply" | "splat" | "spz" => "splat",
        "ts" | "js" => "script",
        "wav" | "ogg" | "mp3" | "flac" => "audio",
        "json" | "toml" | "jsonl" => "data",
        "wgsl" => "shader",
        _ => "file",
    }
}

/// Every file under `dir` (relative to `root`), dot files and directories left out.
pub(crate) fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, u64)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.filter_map(Result::ok).collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for e in entries {
        if e.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let p = e.path();
        if p.is_dir() {
            walk(root, &p, out);
        } else if let Ok(rel) = p.strip_prefix(root) {
            let key = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            out.push((key, e.metadata().map(|m| m.len()).unwrap_or(0)));
        }
    }
}

/// A relative path inside the project: no `..`, no root, no dot files.
pub(crate) fn inside(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path).components().all(|c| match c {
            Component::Normal(s) => !s.to_string_lossy().starts_with('.'),
            _ => false,
        })
}

impl Host {
    /// The catalog: the runtime's commands (fetched once), the server's and the debugger's.
    pub async fn catalog(&self) -> Result<Value, Problem> {
        let runtime = match self.0.catalog.get() {
            Some(c) => c.clone(),
            None => {
                let c = self.game(&Via::Api, "catalog.list", json!({})).await?;
                let _ = self.0.catalog.set(c.clone());
                c
            }
        };
        let mut all = match runtime {
            Value::Array(a) => a,
            _ => Vec::new(),
        };
        all.extend(server_catalog());
        let hub = self.0.debug.read().ok().and_then(|h| h.clone());
        all.extend(debug_catalog(hub.as_deref()));
        all.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        Ok(Value::Array(all))
    }

    /// `debug.rewind {tick, bundle?}`: the kept snapshot at or before `tick` restored (under the
    /// applied scripts unless `bundle: "snapshot"`), then stepped to `tick`.
    async fn rewind(&self, via: &Via, params: Value) -> Result<Value, Problem> {
        let p: RewindParams = decode(&params, "debug.rewind")?;
        let mut restore = json!({"tick": p.tick});
        if let Some(b) = &p.bundle {
            restore["bundle"] = json!(b);
        }
        let restored = self.game(via, "snapshots.restore", restore).await?;
        if let Ok(h) = self.game(&Via::Api, "history.list", json!({})).await {
            self.push("history", h);
        }
        let at = restored["restored"].as_u64().unwrap_or(p.tick);
        let mut out = json!({"restored": at, "tick": at, "scripts": restored["scripts"]});
        if p.tick > at {
            let stepped = self
                .game(via, "time.step", json!({"ticks": p.tick - at}))
                .await?;
            out["tick"] = stepped["tick"].clone();
            if !stepped["stopped_by"].is_null() {
                out["stopped_by"] = stepped["stopped_by"].clone();
            }
        }
        Ok(out)
    }

    /// The registry's components as `world.schema` lists them, from the last publication (the
    /// game's own registry, so it answers while the game is held).
    fn schemas(&self) -> Vec<Value> {
        let snap = self.0.reader.latest();
        snap.registry
            .components
            .iter()
            .map(|c| serde_json::to_value(c).unwrap_or(Value::Null))
            .collect()
    }

    /// Answers `method` on the server side, or `None` when the game answers it.
    pub(crate) async fn local(
        &self,
        via: &Via,
        method: &str,
        params: Value,
    ) -> Option<Result<Value, Problem>> {
        if method == "catalog.list" {
            return Some(match decode::<NoParams>(&params, method) {
                Ok(_) => self.catalog().await,
                Err(e) => Err(e),
            });
        }
        self.local_sync(via, method, params).await
    }

    async fn local_sync(
        &self,
        via: &Via,
        method: &str,
        params: Value,
    ) -> Option<Result<Value, Problem>> {
        let reader = &self.0.reader;
        Some(match method {
            "events.since" => decode::<EventsSince>(&params, method).map(|p| {
                pocket_link::events_since(reader, p.seq, usize_of(p.limit, 100), p.name.as_deref())
            }),
            "events.why" => decode::<EventsWhy>(&params, method)
                .and_then(|p| pocket_link::events_why(reader, p.seq)),
            "log.since" => decode::<LogSince>(&params, method).map(|p| {
                let limit = usize_of(p.limit, 100);
                let mut next = p.seq.map_or(1, |s| s + 1);
                let mut lines = reader.logs(&mut next, usize::MAX);
                if p.seq.is_none() {
                    lines = lines.split_off(lines.len().saturating_sub(limit));
                } else {
                    lines.truncate(limit);
                }
                let last = lines.last().map_or(next.saturating_sub(1), |l| l.seq);
                json!({"lines": lines, "last": last})
            }),
            "assets.list" => decode::<AssetsList>(&params, method).and_then(|p| self.assets(p)),
            "scripts.apply" => match decode::<ScriptsApply>(&params, method) {
                Ok(_) => {
                    // The game compiles, swaps and writes the declarations of what it loaded in
                    // one pass; tsc runs after it in a child process, so it never delays a swap,
                    // and a refused swap still gets its type errors.
                    let mut params = params;
                    params["types"] = json!(true);
                    let r = self.game(via, method, params).await;
                    if held(&r) {
                        return Some(r);
                    }
                    let tc = self.typecheck(&r).await;
                    merge_typecheck(r, tc)
                }
                Err(e) => Err(e),
            },
            "scripts.check" => match decode::<NoParams>(&params, method) {
                Ok(_) => {
                    let dry = json!({"dry_run": true, "types": true});
                    let r = self.game(via, "scripts.apply", dry).await;
                    if held(&r) {
                        return Some(r);
                    }
                    let tc = self.typecheck(&r).await;
                    let r = match r {
                        Ok(v) => Ok(v),
                        Err(p) if p.code == "scripts.refused" => Ok(json!({
                            "outcome": "refused",
                            "diagnostics": p.detail.get("diagnostics").cloned()
                                .unwrap_or(json!([])),
                        })),
                        Err(p) => Err(p),
                    };
                    merge_typecheck(r, tc)
                }
                Err(e) => Err(e),
            },
            "scripts.guide" => decode::<NoParams>(&params, method)
                .map(|_| json!({"path": "docs/sdk.md", "text": GUIDE})),
            "docs.search" => match decode::<DocsSearch>(&params, method) {
                Ok(p) => self.docs(&p.query, usize_of(p.limit, 10)).await,
                Err(e) => Err(e),
            },
            "capture" => {
                let hub = self.0.capture.read().ok().and_then(|h| h.clone());
                match hub {
                    Some(h) => h.capture(params).await,
                    None => Err(not_available("capture", method)),
                }
            }
            m if m.starts_with("debug.") => {
                let hub = self.0.debug.read().ok().and_then(|h| h.clone());
                match hub {
                    Some(_) if m == "debug.rewind" => self.rewind(via, params).await,
                    Some(h) => {
                        let mut params = params;
                        // Entities by name, as every other command takes them.
                        if m == "debug.watch"
                            && let Some(e) = params.get("entity").filter(|e| e.is_string())
                        {
                            match self.entity_id(e) {
                                Ok(id) => params["entity"] = json!(id),
                                Err(p) => return Some(Err(p)),
                            }
                        }
                        h.call(m, params).await
                    }
                    None => Err(not_available("debug", m)),
                }
            }
            _ => return None,
        })
    }

    /// `tsc` over the declarations the game wrote with `scripts.apply {types}` (its answer's
    /// `types`, or its refusal's): the type check of `scripts.apply` and `scripts.check`, in a
    /// child process.
    async fn typecheck(&self, r: &Result<Value, Problem>) -> Value {
        let t = match r {
            Ok(v) => v.get("types"),
            Err(p) => p.detail.get("types"),
        }
        .cloned()
        .unwrap_or(Value::Null);
        if let Some(code) = t.get("error") {
            return json!({"typecheck": "unavailable", "diagnostics": [],
                          "reason": format!("the declarations were not written ({}): {}",
                                            code.as_str().unwrap_or(""),
                                            t["message"].as_str().unwrap_or("")),
                          "types": t});
        }
        let mut tc = typecheck::run(&self.0.project).await;
        tc["types"] = json!({"dir": t["dir"], "tsconfig": t["tsconfig"],
                             "project_from": t["project_from"]});
        tc
    }

    fn assets(&self, p: AssetsList) -> Result<Value, Problem> {
        let root = &self.0.project;
        let dir = match &p.dir {
            Some(d) if !inside(d) => {
                return Err(Problem::new(
                    "request.invalid_value",
                    format!("'{d}' is not a directory inside the project."),
                    detail([("path", json!("/dir"))]),
                ));
            }
            Some(d) => root.join(d),
            None => root.clone(),
        };
        let mut files = Vec::new();
        walk(root, &dir, &mut files);
        Ok(Value::Array(
            files
                .into_iter()
                .map(|(path, bytes)| json!({"path": path, "kind": kind_of(&path), "bytes": bytes}))
                .collect(),
        ))
    }

    async fn docs(&self, query: &str, limit: usize) -> Result<Value, Problem> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let score = |text: &str| {
            let t = text.to_lowercase();
            words.iter().filter(|w| t.contains(w.as_str())).count()
        };
        let mut hits: Vec<(usize, Value)> = Vec::new();
        if let Value::Array(cmds) = self.catalog().await? {
            for c in cmds {
                let name = c["name"].as_str().unwrap_or("");
                let doc = c["doc"].as_str().unwrap_or("");
                let s = score(&format!("{name} {doc} {}", c["params"]));
                if s > 0 {
                    hits.push((s + score(name) * 2, json!({"command": name, "doc": doc})));
                }
            }
        }
        // The guide's sections, as pointers: `scripts.guide` returns the whole text.
        for section in GUIDE.split("\n## ").skip(1) {
            let title = section.lines().next().unwrap_or("");
            let s = score(section);
            if s > 0 {
                hits.push((
                    s + score(title) * 2,
                    json!({"guide": title, "read": "scripts.guide (docs/sdk.md)"}),
                ));
            }
        }
        for c in self.schemas() {
            let name = c["name"].as_str().unwrap_or("");
            let doc = c["doc"].as_str().unwrap_or("");
            let s = score(&format!("{name} {doc} {}", c["schema"]));
            if s > 0 {
                let fields: Vec<&String> = c["schema"]["properties"]
                    .as_object()
                    .map(|m| m.keys().collect())
                    .unwrap_or_default();
                hits.push((
                    s + score(name) * 2,
                    json!({"component": name, "doc": doc, "fields": fields}),
                ));
            }
        }
        hits.sort_by_key(|h| std::cmp::Reverse(h.0));
        Ok(Value::Array(
            hits.into_iter().take(limit).map(|(_, v)| v).collect(),
        ))
    }
}

/// A compile the debugger kept from running (`debug.paused`): nothing to type check.
fn held(r: &Result<Value, Problem>) -> bool {
    matches!(r, Err(p) if p.code == "debug.paused")
}

/// Adds the type check's outcome to a compile's result.
fn merge_typecheck(r: Result<Value, Problem>, tc: Value) -> Result<Value, Problem> {
    match r {
        Ok(mut v) => {
            if let Some(d) = tc["diagnostics"].as_array()
                && let Some(all) = v["diagnostics"].as_array_mut()
            {
                all.extend(d.iter().cloned());
            }
            v["typecheck"] = tc["typecheck"].clone();
            for k in ["tsc_ms", "tsc_version", "reason", "types"] {
                if !tc[k].is_null() {
                    v[k] = tc[k].clone();
                }
            }
            Ok(v)
        }
        Err(mut p) => {
            p.detail.insert("typecheck".into(), tc["typecheck"].clone());
            p.detail.insert("types".into(), tc["types"].clone());
            if let Some(d) = tc["diagnostics"].as_array()
                && !d.is_empty()
            {
                p.detail
                    .insert("typecheck_diagnostics".into(), Value::Array(d.clone()));
            }
            Err(p)
        }
    }
}
