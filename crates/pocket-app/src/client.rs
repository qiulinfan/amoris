//! The `pocket` CLI as a client of a running host (docs/spec/server.md, CLI): `pocket call <method>
//! '<json>'` for any catalog method, and short forms for the common ones (`pocket world tree`,
//! `pocket step 60 --until event:crate.taken`, `pocket undo`, ...). It finds the host through
//! `--host <url|port>`, `POCKET_HOST`, or the `.pocket/host.json` a host writes in its project (from
//! `--project <dir>` or the working directory up). Output is compact text, one line per entity,
//! event or edit, defaults left out; `--json` prints the exact result. A refusal prints
//! `code: message` (with its suggestions) to stderr and exits 1; a usage error exits 2. Help comes
//! from the catalog's docs and schemas, so it says what the host takes.

use std::fmt::Write as _;
use std::io::Read as _;
use std::path::PathBuf;
use std::time::Duration;

use pocket_contract::{Problem, detail};
use serde_json::{Map, Value, json};

use crate::check::Outcome;
use crate::cli::{Args, Flag, usage};

/// The client's flags (every short form shares them; one a form does not use is ignored).
const FLAGS: &[Flag] = &[
    ("json", false),
    ("help", false),
    ("host", true),
    ("project", true),
    ("ticks", true),
    ("until", true),
    ("watch", true),
    ("speed", true),
    ("paused", false),
    ("force", false),
    ("since", true),
    ("why", true),
    ("name", true),
    ("limit", true),
    ("with", true),
    ("fields", true),
    ("label", true),
    ("components", true),
    ("prefab", true),
];

/// The short forms: command, the method it calls, usage.
pub const COMMANDS: &[(&str, &str, &str)] = &[
    ("call", "", "pocket call <method> ['<json params>'|-]"),
    ("status", "status", "pocket status"),
    ("info", "project.info", "pocket info"),
    (
        "save",
        "project.save",
        "pocket save   (the edit world to scene.json)",
    ),
    (
        "world",
        "world.tree",
        "pocket world tree [filter] [--with C,..] | get <entity> [C..] | query <C,..> [--fields C.f,..] [--name n] [--limit n] | schema [C] | edit '<ops json>' [--label l] | set <entity> <C> f=v.. | spawn [name] [--prefab json] [--components json] | remove <entity> <C> | destroy <entity>..",
    ),
    (
        "step",
        "time.step",
        "pocket step [ticks] [--until event:<name>|tick:<n>|<entity>.<C>.<field><op><value>] [--watch <entity>.<C>.<field>[~value]]",
    ),
    (
        "time",
        "time.control",
        "pocket time pause | resume | speed <x> | stepped",
    ),
    (
        "play",
        "play.start",
        "pocket play start [--speed x] [--paused] | stop",
    ),
    ("undo", "history.undo", "pocket undo"),
    ("redo", "history.redo", "pocket redo"),
    ("history", "history.list", "pocket history"),
    (
        "scripts",
        "scripts.list",
        "pocket scripts list | read <path> | write <path> [<file>|-] | apply [--force] | check",
    ),
    (
        "events",
        "events.since",
        "pocket events [--since seq] [--name n|n.*] [--limit n] | --why <seq>",
    ),
    ("logs", "log.since", "pocket logs [--since seq] [--limit n]"),
    (
        "snapshots",
        "snapshots.list",
        "pocket snapshots [list] | restore <tick>",
    ),
    ("assets", "assets.list", "pocket assets [dir]"),
    ("catalog", "catalog.list", "pocket catalog"),
    (
        "debug",
        "debug.state",
        "pocket debug <action> ['<json params>']   (debug.<action>)",
    ),
    ("help", "", "pocket help [command|method]"),
];

/// Whether `cmd` is one of the client's commands.
pub fn is_client(cmd: &str) -> bool {
    COMMANDS.iter().any(|(c, _, _)| *c == cmd)
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(2)))
        .build()
        .into()
}

fn unreachable(url: &str, why: &str) -> Problem {
    Problem::new(
        "host.unreachable",
        format!("No host answers at {url}: {why}. Start one with `pocket serve <project>`."),
        detail([("url", json!(url))]),
    )
}

/// Calls `method` on the host at `url` (`POST /api/call`).
pub fn call_url(url: &str, method: &str, params: Value) -> Result<Value, Problem> {
    let body = json!({"id": 1, "method": method, "params": params}).to_string();
    let mut resp = agent()
        .post(format!("{url}/api/call"))
        .content_type("application/json")
        .send(&body)
        .map_err(|e| unreachable(url, &e.to_string()))?;
    let text = resp
        .body_mut()
        .with_config()
        .limit(256 << 20)
        .read_to_string()
        .map_err(|e| unreachable(url, &e.to_string()))?;
    let v: Value = serde_json::from_str(&text)
        .map_err(|e| unreachable(url, &format!("the answer is not JSON ({e})")))?;
    if let Some(e) = v.get("error") {
        return Err(serde_json::from_value(e.clone())
            .unwrap_or_else(|_| Problem::new("host.error", e.to_string(), detail([]))));
    }
    Ok(v.get("result").cloned().unwrap_or(Value::Null))
}

fn get_url(url: &str, path: &str) -> Result<Value, Problem> {
    let mut resp = agent()
        .get(format!("{url}{path}"))
        .call()
        .map_err(|e| unreachable(url, &e.to_string()))?;
    let text = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| unreachable(url, &e.to_string()))?;
    serde_json::from_str(&text).map_err(|e| unreachable(url, &e.to_string()))
}

/// The host's URL: `--host`, `POCKET_HOST`, or the project's `.pocket/host.json`.
fn host_url(args: &Args) -> Result<String, Problem> {
    let given = args
        .value("host")
        .map(str::to_owned)
        .or_else(|| std::env::var("POCKET_HOST").ok());
    if let Some(h) = given {
        return Ok(if h.chars().all(|c| c.is_ascii_digit()) {
            format!("http://127.0.0.1:{h}")
        } else if h.contains("://") {
            h.trim_end_matches('/').to_owned()
        } else {
            format!("http://{h}")
        });
    }
    let dir = match args.value("project") {
        Some(p) => PathBuf::from(p),
        None => std::env::current_dir().unwrap_or_default(),
    };
    pocket_server::hostfile::find(&dir)
        .map(|(_, f)| f.url)
        .ok_or_else(|| {
            Problem::new(
                "host.not_found",
                format!(
                    "No running host was found for {} (.pocket/host.json); start one with \
                     `pocket serve <project>`, or give --host or POCKET_HOST.",
                    dir.display()
                ),
                detail([("dir", json!(dir.display().to_string()))]),
            )
        })
}

fn bad(text: impl Into<String>) -> Problem {
    usage(text.into(), "", &[])
}

fn pos(args: &Args, i: usize) -> Option<&str> {
    args.positional.get(i).map(String::as_str)
}

/// A JSON argument, `-` for stdin.
fn json_arg(text: &str) -> Result<Value, Problem> {
    let text = if text == "-" {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .map_err(|e| bad(format!("stdin cannot be read: {e}")))?;
        s
    } else {
        text.to_owned()
    };
    serde_json::from_str(&text).map_err(|e| bad(format!("'{text}' is not JSON: {e}")))
}

/// A value as written on the command line: JSON when it parses, else a string.
fn loose(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| json!(text))
}

/// An entity as written: an id or a name.
fn entity(text: &str) -> Value {
    text.parse::<u64>()
        .map_or_else(|_| json!(text), |n| json!(n))
}

fn list(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `<entity>.<Component>.<field>` with an optional test: `>= <= == != > <`, or `~` (crosses).
fn field_test(spec: &str) -> Result<Value, Problem> {
    const OPS: &[(&str, &str)] = &[
        (">=", ">="),
        ("<=", "<="),
        ("==", "=="),
        ("!=", "!="),
        (">", ">"),
        ("<", "<"),
        ("~", "crosses"),
    ];
    let (path, op, value) = match OPS.iter().find_map(|(t, op)| {
        spec.find(t)
            .map(|i| (&spec[..i], *op, &spec[i + t.len()..]))
    }) {
        Some((p, op, v)) => (p, Some(op), Some(loose(v.trim()))),
        None => (spec, None, None),
    };
    let mut parts = path.trim().splitn(3, '.');
    let (Some(e), Some(c), Some(f)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(bad(format!(
            "'{spec}' is not <entity>.<Component>.<field>[op value] (Sloop.Boat.speed>2)"
        )));
    };
    let mut w = json!({"entity": entity(e), "component": c, "field": f});
    if let Some(op) = op {
        w["op"] = json!(op);
    }
    if let Some(v) = value {
        w["value"] = v;
    }
    Ok(w)
}

fn put(m: &mut Map<String, Value>, key: &str, v: Option<Value>) {
    if let Some(v) = v {
        m.insert(key.to_owned(), v);
    }
}

/// The method and parameters a short form stands for.
fn request(cmd: &str, args: &Args) -> Result<(String, Value), Problem> {
    let mut p = Map::new();
    let num = |name: &str| -> Result<Option<Value>, Problem> {
        Ok(args.number::<u64>(name)?.map(|n| json!(n)))
    };
    let method: &str = match cmd {
        "call" => {
            let m = pos(args, 0).ok_or_else(|| bad("pocket call <method> ['<json>']"))?;
            let params = match pos(args, 1) {
                Some(t) => json_arg(t)?,
                None => json!({}),
            };
            return Ok((m.to_owned(), params));
        }
        "status" | "info" | "save" | "undo" | "redo" | "history" | "catalog" => {
            COMMANDS.iter().find(|c| c.0 == cmd).map_or("", |c| c.1)
        }
        "world" => match pos(args, 0).unwrap_or("tree") {
            "tree" => {
                put(&mut p, "filter", pos(args, 1).map(|f| json!(f)));
                put(&mut p, "with", args.value("with").map(|w| json!(list(w))));
                put(&mut p, "limit", num("limit")?);
                "world.tree"
            }
            "get" => {
                let e = pos(args, 1).ok_or_else(|| bad("pocket world get <entity> [C..]"))?;
                p.insert("entity".into(), entity(e));
                let comps: Vec<&String> = args.positional.iter().skip(2).collect();
                if !comps.is_empty() {
                    p.insert("components".into(), json!(comps));
                } else if let Some(c) = args.value("components") {
                    p.insert("components".into(), json!(list(c)));
                }
                "world.get"
            }
            "query" => {
                let with = pos(args, 1)
                    .map(list)
                    .or_else(|| args.value("with").map(list))
                    .ok_or_else(|| bad("pocket world query <C,..> [--fields C.f,..]"))?;
                p.insert("with".into(), json!(with));
                put(
                    &mut p,
                    "fields",
                    args.value("fields").map(|f| json!(list(f))),
                );
                put(&mut p, "name", args.value("name").map(|n| json!(n)));
                put(&mut p, "limit", num("limit")?);
                "world.query"
            }
            "schema" => {
                put(&mut p, "component", pos(args, 1).map(|c| json!(c)));
                "world.schema"
            }
            "edit" => {
                let ops = json_arg(pos(args, 1).ok_or_else(|| bad("pocket world edit '<ops>'"))?)?;
                match ops {
                    Value::Array(_) => {
                        p.insert("ops".into(), ops);
                    }
                    Value::Object(m) => p = m,
                    _ => return Err(bad("world edit takes an array of ops or {ops, label}")),
                }
                put(&mut p, "label", args.value("label").map(|l| json!(l)));
                "world.edit"
            }
            "set" => {
                let (Some(e), Some(c)) = (pos(args, 1), pos(args, 2)) else {
                    return Err(bad("pocket world set <entity> <Component> field=value.."));
                };
                let mut value = Map::new();
                for kv in args.positional.iter().skip(3) {
                    let (k, v) = kv
                        .split_once('=')
                        .ok_or_else(|| bad(format!("'{kv}' is not field=value")))?;
                    value.insert(k.to_owned(), loose(v));
                }
                p.insert(
                    "ops".into(),
                    json!([{"set": {"entity": entity(e), "component": c, "value": value}}]),
                );
                put(&mut p, "label", args.value("label").map(|l| json!(l)));
                "world.edit"
            }
            "spawn" => {
                let mut s = Map::new();
                put(&mut s, "name", pos(args, 1).map(|n| json!(n)));
                put(
                    &mut s,
                    "prefab",
                    args.value("prefab").map(json_arg).transpose()?,
                );
                put(
                    &mut s,
                    "components",
                    args.value("components").map(json_arg).transpose()?,
                );
                p.insert("ops".into(), json!([{"spawn": s}]));
                put(&mut p, "label", args.value("label").map(|l| json!(l)));
                "world.edit"
            }
            "remove" => {
                let (Some(e), Some(c)) = (pos(args, 1), pos(args, 2)) else {
                    return Err(bad("pocket world remove <entity> <Component>"));
                };
                p.insert(
                    "ops".into(),
                    json!([{"remove": {"entity": entity(e), "component": c}}]),
                );
                "world.edit"
            }
            "destroy" => {
                let ops: Vec<Value> = args
                    .positional
                    .iter()
                    .skip(1)
                    .map(|e| json!({"destroy": {"entity": entity(e)}}))
                    .collect();
                if ops.is_empty() {
                    return Err(bad("pocket world destroy <entity>.."));
                }
                p.insert("ops".into(), json!(ops));
                "world.edit"
            }
            other => {
                return Err(usage(
                    format!("pocket world has no '{other}'"),
                    other,
                    &[
                        "tree", "get", "query", "schema", "edit", "set", "spawn", "remove",
                        "destroy",
                    ],
                ));
            }
        },
        "step" => {
            let ticks = match pos(args, 0) {
                Some(t) => Some(json!(
                    t.parse::<u64>()
                        .map_err(|_| bad(format!("'{t}' is not a number of ticks")))?
                )),
                None => num("ticks")?,
            };
            put(&mut p, "ticks", ticks);
            if let Some(u) = args.value("until") {
                if let Some(ev) = u.strip_prefix("event:") {
                    p.insert("until".into(), json!({"event": ev}));
                } else if let Some(t) = u.strip_prefix("tick:") {
                    let t: u64 = t.parse().map_err(|_| bad(format!("'{t}' is not a tick")))?;
                    p.insert("until".into(), json!({"tick": t}));
                } else {
                    p.insert("watch".into(), field_test(u)?);
                }
            }
            if let Some(w) = args.value("watch") {
                p.insert("watch".into(), field_test(w)?);
            }
            "time.step"
        }
        "time" => match pos(args, 0).unwrap_or("status") {
            "pause" => {
                p.insert("pause".into(), json!(true));
                "time.control"
            }
            "resume" => {
                p.insert("pause".into(), json!(false));
                "time.control"
            }
            "speed" => {
                let x: f64 = pos(args, 1)
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| bad("pocket time speed <x>"))?;
                p.insert("speed".into(), json!(x));
                "time.control"
            }
            "stepped" => {
                p.insert("pacing".into(), json!("stepped"));
                "time.control"
            }
            "status" => "status",
            other => {
                return Err(usage(
                    format!("pocket time has no '{other}'"),
                    other,
                    &["pause", "resume", "speed", "stepped", "status"],
                ));
            }
        },
        "play" => match pos(args, 0).unwrap_or("start") {
            "start" => {
                put(
                    &mut p,
                    "speed",
                    args.number::<f64>("speed")?.map(|s| json!(s)),
                );
                if args.has("paused") {
                    p.insert("paused".into(), json!(true));
                }
                "play.start"
            }
            "stop" => "play.stop",
            other => {
                return Err(usage(
                    format!("pocket play has no '{other}'"),
                    other,
                    &["start", "stop"],
                ));
            }
        },
        "scripts" => match pos(args, 0).unwrap_or("list") {
            "list" => "scripts.list",
            "read" => {
                let path = pos(args, 1).ok_or_else(|| bad("pocket scripts read <path>"))?;
                p.insert("path".into(), json!(path));
                "scripts.read"
            }
            "write" => {
                let path =
                    pos(args, 1).ok_or_else(|| bad("pocket scripts write <path> [file|-]"))?;
                let text = match pos(args, 2) {
                    Some("-") | None => {
                        let mut s = String::new();
                        std::io::stdin()
                            .read_to_string(&mut s)
                            .map_err(|e| bad(format!("stdin cannot be read: {e}")))?;
                        s
                    }
                    Some(f) => std::fs::read_to_string(f)
                        .map_err(|e| bad(format!("{f} cannot be read: {e}")))?,
                };
                p.insert("path".into(), json!(path));
                p.insert("text".into(), json!(text));
                "scripts.write"
            }
            "apply" => {
                if args.has("force") {
                    p.insert("force".into(), json!(true));
                }
                "scripts.apply"
            }
            "check" => "scripts.check",
            other => {
                return Err(usage(
                    format!("pocket scripts has no '{other}'"),
                    other,
                    &["list", "read", "write", "apply", "check"],
                ));
            }
        },
        "events" => {
            if let Some(w) = args.number::<u64>("why")? {
                p.insert("seq".into(), json!(w));
                "events.why"
            } else {
                put(&mut p, "seq", num("since")?);
                put(&mut p, "limit", num("limit")?);
                put(&mut p, "name", args.value("name").map(|n| json!(n)));
                "events.since"
            }
        }
        "logs" => {
            put(&mut p, "seq", num("since")?);
            put(&mut p, "limit", num("limit")?);
            "log.since"
        }
        "snapshots" => match pos(args, 0).unwrap_or("list") {
            "list" => "snapshots.list",
            "restore" => {
                let t: u64 = pos(args, 1)
                    .and_then(|t| t.parse().ok())
                    .ok_or_else(|| bad("pocket snapshots restore <tick>"))?;
                p.insert("tick".into(), json!(t));
                "snapshots.restore"
            }
            other => {
                return Err(usage(
                    format!("pocket snapshots has no '{other}'"),
                    other,
                    &["list", "restore"],
                ));
            }
        },
        "assets" => {
            put(&mut p, "dir", pos(args, 0).map(|d| json!(d)));
            "assets.list"
        }
        "debug" => {
            let action = pos(args, 0).unwrap_or("state");
            let params = match pos(args, 1) {
                Some(t) => json_arg(t)?,
                None => json!({}),
            };
            return Ok((format!("debug.{action}"), params));
        }
        _ => return Err(bad(format!("'{cmd}' is not a client command"))),
    };
    Ok((method.to_owned(), Value::Object(p)))
}

// --- Text forms ----------------------------------------------------------------------------------

fn compact(v: &Value) -> String {
    v.to_string()
}

fn opt(v: &Value) -> String {
    match v {
        Value::Null => "-".into(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn short_hash(v: &Value) -> String {
    v.as_str()
        .map_or_else(|| "-".into(), |s| s.chars().take(12).collect())
}

fn status_line(v: &Value) -> String {
    let pacing = match &v["pacing"] {
        Value::String(s) => s.clone(),
        Value::Object(m) => m.get("real_time").map_or_else(
            || compact(&v["pacing"]),
            |r| format!("real-time x{}", opt(&r["speed"])),
        ),
        other => compact(other),
    };
    let mut s = format!(
        "tick {} ({:.2} s) {} {}{}",
        opt(&v["tick"]),
        v["t_s"].as_f64().unwrap_or(0.0),
        v["mode"].as_str().unwrap_or("edit"),
        if v["paused"] == json!(true) {
            "paused "
        } else {
            "running "
        },
        pacing
    );
    if let Some(n) = v["entities"].as_u64() {
        let _ = write!(s, " | {n} entities");
    }
    let _ = write!(s, " | hash {}", short_hash(&v["world_hash"]));
    if v["halted"] == json!(true) || !v["poisoned"].is_null() {
        let _ = write!(s, " | HALTED (poisoned at {})", opt(&v["poisoned"]));
    }
    if let Some(r) = v.get("restored") {
        s = format!("restored tick {}; {s}", opt(r));
    }
    s
}

fn entity_line(e: &Value) -> String {
    let comps: Vec<&str> = e["components"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    format!(
        "#{} {}  {}",
        opt(&e["id"]),
        opt(&e["name"]),
        comps.join(" ")
    )
}

fn event_line(e: &Value) -> String {
    let seq = if e["seq"].is_null() {
        format!("id{}", opt(&e["id"]))
    } else {
        opt(&e["seq"])
    };
    let mut s = format!(
        "#{} t={} {}",
        seq,
        opt(&e["tick"]),
        e["name"].as_str().unwrap_or("?")
    );
    if !e["subject"].is_null() {
        let _ = write!(s, " subject={}", opt(&e["subject"]));
    }
    if !e["cause"].is_null() {
        let _ = write!(s, " cause=id{}", opt(&e["cause"]));
    }
    if !e["data"].is_null() {
        let _ = write!(s, " {}", compact(&e["data"]));
    }
    s
}

fn diagnostics(v: &Value, out: &mut String) {
    for d in v.as_array().into_iter().flatten() {
        let _ = writeln!(
            out,
            "  {}:{}:{} {} {}",
            opt(&d["file"]),
            opt(&d["line"]),
            opt(&d["column"]),
            opt(&d["code"]),
            opt(&d["message"])
        );
    }
}

fn edit_line(v: &Value) -> String {
    let mut s = format!(
        "{} ({} edit{} at tick {})",
        v["label"].as_str().unwrap_or("edit"),
        opt(&v["applied"]),
        if v["applied"] == json!(1) { "" } else { "s" },
        opt(&v["tick"])
    );
    if let Some(ids) = v["spawned"].as_array()
        && !ids.is_empty()
    {
        let ids: Vec<String> = ids.iter().map(|i| format!("#{i}")).collect();
        let _ = write!(s, "; spawned {}", ids.join(" "));
    }
    s
}

/// The text form of a method's result.
fn text(method: &str, v: &Value) -> String {
    let mut out = String::new();
    let lines = |out: &mut String, items: &Value, f: &dyn Fn(&Value) -> String| {
        for i in items.as_array().into_iter().flatten() {
            let _ = writeln!(out, "{}", f(i));
        }
    };
    match method {
        "status" | "time.control" | "play.start" | "play.stop" | "snapshots.restore" => {
            out = status_line(v) + "\n";
        }
        "world.tree" => lines(&mut out, v, &entity_line),
        "world.get" => {
            let _ = writeln!(out, "#{} {}", opt(&v["id"]), opt(&v["name"]));
            for (c, val) in v["components"].as_object().into_iter().flatten() {
                let _ = writeln!(out, "  {c}: {}", compact(val));
            }
        }
        "world.query" => lines(&mut out, v, &|r| {
            let mut s = format!("#{} {}", opt(&r["id"]), opt(&r["name"]));
            for (k, val) in r.as_object().into_iter().flatten() {
                if k != "id" && k != "name" {
                    let _ = write!(s, "  {k}={}", compact(val));
                }
            }
            s
        }),
        "world.schema" => {
            if v.is_array() {
                lines(&mut out, v, &|c| {
                    format!(
                        "{} ({}) {}",
                        opt(&c["name"]),
                        opt(&c["origin"]),
                        opt(&c["doc"])
                    )
                });
            } else {
                let _ = writeln!(
                    out,
                    "{} ({} v{}) {}",
                    opt(&v["name"]),
                    opt(&v["origin"]),
                    opt(&v["version"]),
                    opt(&v["doc"])
                );
                params(&v["schema"], &mut out);
            }
        }
        "world.edit" | "world_edit" => out = edit_line(v) + "\n",
        "history.undo" => out = format!("undone: {}\n", edit_line(v)),
        "history.redo" => out = format!("redone: {}\n", edit_line(v)),
        "history.list" => {
            let join = |k: &str| -> String {
                v[k].as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(" | ")
                    })
                    .unwrap_or_default()
            };
            let _ = writeln!(out, "undo: {}\nredo: {}", join("undo"), join("redo"));
        }
        "time.step" => {
            let _ = write!(
                out,
                "tick {} hash {}",
                opt(&v["tick"]),
                short_hash(&v["world_hash"])
            );
            let why = &v["stopped_by"];
            match why["reason"].as_str() {
                Some("event") => {
                    let _ = write!(out, " | stopped by {}", event_line(&why["event"]));
                }
                Some("watch") => {
                    let _ = write!(
                        out,
                        " | stopped: {} {} -> {}",
                        opt(&why["field"]),
                        opt(&why["previous"]),
                        opt(&why["value"])
                    );
                }
                Some(r) => {
                    let _ = write!(out, " | stopped: {r}");
                }
                None => {}
            }
            out.push('\n');
            if let Some(errs) = v["errors"].as_array() {
                for e in errs {
                    let _ = writeln!(out, "  error {}: {}", opt(&e["code"]), opt(&e["message"]));
                }
            }
        }
        "snapshots.list" => {
            let ticks: Vec<String> = v["snapshots"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|s| opt(&s["tick"]))
                .collect();
            let _ = writeln!(
                out,
                "kept (every {} ticks): {}",
                opt(&v["every"]),
                ticks.join(" ")
            );
        }
        "events.since" => {
            lines(&mut out, &v["events"], &event_line);
            let _ = writeln!(out, "last {}", opt(&v["last"]));
        }
        "events.why" => {
            let _ = writeln!(out, "{}", event_line(&v["event"]));
            for c in v["causes"].as_array().into_iter().flatten() {
                let _ = writeln!(out, "  <- {}", event_line(c));
            }
            if v["complete"] == json!(false) {
                let _ = writeln!(out, "  <- (older causes left the ring)");
            }
        }
        "log.since" => {
            lines(&mut out, &v["lines"], &|l| {
                let mut s = format!("[{} {}", opt(&l["level"]), opt(&l["source"]));
                if !l["tick"].is_null() {
                    let _ = write!(s, " t={}", opt(&l["tick"]));
                }
                let _ = write!(s, "] {}", opt(&l["message"]));
                if !l["file"].is_null() {
                    let _ = write!(s, " ({}:{})", opt(&l["file"]), opt(&l["line"]));
                }
                s
            });
        }
        "scripts.list" => lines(&mut out, v, &|f| {
            let n = f["diagnostics"].as_array().map_or(0, Vec::len);
            let mut s = format!("{} {} B", opt(&f["path"]), opt(&f["bytes"]));
            if n > 0 {
                let _ = write!(s, " ({n} diagnostics)");
            }
            s
        }),
        "scripts.read" => out = v["text"].as_str().unwrap_or("").to_owned(),
        "scripts.write" | "scripts.apply" | "scripts.check" => {
            let mut head = Vec::new();
            for k in ["path", "outcome", "typecheck"] {
                if !v[k].is_null() {
                    head.push(format!("{k} {}", opt(&v[k])));
                }
            }
            if !v["bundle"].is_null() {
                head.push(format!("bundle {}", short_hash(&v["bundle"])));
            }
            let n = v["diagnostics"].as_array().map_or(0, Vec::len);
            head.push(format!("{n} diagnostics"));
            let _ = writeln!(out, "{}", head.join(" | "));
            diagnostics(&v["diagnostics"], &mut out);
        }
        "assets.list" => lines(&mut out, v, &|a| {
            format!(
                "{} {} {} B",
                opt(&a["path"]),
                opt(&a["kind"]),
                opt(&a["bytes"])
            )
        }),
        "catalog.list" => lines(&mut out, v, &|c| {
            format!(
                "{} ({}) {}",
                opt(&c["name"]),
                opt(&c["kind"]),
                opt(&c["doc"])
            )
        }),
        "project.info" => {
            for (k, val) in v.as_object().into_iter().flatten() {
                let shown = match val {
                    Value::Array(a) => a.iter().map(opt).collect::<Vec<_>>().join(" "),
                    other => opt(other),
                };
                let _ = writeln!(out, "{k}: {shown}");
            }
        }
        _ => {
            out = serde_json::to_string_pretty(v).unwrap_or_default() + "\n";
        }
    }
    out
}

/// A problem as text: `code: message`, and its suggestions when the message does not hold them.
fn problem_text(p: &Problem) -> String {
    let mut s = format!("{}: {}", p.code, p.message);
    if let Some(sug) = p.detail.get("suggestions").and_then(Value::as_array)
        && !sug.is_empty()
        && !p.message.contains("did you mean")
    {
        let names: Vec<String> = sug.iter().map(opt).collect();
        let _ = write!(s, "\n  did you mean: {}", names.join(", "));
    }
    for k in ["diagnostics", "typecheck_diagnostics"] {
        if let Some(d) = p.detail.get(k) {
            s.push('\n');
            diagnostics(d, &mut s);
        }
    }
    s.trim_end().to_owned()
}

// --- Help ----------------------------------------------------------------------------------------

fn resolve<'a>(root: &'a Value, s: &'a Value) -> &'a Value {
    match s.get("$ref").and_then(Value::as_str) {
        Some(r) => r
            .strip_prefix("#/$defs/")
            .and_then(|n| root.get("$defs").and_then(|d| d.get(n)))
            .unwrap_or(s),
        None => s,
    }
}

fn type_of(root: &Value, s: &Value) -> String {
    let s = resolve(root, s);
    if s.get("type") == Some(&json!("array"))
        && let Some(items) = s.get("items")
    {
        return format!("[{}]", type_of(root, items));
    }
    if let Some(t) = s.get("type") {
        return match t {
            Value::Array(a) => a
                .iter()
                .map(opt)
                .filter(|t| t != "null")
                .collect::<Vec<_>>()
                .join("|"),
            other => opt(other),
        };
    }
    if let Some(e) = s.get("enum").and_then(Value::as_array) {
        return e.iter().map(opt).collect::<Vec<_>>().join("|");
    }
    for k in ["anyOf", "oneOf"] {
        if let Some(alts) = s.get(k).and_then(Value::as_array) {
            let ts: Vec<String> = alts
                .iter()
                .map(|a| {
                    let a = resolve(root, a);
                    if let Some(c) = a.get("const") {
                        return opt(c);
                    }
                    if let Some(e) = a.get("enum").and_then(Value::as_array) {
                        return e.iter().map(opt).collect::<Vec<_>>().join("|");
                    }
                    if let Some(p) = a.get("properties").and_then(Value::as_object)
                        && p.len() == 1
                    {
                        return format!("{{{}}}", p.keys().next().map_or("", String::as_str));
                    }
                    type_of(root, a)
                })
                .filter(|t| t != "null")
                .collect();
            return ts.join("|");
        }
    }
    "any".into()
}

/// A params schema's properties as lines.
fn params(schema: &Value, out: &mut String) {
    props(schema, schema, "  ", out);
}

/// The object schema `s`'s properties as lines, objects with properties one level deeper.
fn props(root: &Value, s: &Value, indent: &str, out: &mut String) {
    let schema = s;
    let required: Vec<&str> = schema["required"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let Some(fields) = schema["properties"].as_object() else {
        let _ = writeln!(out, "  (no parameters)");
        return;
    };
    for (name, s) in fields {
        let r = resolve(root, s);
        let doc = s
            .get("description")
            .or_else(|| r.get("description"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let opt_mark = if required.contains(&name.as_str()) {
            ""
        } else {
            "?"
        };
        let _ = writeln!(out, "{indent}{name}{opt_mark}: {}  {doc}", type_of(root, s));
        let inner = match r.get("anyOf").and_then(Value::as_array) {
            Some(alts) => alts
                .iter()
                .map(|a| resolve(root, a))
                .find(|a| a.get("properties").is_some())
                .unwrap_or(r),
            None => r,
        };
        if inner.get("properties").is_some() && indent.len() < 6 {
            props(root, inner, &format!("{indent}  "), out);
        }
    }
}

fn catalog(args: &Args) -> Value {
    if let Ok(url) = host_url(args)
        && let Ok(c) = get_url(&url, "/api/catalog")
    {
        return c;
    }
    let mut all = match pocket_runtime::catalog_json(&[]) {
        Value::Array(a) => a,
        _ => Vec::new(),
    };
    all.extend(pocket_server::server_catalog());
    Value::Array(all)
}

fn help(args: &Args) -> String {
    let mut out = String::new();
    let topic = pos(args, 0)
        .or_else(|| args.has("help").then_some(""))
        .unwrap_or("");
    if topic.is_empty() {
        out.push_str(
            "pocket: a client of a running host (pocket serve <project>), found through \
             --host, POCKET_HOST or .pocket/host.json. --json prints exact JSON.\n\n",
        );
        for (_, _, u) in COMMANDS {
            let _ = writeln!(out, "  {u}");
        }
        out.push_str("\nHost commands: pocket serve <project> [--port 7878], pocket mcp <project>; \
                      also pocket run|check|replay|hashes.\nMethods (pocket call <method> '<json>'; \
                      pocket help <method>):\n");
        for c in catalog(args).as_array().into_iter().flatten() {
            let _ = writeln!(out, "  {:<18} {}", opt(&c["name"]), opt(&c["doc"]));
        }
        return out;
    }
    let method = COMMANDS
        .iter()
        .find(|c| c.0 == topic)
        .map(|c| {
            let _ = writeln!(out, "usage: {}", c.2);
            c.1
        })
        .filter(|m| !m.is_empty())
        .unwrap_or(topic);
    let cat = catalog(args);
    let found = cat.as_array().into_iter().flatten().find(|c| {
        c["name"] == json!(method)
            || c["aliases"]
                .as_array()
                .is_some_and(|a| a.contains(&json!(method)))
    });
    match found {
        Some(c) => {
            let _ = writeln!(
                out,
                "{} ({}): {}",
                opt(&c["name"]),
                opt(&c["kind"]),
                opt(&c["doc"])
            );
            params(&c["params"], &mut out);
        }
        None if out.is_empty() => {
            let names: Vec<&str> = cat
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| c["name"].as_str())
                .chain(COMMANDS.iter().map(|c| c.0))
                .collect();
            let s = pocket_contract::suggest_names(topic, names.iter().copied());
            let _ = writeln!(out, "no command or method '{topic}'; did you mean {s:?}?");
        }
        None => {}
    }
    out
}

/// Runs a client command.
pub fn run(cmd: &str, raw: &[String]) -> Outcome {
    let args = match Args::parse(raw, FLAGS) {
        Ok(a) => a,
        Err(p) => {
            eprintln!("{}", problem_text(&p));
            return Outcome {
                stdout: String::new(),
                code: 2,
            };
        }
    };
    if cmd == "help" || args.has("help") {
        let mut a = args;
        if cmd != "help" {
            a.positional.insert(0, cmd.to_owned());
        }
        print!("{}", help(&a));
        return Outcome {
            stdout: String::new(),
            code: 0,
        };
    }
    let json_out = args.has("json");
    let failed = |p: &Problem, code: i32| {
        if json_out {
            println!("{}", json!({"error": p}));
        } else {
            eprintln!("{}", problem_text(p));
        }
        Outcome {
            stdout: String::new(),
            code,
        }
    };
    let (method, params) = match request(cmd, &args) {
        Ok(r) => r,
        Err(p) => return failed(&p, 2),
    };
    let url = match host_url(&args) {
        Ok(u) => u,
        Err(p) => return failed(&p, 1),
    };
    match call_url(&url, &method, params) {
        Ok(v) => {
            if json_out {
                println!("{v}");
            } else {
                print!("{}", text(&method, &v));
            }
            Outcome {
                stdout: String::new(),
                code: 0,
            }
        }
        Err(p) => failed(&p, 1),
    }
}
