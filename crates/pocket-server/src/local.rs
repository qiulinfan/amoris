//! Methods the server answers itself (docs/spec/server.md, methods): the merged catalog, the event
//! and log streams `pocket-link` keeps for presenters, the project's asset files, the type check
//! stage of `scripts.apply` and `scripts.check`, and the plug-ins (`debug.*`, `capture`), which
//! answer `*.not_available` until the integrator installs them.

use std::path::{Component, Path};

use pocket_contract::{CheckOptions, Problem, detail};
use schemars::JsonSchema;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::{Host, Via, typecheck};

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
            "Type checks (when tsc is installed) and compiles the scripts without swapping.",
            schema::<NoParams>(),
        ),
        e(
            "docs.search",
            "read",
            "Searches command and component names and docs.",
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
            any.clone(),
        ),
        e(
            "debug.state",
            "read",
            "The debugger: debug.breakpoints.set, debug.breakpoints.clear, debug.pause, \
             debug.continue, debug.step, debug.state, debug.eval, debug.watch, debug.rewind.",
            any,
        ),
    ]
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
    /// The catalog: the runtime's commands (fetched once) and the server's.
    pub async fn catalog(&self) -> Result<Value, Problem> {
        if let Some(c) = self.0.catalog.get() {
            return Ok(c.clone());
        }
        let mut all = match self.game(&Via::Api, "catalog.list", json!({})).await? {
            Value::Array(a) => a,
            _ => Vec::new(),
        };
        all.extend(server_catalog());
        all.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        let c = Value::Array(all);
        let _ = self.0.catalog.set(c.clone());
        Ok(c)
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
                    let tc = typecheck::run(&self.0.project).await;
                    let r = self.game(via, method, params).await;
                    merge_typecheck(r, tc)
                }
                Err(e) => Err(e),
            },
            "scripts.check" => match decode::<NoParams>(&params, method) {
                Ok(_) => {
                    let tc = typecheck::run(&self.0.project).await;
                    let r = self
                        .game(via, "scripts.apply", json!({"dry_run": true}))
                        .await;
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
                    Some(h) => h.call(m, params).await,
                    None => Err(not_available("debug", m)),
                }
            }
            _ => return None,
        })
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
        if let Ok(Value::Array(comps)) = self.game(&Via::Api, "world.schema", json!({})).await {
            for c in comps {
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
        }
        hits.sort_by_key(|h| std::cmp::Reverse(h.0));
        Ok(Value::Array(
            hits.into_iter().take(limit).map(|(_, v)| v).collect(),
        ))
    }
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
            Ok(v)
        }
        Err(mut p) => {
            p.detail.insert("typecheck".into(), tc["typecheck"].clone());
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
