//! Methods the server answers itself (docs/spec/server.md, methods): the merged catalog, the event,
//! log and profile streams `amoris-link` keeps for presenters, the project's asset files (listed,
//! and uploaded in parts by `assets.upload`), the type check stage of `scripts.apply` and
//! `scripts.check`, the script guide (`scripts.guide`), the agent sessions (`agent.session.*`,
//! `sessions.rs`), and the plug-ins (`debug.*`, `capture`), which answer `*.not_available` until
//! the integrator installs them.

use std::path::{Component, Path};

use amoris_contract::{CheckOptions, Problem, detail};
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

/// `profile.frame`'s parameters.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ProfileFrame {
    /// The last this many ticks, default 60 (the ring keeps 600).
    #[serde(default)]
    ticks: Option<u32>,
}

/// `assets.list`'s parameters.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AssetsList {
    /// A directory inside the project; default: the whole project.
    #[serde(default)]
    dir: Option<String>,
}

/// `assets.upload`'s parameters: a file sent into the project, in parts (server.md 3).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AssetsUpload {
    /// Where the file goes: a path inside the project, such as `models/lighthouse.glb`.
    path: String,
    /// This part's bytes, base64 (the standard alphabet, padded). At most 1 MiB of bytes a part
    /// keeps a call under the HTTP API's 2 MB body.
    data: String,
    /// Where this part starts in the file: 0 begins the upload (again), and each later part starts
    /// where the parts before it ended.
    #[serde(default)]
    offset: u64,
    /// Whether this is the last part (default true): the file is then moved into place.
    #[serde(default = "yes")]
    done: bool,
    /// Whether a file already at `path` is replaced (default false: refused with `assets.exists`).
    #[serde(default)]
    overwrite: bool,
}

fn yes() -> bool {
    true
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

/// `capture`'s parameters, which the installed `CaptureHub` reads (amoris-app's `present.rs`); the
/// server passes them on as they came.
#[derive(Deserialize, JsonSchema)]
#[allow(dead_code)]
struct Capture {
    /// The camera's position `[x, y, z]`; without it, the scene's camera.
    #[serde(default)]
    position: Option<[f64; 3]>,
    /// The point the camera looks at `[x, y, z]`; default: the origin.
    #[serde(default)]
    look_at: Option<[f64; 3]>,
    /// The camera's rotation as a quaternion `[x, y, z, w]`, when there is no `look_at`.
    #[serde(default)]
    rotation: Option<[f64; 4]>,
    /// The vertical field of view in degrees, default 60.
    #[serde(default)]
    fov_deg: Option<f64>,
    /// The image's width in pixels, default 1280 (16 to 4096).
    #[serde(default)]
    width: Option<u32>,
    /// The image's height in pixels, default 720 (16 to 4096).
    #[serde(default)]
    height: Option<u32>,
    /// First draw this many frames at 60 a second (at most 1200), so spray and foam gather.
    #[serde(default)]
    frames: Option<u32>,
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
            "profile.frame",
            "read",
            "What the last ticks cost the game thread (default 60), in ms per tick: each system's \
             own time in the order they ran (a script system's out of script.update), the step's \
             time outside its systems (tick.other), the render feed's extraction (loop.present) \
             and the snapshot and streams published after it (loop.publish), and each phase; \
             ticks the debugger held are counted in held and left out.",
            schema::<ProfileFrame>(),
        ),
        e(
            "assets.list",
            "read",
            "The project's files with their kind and size (served at /assets/<path>).",
            schema::<AssetsList>(),
        ),
        e(
            "assets.upload",
            "request",
            "Writes a file into the project (an asset from the editor's browser or an agent): its \
             bytes in base64 parts {path, data, offset, done}, offset 0 first, each part at most \
             1 MiB; the last part (done, the default) moves the file into place and answers \
             {path, kind, bytes}. A file already at path is replaced only with overwrite.",
            schema::<AssetsUpload>(),
        ),
        e(
            "scripts.check",
            "read",
            "Compiles and loads the scripts without swapping, writes their declarations under .amoris/types and type checks them with tsc (TypeScript 7) when installed.",
            schema::<NoParams>(),
        ),
        e(
            "scripts.guide",
            "read",
            "How game scripts are written (docs/sdk.md): the amoris SDK, one example per concept, common mistakes and their messages.",
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
            "Renders the running world to a PNG under .amoris/captures/ from a camera {position, \
             look_at or rotation, fov_deg; width, height; frames: first draw that many frames of \
             the running game at 60 a second, so spray and foam gather}; without position, the \
             scene's camera. The reply names the image, its size and tick, and each water \
             region's spray: drops born in the last frame and alive.",
            schema::<Capture>(),
        ),
        e(
            "agent.session.list",
            "read",
            "The host's agent sessions in opening order, {sessions: [{id, transport, role, seat, \
             label, calls, failed_calls, last_method, live, opened_ts, last_ts}], last}: the MCP \
             clients on /mcp and stdio, the bridges that registered themselves, and the calls \
             made as a seat with no session.",
            schema::<NoParams>(),
        ),
        e(
            "agent.session.events",
            "read",
            "What the agent sessions called, {seq, ts, session, kind: opened|call|result|closed, \
             ...}: the newest (the host keeps 2,000), or with since the first limit after it; \
             last is the cursor to ask again with, missed counts what left the ring.",
            schema::<crate::sessions::EventsParams>(),
        ),
        e(
            "agent.session.open",
            "control",
            "Registers the caller as an agent session with a developer client of its own (with \
             seat: that seat's player) and answers its row and a handle; its calls are POST \
             /api/call {handle, method, params}, listed under its id and named in the history.",
            schema::<crate::sessions::OpenParams>(),
        ),
        e(
            "agent.session.close",
            "control",
            "Closes a registered session: by the handle its opener holds, or a bridge session by \
             its id (session) when its process is gone.",
            schema::<crate::sessions::CloseParams>(),
        ),
        e(
            "agent.session.agents",
            "read",
            "The agent programs this host may start, {agents: [{name, label}], running, limit}: \
             registered by the process owner with amoris serve --agents <file>.",
            schema::<NoParams>(),
        ),
        e(
            "agent.session.start",
            "control",
            "Starts a registered agent program as an agent session of this host (transport \
             launch) with a goal, and with seat as that seat's player; its output lines are the \
             session's message events and the session closes when it exits.",
            schema::<crate::launch::StartParams>(),
        ),
        e(
            "agent.session.stop",
            "control",
            "Ends a started agent program; answers its session's row once closed.",
            schema::<crate::launch::StopParams>(),
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

/// `debug.writes`'s parameters: the host's own debugger method (it steps the game with a write
/// trace, docs/spec/debugger.md 13).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WritesParams {
    /// The entity: its id, a name or `Name#id`.
    pub entity: EntityParam,
    /// The component, such as `Boat`.
    pub component: String,
    /// A field or a path into it (`rudder`, `position.y`); the whole component when absent.
    #[serde(default)]
    pub field: Option<String>,
    /// Ticks to step under the trace (default 60).
    #[serde(default)]
    pub ticks: Option<u64>,
}

/// An entity as the host's debugger methods take it: an id, or a name (`Sloop`, `Sloop#3`).
#[derive(Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum EntityParam {
    Id(u64),
    Name(String),
}

impl EntityParam {
    fn json(&self) -> Value {
        match self {
            EntityParam::Id(n) => json!(n),
            EntityParam::Name(s) => json!(s),
        }
    }
}

/// The ticks `debug.writes` steps when `ticks` is not given.
pub const DEFAULT_WRITES_TICKS: u64 = 60;

/// The catalog entries of the debugger installed as `hub` (its methods, and the host's
/// `debug.rewind` and `debug.writes`), or a placeholder that says no debugger is installed.
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
    out.push(entry(&json!({
        "name": "debug.writes",
        "kind": "control",
        "doc": "Who changed a field: steps ticks (default 60) and names, tick by tick, every \
                write that changed entity.component.field, without pausing: the boundary's \
                recorded writes (world.edit, player.act controls and intents) and each system, \
                engine or script, with its phase, the values before and after and, for a \
                script, the statement that wrote. The world hash is the same as without it.",
        "params": schema::<WritesParams>(),
    })));
    out
}

fn decode<T: DeserializeOwned + JsonSchema>(v: &Value, owner: &str) -> Result<T, Problem> {
    amoris_contract::decode::<T>(v, &CheckOptions::new(owner)).map(|d| d.value)
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

/// The most bytes one `assets.upload` part holds (1 MiB): its base64 keeps a call under the HTTP
/// API's 2 MB body (axum's default) and a WebSocket message well under its 16 MB frame.
pub const UPLOAD_PART: usize = 1 << 20;

/// Decodes base64 (RFC 4648's standard alphabet, padded). Written here rather than taken from a
/// crate: a new dependency changes `Cargo.lock`, which the resident water's kernel key hashes, and
/// with it every committed viewport build (AGENTS.md, the browser).
fn base64(text: &str) -> Result<Vec<u8>, String> {
    let digit = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let t = text.as_bytes();
    if !t.len().is_multiple_of(4) {
        return Err(format!("its length {} is not a multiple of 4", t.len()));
    }
    let mut out = Vec::with_capacity(t.len() / 4 * 3);
    for (i, quad) in t.chunks(4).enumerate() {
        let last = (i + 1) * 4 == t.len();
        let pad = quad.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return Err(format!("'=' inside the text, at {}", i * 4));
        }
        let mut n = 0u32;
        for (k, &c) in quad[..4 - pad].iter().enumerate() {
            let d = digit(c)
                .ok_or_else(|| format!("'{}' at {} is not a base64 digit", c as char, i * 4 + k))?;
            n |= d << (18 - 6 * k);
        }
        let b = n.to_be_bytes();
        out.extend_from_slice(&b[1..4 - pad]);
    }
    Ok(out)
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
            self.push_history(h);
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

    /// `debug.writes {entity, component, field?, ticks?}`: a `time.step` of `ticks` under a write
    /// trace of the field; the step's answer with the trace's (`writes`) in place of its own.
    async fn writes(&self, via: &Via, params: Value) -> Result<Value, Problem> {
        let p: WritesParams = decode(&params, "debug.writes")?;
        let mut trace = json!({"entity": p.entity.json(), "component": p.component});
        if let Some(f) = &p.field {
            trace["field"] = json!(f);
        }
        let ticks = p.ticks.unwrap_or(DEFAULT_WRITES_TICKS);
        let stepped = self
            .game(via, "time.step", json!({"ticks": ticks, "trace": trace}))
            .await
            .map_err(|mut e| {
                // The step's refusals point into its `trace`; this method takes those at the top.
                if let Some(Value::String(path)) = e.detail.get_mut("path")
                    && let Some(rest) = path.strip_prefix("/trace/")
                {
                    *path = format!("/{rest}");
                }
                e
            })?;
        let mut out = match stepped.get("writes") {
            Some(Value::Object(w)) => Value::Object(w.clone()),
            // A debugger stop answered the step inside a tick: the trace was dropped there.
            _ => json!({"dropped": "debugger stop"}),
        };
        for k in ["tick", "world_hash", "errors", "stopped_by", "paused"] {
            if let Some(v) = stepped.get(k).filter(|v| !v.is_null()) {
                out[k] = v.clone();
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
                amoris_link::events_since(reader, p.seq, usize_of(p.limit, 100), p.name.as_deref())
            }),
            "events.why" => decode::<EventsWhy>(&params, method)
                .and_then(|p| amoris_link::events_why(reader, p.seq)),
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
            "profile.frame" => decode::<ProfileFrame>(&params, method).map(|p| {
                let n = usize_of(p.ticks, 60).clamp(1, amoris_link::PROFILE_TICKS);
                crate::push::profile_of(self, reader.profile_last(n))
            }),
            "assets.list" => decode::<AssetsList>(&params, method).and_then(|p| self.assets(p)),
            "assets.upload" => decode::<AssetsUpload>(&params, method).and_then(|p| self.upload(p)),
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
            // The host's own state: answered without the game, also while the debugger holds it.
            "agent.session.list"
            | "agent.session.events"
            | "agent.session.open"
            | "agent.session.close" => self.agent_session(method, &params),
            "agent.session.agents" => decode::<NoParams>(&params, method).map(|_| self.agents()),
            "agent.session.start" => self.start_agent(&params).await,
            "agent.session.stop" => self.stop_agent(&params).await,
            m if m.starts_with("debug.") => {
                let hub = self.0.debug.read().ok().and_then(|h| h.clone());
                match hub {
                    Some(_) if m == "debug.rewind" => self.rewind(via, params).await,
                    Some(_) if m == "debug.writes" => self.writes(via, params).await,
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

    /// `assets.upload`: appends a part to the upload's staging file under `.amoris/uploads/` (a dot
    /// directory, so neither listed nor served), and on the last part moves it to `path`.
    fn upload(&self, p: AssetsUpload) -> Result<Value, Problem> {
        use std::io::Write as _;
        let invalid = |field: &str, message: String| {
            Problem::new(
                "request.invalid_value",
                message,
                detail([("path", json!(format!("/{field}")))]),
            )
        };
        let failed = |what: &str, e: std::io::Error| {
            Problem::new(
                "assets.write_failed",
                format!("Could not {what} for '{}': {e}.", p.path),
                detail([("path", json!(p.path))]),
            )
        };
        if !inside(&p.path) || p.path.ends_with('/') {
            return Err(invalid(
                "path",
                format!("'{}' is not a file path inside the project.", p.path),
            ));
        }
        let bytes = base64(&p.data)
            .map_err(|e| invalid("data", format!("The data is not base64: {e}.")))?;
        if bytes.len() > UPLOAD_PART {
            return Err(invalid(
                "data",
                format!(
                    "A part holds at most {UPLOAD_PART} bytes; this one holds {}.",
                    bytes.len()
                ),
            ));
        }
        let root = &self.0.project;
        let dest = root.join(&p.path);
        let exists = || {
            Problem::new(
                "assets.exists",
                format!(
                    "The project already has '{}'; upload with overwrite to replace it.",
                    p.path
                ),
                detail([("path", json!(p.path))]),
            )
        };
        if !p.overwrite && dest.exists() {
            return Err(exists());
        }
        let staging = root.join(".amoris/uploads");
        std::fs::create_dir_all(&staging).map_err(|e| failed("make the upload directory", e))?;
        let part = staging.join(format!(
            "{}.part",
            p.path.replace('%', "%25").replace('/', "%2F")
        ));
        let had = if p.offset == 0 {
            0
        } else {
            std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0)
        };
        if had != p.offset {
            return Err(Problem::new(
                "assets.upload_offset",
                format!(
                    "This part of '{}' starts at {}, but the parts received end at {had}:                      upload from offset {had}, or from 0 again.",
                    p.path, p.offset
                ),
                detail([
                    ("path", json!(p.path)),
                    ("expected", json!(had)),
                    ("got", json!(p.offset)),
                ]),
            ));
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(p.offset != 0)
            .truncate(p.offset == 0)
            .open(&part)
            .map_err(|e| failed("open the upload", e))?;
        f.write_all(&bytes)
            .map_err(|e| failed("write the upload", e))?;
        drop(f);
        let received = p.offset + bytes.len() as u64;
        if !p.done {
            return Ok(json!({"path": p.path, "received": received}));
        }
        let parent = dest.parent().unwrap_or(root);
        std::fs::create_dir_all(parent).map_err(|e| failed("make the directory", e))?;
        // The directory, links resolved, must still be inside the project.
        let inside_root = match (parent.canonicalize(), root.canonicalize()) {
            (Ok(d), Ok(r)) => d.starts_with(r),
            _ => false,
        };
        if !inside_root {
            let _ = std::fs::remove_file(&part);
            return Err(invalid(
                "path",
                format!("'{}' leads out of the project through a link.", p.path),
            ));
        }
        if !p.overwrite && dest.exists() {
            let _ = std::fs::remove_file(&part);
            return Err(exists());
        }
        std::fs::rename(&part, &dest).map_err(|e| failed("move the upload into place", e))?;
        Ok(json!({"path": p.path, "kind": kind_of(&p.path), "bytes": received}))
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

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_decodes_the_standard_alphabet_and_refuses_the_rest() {
        assert_eq!(base64("").unwrap(), b"");
        assert_eq!(base64("Zg==").unwrap(), b"f");
        assert_eq!(base64("Zm8=").unwrap(), b"fo");
        assert_eq!(base64("Zm9v").unwrap(), b"foo");
        assert_eq!(base64("Zm9vYmFy").unwrap(), b"foobar");
        assert_eq!(base64("AP8/+w==").unwrap(), [0x00, 0xff, 0x3f, 0xfb]);
        assert!(base64("Zm9").is_err());
        assert!(base64("Zg==Zm9v").is_err());
        assert!(base64("Zm9-").is_err());
        assert!(base64("Z===").is_err());
    }
}
