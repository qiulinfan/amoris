//! The `pocket` CLI as a client of a running host (docs/spec/server.md, CLI): `pocket call <method>
//! '<json>'` for any catalog method, and short forms for the common ones (`pocket world tree`,
//! `pocket step 60 --until event:crate.taken`, `pocket undo`, ...). It finds the host through
//! `--host <url|port>`, `POCKET_HOST`, or the `.pocket/host.json` a host writes in its project (from
//! `--project <dir>` or the working directory up). Output is compact text, one line per entity,
//! event or edit, defaults left out; `--json` prints the exact result. A refusal prints
//! `code: message` (with its suggestions) to stderr and exits 1, as does a `scripts check` that finds
//! an error; a usage error exits 2. Help comes from the catalog's docs and schemas, so it says what
//! the host takes.

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
    ("timeout", true),
    ("if", true),
    ("log", true),
    ("frame", true),
    ("full", false),
    ("bundle", true),
    ("lines", true),
    ("numbers", false),
    ("sample", true),
    ("every", true),
    ("seat", true),
];

/// How long the CLI waits for an answer unless `--timeout <s>` or `POCKET_TIMEOUT` says otherwise
/// (a debugger's `timeout_ms` waits longer): nothing a held host refuses at once can hang an
/// agent's shell (docs/bench/debug-eval.md, the first finding).
pub const DEFAULT_TIMEOUT_S: f64 = 60.0;

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
        "pocket world tree [filter] [--with C,..] | get <entity> [C.. | C.f,C.g..] | query <C,..> [--fields C.f,..] [--name n] [--limit n] | schema [C] | edit '<ops json>' [--label l] | set <entity> <C> f=v.. | spawn [name] [--prefab json] [--components json] | remove <entity> <C> | destroy <entity>..",
    ),
    (
        "step",
        "time.step",
        "pocket step [ticks] [--until event:<name>|tick:<n>|<entity>.<C>.<field><op><value>] [--watch <entity>.<C>.<field>[~value]] [--sample <entity>.<C>.<field>,.. [--every k]]",
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
        "pocket scripts list | read <path> [--lines a-b] [--numbers] | write <path> [<file>|-] | apply [--force] | check | types | guide | status",
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
        "pocket snapshots [list] | restore <tick> [--bundle snapshot]",
    ),
    ("assets", "assets.list", "pocket assets [dir]"),
    ("catalog", "catalog.list", "pocket catalog"),
    ("docs", "docs.search", "pocket docs <words..> [--limit n]"),
    (
        "debug",
        "debug.state",
        "pocket debug state [--full] | break <file>:<line> [--if <expr>] [--log <text>] | clear [id] | list | eval <expr> [--frame n] | set <name> <expr> [--frame n] | watch <entity>.<C>[.<field>] | unwatch [id] | continue | step [over|into|out] | pause | wait [ms] | exceptions none|uncaught|all | rewind <tick> [--bundle snapshot] | attach | detach | <action> '<json params>'",
    ),
    (
        "player",
        "player.observe",
        "pocket player observe | describe | act | wait | nearby | events | affordances | intents | continue | session | pacing ['<json params>'] [--seat <seat>] [--ticks n]   (--seat plays as that seat; act also takes '[actions]')",
    ),
    ("help", "", "pocket help [command|method]"),
];

/// Whether `cmd` is one of the client's commands.
pub fn is_client(cmd: &str) -> bool {
    COMMANDS.iter().any(|(c, _, _)| *c == cmd)
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(2)))
        .timeout_global(Some(timeout))
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

/// What a request that ran out of time says.
fn timed_out(url: &str, method: &str, waited: Duration) -> Problem {
    Problem::new(
        "host.timeout",
        format!(
            "The host at {url} did not answer {method} within {:.0} s. It may still be running it \
             (a long step or type check); `pocket status` and `pocket debug state` answer \
             meanwhile. --timeout <s> or POCKET_TIMEOUT waits longer.",
            waited.as_secs_f64()
        ),
        detail([
            ("url", json!(url)),
            ("method", json!(method)),
            ("waited_s", json!(waited.as_secs_f64())),
        ]),
    )
}

/// How long to wait for `method`: `given` seconds (`--timeout`), else `POCKET_TIMEOUT`, else
/// [`DEFAULT_TIMEOUT_S`]; at least 10 s past a `timeout_ms` the call itself asks to wait.
pub fn timeout_for(given: Option<f64>, params: &Value) -> Duration {
    let base = given
        .or_else(|| {
            std::env::var("POCKET_TIMEOUT")
                .ok()
                .and_then(|t| t.parse().ok())
        })
        .filter(|t: &f64| t.is_finite() && *t > 0.0)
        .unwrap_or(DEFAULT_TIMEOUT_S);
    let asked = params["timeout_ms"]
        .as_f64()
        .map_or(0.0, |ms| ms / 1000.0 + 10.0);
    Duration::from_secs_f64(base.max(asked))
}

/// Calls `method` on the host at `url` (`POST /api/call`), waiting as [`timeout_for`] says.
pub fn call_url(url: &str, method: &str, params: Value) -> Result<Value, Problem> {
    let timeout = timeout_for(None, &params);
    call_url_within(url, method, params, timeout)
}

/// [`call_url`] waiting at most `timeout` for the answer: `host.timeout` after it.
pub fn call_url_within(
    url: &str,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<Value, Problem> {
    call_url_as(url, method, params, None, timeout)
}

/// [`call_url_within`] made as `seat`'s player when one is given (docs/spec/player.md): the host
/// sends it from the seat's player source, which the runtime restricts to the seat's perception.
pub fn call_url_as(
    url: &str,
    method: &str,
    params: Value,
    seat: Option<&str>,
    timeout: Duration,
) -> Result<Value, Problem> {
    let mut body = json!({"id": 1, "method": method, "params": params});
    if let Some(s) = seat {
        body["seat"] = json!(s);
    }
    let body = body.to_string();
    let failed = |e: ureq::Error| match e {
        ureq::Error::Timeout(_) => timed_out(url, method, timeout),
        e => unreachable(url, &e.to_string()),
    };
    let mut resp = agent(timeout)
        .post(format!("{url}/api/call"))
        .content_type("application/json")
        .send(&body)
        .map_err(failed)?;
    let text = resp
        .body_mut()
        .with_config()
        .limit(256 << 20)
        .read_to_string()
        .map_err(failed)?;
    let v: Value = serde_json::from_str(&text)
        .map_err(|e| unreachable(url, &format!("the answer is not JSON ({e})")))?;
    if let Some(e) = v.get("error") {
        return Err(serde_json::from_value(e.clone())
            .unwrap_or_else(|_| Problem::new("host.error", e.to_string(), detail([]))));
    }
    Ok(v.get("result").cloned().unwrap_or(Value::Null))
}

fn get_url(url: &str, path: &str) -> Result<Value, Problem> {
    let mut resp = agent(Duration::from_secs(10))
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
                let e = pos(args, 1)
                    .ok_or_else(|| bad("pocket world get <entity> [C.. | C.f,C.g..]"))?;
                p.insert("entity".into(), entity(e));
                // Components and fields alike, space or comma separated: `Boat Transform`,
                // `Boat.heading_deg,Boat.rudder`.
                let mut wanted: Vec<String> = args
                    .positional
                    .iter()
                    .skip(2)
                    .flat_map(|a| list(a))
                    .collect();
                if wanted.is_empty()
                    && let Some(c) = args.value("components")
                {
                    wanted = list(c);
                }
                let (fields, comps): (Vec<String>, Vec<String>) =
                    wanted.into_iter().partition(|w| w.contains('.'));
                if !comps.is_empty() {
                    p.insert("components".into(), json!(comps));
                }
                if let Some(f) = args.value("fields") {
                    let mut all = fields;
                    all.extend(list(f));
                    p.insert("fields".into(), json!(all));
                } else if !fields.is_empty() {
                    p.insert("fields".into(), json!(fields));
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
            if let Some(s) = args.value("sample") {
                let mut sample = json!({"fields": list(s)});
                if let Some(every) = args.number::<u64>("every")? {
                    sample["every"] = json!(every);
                }
                p.insert("sample".into(), sample);
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
                let path = pos(args, 1)
                    .ok_or_else(|| bad("pocket scripts read <path> [--lines 50-80] [--numbers]"))?;
                p.insert("path".into(), json!(path));
                // Line numbers whenever lines are picked: what breakpoints and errors count.
                if let Some(l) = args.value("lines") {
                    p.insert("lines".into(), json!(l));
                    p.insert("numbered".into(), json!(true));
                }
                if args.has("numbers") {
                    p.insert("numbered".into(), json!(true));
                }
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
            "types" => "scripts.types",
            "guide" => "scripts.guide",
            "status" => "scripts.status",
            other => {
                return Err(usage(
                    format!("pocket scripts has no '{other}'"),
                    other,
                    &[
                        "list", "read", "write", "apply", "check", "types", "guide", "status",
                    ],
                ));
            }
        },
        "docs" => {
            let words: Vec<&str> = args.positional.iter().map(String::as_str).collect();
            if words.is_empty() {
                return Err(bad("pocket docs <words..>"));
            }
            p.insert("query".into(), json!(words.join(" ")));
            put(&mut p, "limit", num("limit")?);
            "docs.search"
        }
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
                    .ok_or_else(|| bad("pocket snapshots restore <tick> [--bundle snapshot]"))?;
                p.insert("tick".into(), json!(t));
                put(&mut p, "bundle", args.value("bundle").map(|b| json!(b)));
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
        "debug" => return debug_request(args),
        "player" => return player_request(args),
        _ => return Err(bad(format!("'{cmd}' is not a client command"))),
    };
    Ok((method.to_owned(), Value::Object(p)))
}

/// The `player` tool's actions (`pocket-mcp`'s, the `player.*` catalog methods).
const PLAYER_ACTIONS: [&str; 11] = pocket_mcp::tools::PLAYER_ACTIONS;

/// `pocket player <action> ['<json params>'] [--seat s]` (docs/spec/player.md): a `player.*`
/// method. Observations, nearby percepts, events and a described entity come as their text
/// projection unless `--json` or the parameters ask otherwise; `act` also takes the actions alone
/// (`pocket player act '[{"do": "start", ...}]'`).
fn player_request(args: &Args) -> Result<(String, Value), Problem> {
    let action = pos(args, 0).unwrap_or("observe");
    if !PLAYER_ACTIONS.contains(&action) {
        return Err(usage(
            format!("pocket player has no '{action}'"),
            action,
            &PLAYER_ACTIONS,
        ));
    }
    let mut p = match pos(args, 1) {
        Some(text) => match json_arg(text)? {
            Value::Object(m) => m,
            Value::Array(actions) if action == "act" => {
                Map::from_iter([("actions".to_owned(), Value::Array(actions))])
            }
            _ => {
                return Err(bad(format!(
                    "pocket player {action} takes a JSON object of its parameters"
                )));
            }
        },
        None => Map::new(),
    };
    let reads_text = matches!(action, "observe" | "nearby" | "events")
        || (action == "describe" && p.contains_key("entity"));
    if reads_text && !args.has("json") && !p.contains_key("projection") {
        p.insert("projection".into(), json!("text"));
    }
    if let Some(t) = args.number::<u64>("ticks")? {
        p.insert("ticks".into(), json!(t));
    }
    Ok((format!("player.{action}"), Value::Object(p)))
}

/// `pocket debug ...`: the debugger's short forms, or `<action> ['<json params>']` for any
/// `debug.*` method with its exact parameters.
fn debug_request(args: &Args) -> Result<(String, Value), Problem> {
    let action = pos(args, 0).unwrap_or("state");
    let rest: Vec<&str> = args.positional.iter().skip(1).map(String::as_str).collect();
    let canonical = match action {
        "break" => "breakpoints.set",
        "clear" => "breakpoints.clear",
        "list" => "breakpoints.list",
        a => a,
    };
    if let [one] = rest.as_slice()
        && one.trim_start().starts_with('{')
        && let Ok(v @ Value::Object(_)) = serde_json::from_str::<Value>(one)
    {
        return Ok((format!("debug.{canonical}"), v));
    }
    let mut p = Map::new();
    let frame = args.number::<u64>("frame")?.map(|f| json!(f));
    let method = match canonical {
        "state" => "debug.state",
        "breakpoints.set" => {
            let usage = "pocket debug break <file>:<line> [--if <expr>] [--log <text>]";
            let spec = rest.first().ok_or_else(|| bad(usage))?;
            let (file, line) = spec
                .rsplit_once(':')
                .and_then(|(f, l)| Some((f, l.parse::<u64>().ok()?)))
                .ok_or_else(|| {
                    bad(format!(
                        "'{spec}' is not <file>:<line> (scripts/helm.ts:67)"
                    ))
                })?;
            p.insert("file".into(), json!(file));
            p.insert("line".into(), json!(line));
            put(&mut p, "condition", args.value("if").map(|c| json!(c)));
            put(&mut p, "log", args.value("log").map(|l| json!(l)));
            "debug.breakpoints.set"
        }
        "breakpoints.clear" => {
            put(&mut p, "id", rest.first().map(|i| json!(i)));
            "debug.breakpoints.clear"
        }
        "breakpoints.list" => "debug.breakpoints.list",
        "eval" => {
            if rest.is_empty() {
                return Err(bad("pocket debug eval <expression> [--frame n]"));
            }
            p.insert("expr".into(), json!(rest.join(" ")));
            put(&mut p, "frame", frame);
            "debug.eval"
        }
        "set" => {
            let [name, value @ ..] = rest.as_slice() else {
                return Err(bad("pocket debug set <name> <expression> [--frame n]"));
            };
            if value.is_empty() {
                return Err(bad("pocket debug set <name> <expression> [--frame n]"));
            }
            p.insert("name".into(), json!(name));
            p.insert("value".into(), json!(value.join(" ")));
            put(&mut p, "frame", frame);
            "debug.set"
        }
        "watch" => {
            let spec = rest
                .first()
                .ok_or_else(|| bad("pocket debug watch <entity>.<Component>[.<field>]"))?;
            let mut parts = spec.splitn(3, '.');
            let (Some(e), Some(c)) = (parts.next(), parts.next()) else {
                return Err(bad(format!(
                    "'{spec}' is not <entity>.<Component>[.<field>] (Sloop.Boat.hoist)"
                )));
            };
            p.insert("entity".into(), entity(e));
            p.insert("component".into(), json!(c));
            put(&mut p, "field", parts.next().map(|f| json!(f)));
            "debug.watch"
        }
        "unwatch" => {
            put(&mut p, "id", rest.first().map(|i| json!(i)));
            "debug.unwatch"
        }
        "continue" => "debug.continue",
        "step" => {
            let kind = rest.first().copied().unwrap_or("over");
            if !["over", "into", "out"].contains(&kind) {
                return Err(usage(
                    format!("pocket debug step takes over, into or out, not '{kind}'"),
                    kind,
                    &["over", "into", "out"],
                ));
            }
            p.insert("kind".into(), json!(kind));
            "debug.step"
        }
        "pause" | "wait" => {
            let ms = match rest.first() {
                Some(t) => t
                    .parse::<u64>()
                    .map_err(|_| bad(format!("'{t}' is not a number of milliseconds")))?,
                None => 2000,
            };
            p.insert("timeout_ms".into(), json!(ms));
            if canonical == "pause" {
                "debug.pause"
            } else {
                "debug.wait"
            }
        }
        "exceptions" => {
            let mode = rest
                .first()
                .ok_or_else(|| bad("pocket debug exceptions none|uncaught|all"))?;
            p.insert("mode".into(), json!(mode));
            "debug.exceptions"
        }
        "rewind" => {
            let t: u64 = rest
                .first()
                .and_then(|t| t.parse().ok())
                .ok_or_else(|| bad("pocket debug rewind <tick> [--bundle snapshot]"))?;
            p.insert("tick".into(), json!(t));
            put(&mut p, "bundle", args.value("bundle").map(|b| json!(b)));
            "debug.rewind"
        }
        "attach" => "debug.attach",
        "detach" => "debug.detach",
        other => {
            let params = match rest.first() {
                Some(t) => json_arg(t)?,
                None => json!({}),
            };
            return Ok((format!("debug.{other}"), params));
        }
    };
    Ok((method.to_owned(), Value::Object(p)))
}

// --- Text forms ----------------------------------------------------------------------------------

/// A value for reading: arrays past 8 items cut to their first 3 and a count (`--json` has them
/// all), so `world get` of an entity with buoyancy points or hulls stays a few lines.
fn shorten(v: &Value) -> Value {
    match v {
        Value::Array(a) if a.len() > 8 => {
            let mut out: Vec<Value> = a.iter().take(3).map(shorten).collect();
            out.push(json!(format!("... {} more", a.len() - 3)));
            Value::Array(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(shorten).collect()),
        Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), shorten(v))).collect()),
        other => other.clone(),
    }
}

/// A value in a table or a locals list: numbers to 6 decimals at most, long texts cut.
fn cell(v: &Value) -> String {
    let s = match v {
        Value::Number(n) if n.is_f64() => {
            let f = n.as_f64().unwrap_or(0.0);
            let t = format!("{f:.6}");
            let t = t.trim_end_matches('0').trim_end_matches('.');
            if t.is_empty() || t == "-" {
                "0".to_owned()
            } else {
                t.to_owned()
            }
        }
        other => opt(other),
    };
    if s.chars().count() > 120 {
        let cut: String = s.chars().take(117).collect();
        format!("{cut}...")
    } else {
        s
    }
}

/// Where and why the debugger stopped, in one line:
/// `scripts/helm.ts:67 (breakpoint bp1 in helm, tick 6)`.
fn stop_line(s: &Value) -> String {
    let loc = &s["location"];
    let at = match (loc["file"].as_str(), loc["line"].as_u64()) {
        (Some(f), Some(l)) => format!("{f}:{l}"),
        _ => "a script statement".to_owned(),
    };
    let mut why = s["reason"].as_str().unwrap_or("pause").replace('_', " ");
    if let Some(bp) = s["breakpoint"]
        .as_str()
        .or_else(|| s["hit_breakpoints"][0].as_str())
    {
        let _ = write!(why, " {bp}");
    }
    let mut out = format!("{at} ({why}");
    if let Some(sys) = s["system"].as_str() {
        let _ = write!(out, " in {sys}");
    }
    let _ = write!(out, ", tick {})", opt(&s["tick"]));
    out
}

/// A data breakpoint's hit: `w1 #3 Boat.hoist 0 -> 1, written at scripts/helm.ts:78`.
fn watch_hit(w: &Value) -> String {
    let id = w["id"]
        .as_str()
        .or_else(|| w["watch"].as_str())
        .unwrap_or("?");
    let field = w["field"]
        .as_str()
        .map_or_else(String::new, |f| format!(".{f}"));
    let mut s = format!(
        "{id} #{} {}{field} {} -> {}",
        opt(&w["entity"]),
        opt(&w["component"]),
        cell(&w["before"]),
        cell(&w["after"])
    );
    let at = &w["written_at"];
    if let (Some(f), Some(l)) = (at["file"].as_str(), at["line"].as_u64()) {
        let _ = write!(s, ", written at {f}:{l}");
    }
    s
}

/// A local as `name = value`: an object's description (`Float64Array(1)`), else its JSON.
fn local_line(l: &Value) -> String {
    let v = &l["value"];
    let shown = match (l["description"].as_str(), v) {
        (Some(d), Value::Object(_) | Value::Array(_) | Value::Null) => d.to_owned(),
        // `undefined`, or a `let` its statement has not reached yet.
        (None, Value::Null) if l["type"] != "null" => opt(&l["type"]),
        _ => cell(v),
    };
    format!("{} = {shown}", opt(&l["name"]))
}

/// `debug.state` as text: where and why, the innermost frame's locals (`ctx` and closures only
/// with `--full`), one line per other frame, the breakpoints and watches.
fn debug_state_text(v: &Value, full: bool) -> String {
    let mut out = String::new();
    if v["state"] == "paused" {
        let _ = writeln!(out, "paused at {}", stop_line(v));
        if !v["data"].is_null() {
            let _ = writeln!(out, "  watch {}", watch_hit(&v["data"]));
        }
        if let Some(t) = v["exception"]["text"].as_str() {
            let caught = if v["exception"]["caught"] == json!(true) {
                "caught"
            } else {
                "uncaught"
            };
            let _ = writeln!(out, "  exception ({caught}): {t}");
        }
        for (i, f) in v["frames"].as_array().into_iter().flatten().enumerate() {
            let loc = &f["location"];
            let _ = writeln!(
                out,
                "#{} {} {}:{}{}",
                opt(&f["frame"]),
                opt(&f["function"]),
                opt(&loc["file"]),
                opt(&loc["line"]),
                if f["returned"] == json!(true) {
                    " (returned)"
                } else {
                    ""
                }
            );
            if i == 0 || full {
                for l in f["locals"].as_array().into_iter().flatten() {
                    if l["name"] == "ctx" && !full {
                        continue;
                    }
                    let _ = writeln!(out, "    {}", local_line(l));
                }
            }
            if full {
                for l in f["closure"].as_array().into_iter().flatten() {
                    let _ = writeln!(out, "    (closure) {}", local_line(l));
                }
            }
        }
    } else {
        let _ = writeln!(
            out,
            "running (tick {}{}; attached {}, instrumented {})",
            opt(&v["tick"]),
            v["system"]
                .as_str()
                .map_or_else(String::new, |s| format!(", last in {s}")),
            opt(&v["attached"]),
            opt(&v["instrumented"])
        );
    }
    for b in v["breakpoints"].as_array().into_iter().flatten() {
        let at = &b["locations"][0];
        let mut s = format!(
            "breakpoint {} {}:{}",
            opt(&b["id"]),
            opt(&at["file"]),
            opt(&at["line"])
        );
        if let Some(c) = b["condition"].as_str() {
            let _ = write!(s, " if {c}");
        }
        if let Some(l) = b["log"].as_str() {
            let _ = write!(s, " log {l}");
        }
        let _ = writeln!(out, "{s}");
    }
    for w in v["watches"].as_array().into_iter().flatten() {
        let field = w["field"]
            .as_str()
            .map_or_else(String::new, |f| format!(".{f}"));
        let _ = writeln!(
            out,
            "watch {} #{} {}{field}",
            opt(&w["id"]),
            opt(&w["entity"]),
            opt(&w["component"])
        );
    }
    if v["state"] == "paused" && !full {
        out.push_str("(pocket debug eval <expr> | step [over|into|out] | continue; --full for every frame)\n");
    }
    out
}

/// A `time.step`'s samples as a table: a header, one row per sample, columns aligned.
fn samples_table(s: &Value, out: &mut String) {
    let header: Vec<String> = s["columns"]
        .as_array()
        .into_iter()
        .flatten()
        .map(opt)
        .collect();
    let rows: Vec<Vec<String>> = s["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| r.as_array().into_iter().flatten().map(cell).collect())
        .collect();
    let widths: Vec<usize> = (0..header.len())
        .map(|i| {
            rows.iter()
                .filter_map(|r| r.get(i))
                .chain(std::iter::once(&header[i]))
                .map(|c| c.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |cells: &[String]| -> String {
        cells
            .iter()
            .zip(&widths)
            .map(|(c, w)| format!("{c:<w$}"))
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_owned()
    };
    let _ = writeln!(out, "{}", line(&header));
    for r in &rows {
        let _ = writeln!(out, "{}", line(r));
    }
}

/// The note a read made while the debugger holds the game starts with.
fn paused_note(p: &Value) -> String {
    format!(
        "(paused at {}: the world as of tick {}'s end)",
        stop_line(p),
        opt(&p["snapshot_tick"])
    )
}

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
    // The bundle the world runs: after an apply or a restore, which scripts are live.
    if !v["bundle"].is_null() {
        let _ = write!(s, " | scripts {}", short_hash(&v["bundle"]));
    }
    if v["halted"] == json!(true) || !v["poisoned"].is_null() {
        let _ = write!(s, " | HALTED (poisoned at {})", opt(&v["poisoned"]));
    }
    if let Some(p) = v.get("paused_at") {
        let _ = write!(s, " | HELD at {}", stop_line(p));
    }
    if let Some(r) = v.get("restored") {
        let sc = &v["scripts"];
        let scripts = match sc["kept"].as_str() {
            Some(kept) => format!(
                "; scripts {} ({kept}{})",
                short_hash(&sc["bundle"]),
                if sc["swap_refused"].is_null() {
                    ""
                } else {
                    ": the applied ones do not load on it"
                }
            ),
            None => String::new(),
        };
        s = format!("restored tick {}{scripts} | {s}", opt(r));
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
            opt(&d["message"]).replace('\n', "\n      ")
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

/// The note a list read while the debugger holds the game starts with (its rows carry the tick).
fn paused_rows_note(v: &Value, out: &mut String) {
    if let Some(t) = v[0]["paused_at"].as_u64() {
        let _ = writeln!(
            out,
            "(paused in tick {t}: the world as of tick {}'s end; pocket debug state)",
            t.saturating_sub(1)
        );
    }
}

/// The text form of a method's result; `full`: every frame of a debugger state.
fn text(method: &str, v: &Value, full: bool) -> String {
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
        "world.tree" => {
            paused_rows_note(v, &mut out);
            lines(&mut out, v, &entity_line);
        }
        "world.get" | "world_get" => {
            if !v["paused_at"].is_null() {
                let _ = writeln!(out, "{}", paused_note(&v["paused_at"]));
            }
            let _ = writeln!(out, "#{} {}", opt(&v["id"]), opt(&v["name"]));
            for (c, val) in v["components"].as_object().into_iter().flatten() {
                let _ = writeln!(out, "  {c}: {}", compact(&shorten(val)));
            }
            for (f, val) in v["fields"].as_object().into_iter().flatten() {
                let _ = writeln!(out, "  {f} = {}", compact(&shorten(val)));
            }
        }
        "world.query" => {
            paused_rows_note(v, &mut out);
            lines(&mut out, v, &|r| {
                let mut s = format!("#{} {}", opt(&r["id"]), opt(&r["name"]));
                for (k, val) in r.as_object().into_iter().flatten() {
                    if k != "id" && k != "name" && k != "paused_at" {
                        let _ = write!(s, "  {k}={}", compact(&shorten(val)));
                    }
                }
                s
            });
        }
        "debug.state" | "debug.step" | "debug.pause" | "debug.wait" | "debug.attach"
        | "debug.detach" => out = debug_state_text(v, full),
        "debug.continue" => out = "running\n".into(),
        "debug.breakpoints.set" | "debug.break" => {
            let _ = write!(
                out,
                "{} {}:{}",
                opt(&v["id"]),
                opt(&v["file"]),
                opt(&v["line"])
            );
            if v["verified"] != json!(true) {
                out.push_str(" (not verified)");
            }
            out.push('\n');
        }
        "debug.breakpoints.list" => {
            for b in v["breakpoints"].as_array().into_iter().flatten() {
                let at = &b["locations"][0];
                let mut s = format!(
                    "{} {} {}:{}",
                    opt(&b["id"]),
                    opt(&b["owner"]),
                    opt(&at["file"]),
                    opt(&at["line"])
                );
                if let Some(c) = b["condition"].as_str() {
                    let _ = write!(s, " if {c}");
                }
                if let Some(l) = b["log"].as_str() {
                    let _ = write!(s, " log {l}");
                }
                let _ = writeln!(out, "{s}");
            }
        }
        "debug.breakpoints.clear" | "debug.unwatch" | "debug.watch.clear" => {
            let _ = writeln!(out, "cleared {}", opt(&v["cleared"]));
        }
        "debug.eval" | "debug.set" => {
            let shown = match (v["description"].as_str(), &v["value"]) {
                (Some(d), Value::Object(_) | Value::Array(_) | Value::Null) => d.to_owned(),
                _ => compact(&v["value"]),
            };
            let _ = writeln!(out, "{shown}  ({})", opt(&v["type"]));
        }
        "debug.watch" => {
            let field = v["field"]
                .as_str()
                .map_or_else(String::new, |f| format!(".{f}"));
            let _ = writeln!(
                out,
                "{} watching #{} {}{field}",
                opt(&v["id"]),
                opt(&v["entity"]),
                opt(&v["component"])
            );
        }
        "debug.exceptions" => {
            let _ = writeln!(out, "pause on exceptions: {}", opt(&v["mode"]));
        }
        "debug.rewind" => {
            let sc = &v["scripts"];
            let _ = write!(
                out,
                "restored tick {}; scripts {} ({}) | at tick {}",
                opt(&v["restored"]),
                short_hash(&sc["bundle"]),
                opt(&sc["kept"]),
                opt(&v["tick"])
            );
            if !v["stopped_by"].is_null() {
                let _ = write!(out, " | stopped at {}", stop_line(&v["stopped_by"]));
            }
            out.push('\n');
        }
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
                // The script debugger stopped the game inside the tick.
                Some(
                    "breakpoint" | "data_breakpoint" | "step" | "pause" | "exception"
                    | "debugger_statement",
                ) => {
                    let _ = write!(out, " | stopped at {}", stop_line(why));
                    if !why["watch"].is_null() {
                        let _ = write!(out, "; {}", watch_hit(&why["watch"]));
                    }
                    if v["paused"] == json!(true) {
                        out.push_str(": pocket debug state | pocket debug continue");
                    }
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
            if v["samples"].is_object() {
                samples_table(&v["samples"], &mut out);
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
        "scripts.read" | "scripts.guide" => out = v["text"].as_str().unwrap_or("").to_owned(),
        "scripts.status" => {
            let ran: Vec<String> = v["ran_last_tick"]
                .as_array()
                .into_iter()
                .flatten()
                .map(opt)
                .collect();
            let _ = writeln!(
                out,
                "scripts {} (running){}",
                opt(&v["bundle"]),
                if v["ran_last_tick"].is_null() {
                    String::new()
                } else {
                    format!(" | ran last tick: {}", ran.join(" "))
                }
            );
            if !v["paused_at"].is_null() {
                let _ = writeln!(out, "{}", paused_note(&v["paused_at"]));
            }
        }
        "docs.search" => lines(&mut out, v, &|d| {
            if let Some(c) = d["command"].as_str() {
                format!("command {c}: {}", opt(&d["doc"]))
            } else if let Some(c) = d["component"].as_str() {
                let fields: Vec<String> = d["fields"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(opt)
                    .collect();
                format!("component {c} ({}): {}", fields.join(", "), opt(&d["doc"]))
            } else {
                format!("guide: {} ({})", opt(&d["guide"]), opt(&d["read"]))
            }
        }),
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
            if let Some(ms) = v["tsc_ms"].as_f64() {
                let version = v["tsc_version"].as_str().unwrap_or("");
                head.push(format!("tsc {version} {ms:.0} ms").replace("  ", " "));
            }
            let n = v["diagnostics"].as_array().map_or(0, Vec::len);
            head.push(format!("{n} diagnostics"));
            let _ = writeln!(out, "{}", head.join(" | "));
            diagnostics(&v["diagnostics"], &mut out);
            if let Some(r) = v["reason"].as_str() {
                let _ = writeln!(out, "typecheck: {r}");
            }
        }
        "scripts.types" => {
            let list = |k: &str| {
                v["components"][k]
                    .as_array()
                    .map(|a| a.iter().map(opt).collect::<Vec<_>>().join(" "))
                    .unwrap_or_default()
            };
            for f in v["files"].as_array().into_iter().flatten() {
                let _ = writeln!(out, "{} {} B", opt(&f["path"]), opt(&f["bytes"]));
            }
            if !v["tsconfig"].is_null() {
                let _ = writeln!(out, "tsconfig.json {}", opt(&v["tsconfig"]));
            }
            let _ = writeln!(out, "engine: {}", list("engine"));
            let _ = writeln!(
                out,
                "game ({}): {}",
                opt(&v["project_from"]),
                list("project")
            );
            let _ = writeln!(out, "not for scripts: {}", list("unavailable"));
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
        // A player's JSON answers (act, wait, session, describe) on one line: what an agent reads
        // in its shell costs tokens.
        m if m.starts_with("player.") => {
            out = serde_json::to_string(v).unwrap_or_default() + "\n";
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

/// The debugger's methods for the catalog without a host.
struct OfflineDebugger;

impl pocket_server::DebugHub for OfflineDebugger {
    fn call(&self, method: &str, _: Value) -> pocket_server::BoxFuture<Result<Value, Problem>> {
        let m = method.to_owned();
        Box::pin(async move {
            Err(Problem::new(
                "debug.not_available",
                format!("{m} needs a running host."),
                detail([]),
            ))
        })
    }

    fn methods(&self) -> Vec<Value> {
        crate::present::debug_methods()
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
    all.extend(pocket_server::debug_catalog(Some(&OfflineDebugger)));
    all.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
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
             --host, POCKET_HOST or .pocket/host.json. --json prints exact JSON; --timeout <s> (default 60, POCKET_TIMEOUT) bounds the wait.\n\n",
        );
        for (_, _, u) in COMMANDS {
            let _ = writeln!(out, "  {u}");
        }
        out.push_str("\nHost commands: pocket serve <project> [--port 7878], pocket mcp <project>; \
                      also pocket run|check|replay|hashes|version.\nMethods (pocket call <method> '<json>'; \
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

/// Whether a check found an error (the compile's, the load's or `tsc`'s) or `tsc` timed out, so
/// that `pocket scripts check` exits 1 and can gate an apply: scripts are never passed unchecked.
fn has_errors(v: &Value) -> bool {
    v["outcome"] == "refused"
        || v["typecheck"] == "failed"
        || v["typecheck"] == "timeout"
        || v["diagnostics"]
            .as_array()
            .is_some_and(|d| d.iter().any(|d| d["severity"] == "error"))
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
    let given = match args.number::<f64>("timeout") {
        Ok(t) => t,
        Err(p) => return failed(&p, 2),
    };
    let timeout = timeout_for(given, &params);
    let seat = args.value("seat");
    // A seat's player sends the player tools alone; the host and the game refuse the rest too.
    if seat.is_some()
        && let Err(p) = pocket_server::seat_permits(&method)
    {
        return failed(&p, 2);
    }
    match call_url_as(&url, &method, params, seat, timeout) {
        Ok(v) => {
            if json_out {
                println!("{v}");
            } else if let Value::String(s) = &v {
                // A text projection (an observation) prints as it is.
                print!("{s}");
                if !s.ends_with('\n') {
                    println!();
                }
            } else {
                print!("{}", text(&method, &v, args.has("full")));
            }
            Outcome {
                stdout: String::new(),
                code: i32::from(method == "scripts.check" && has_errors(&v)),
            }
        }
        Err(p) => failed(&p, 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(cmd: &str, args: &[&str]) -> (String, Value) {
        let raw: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
        let a = Args::parse(&raw, FLAGS).unwrap();
        request(cmd, &a).unwrap_or_else(|p| panic!("{}: {}", p.code, p.message))
    }

    /// The debugger's short forms (docs/bench/debug-eval.md, finding 4).
    #[test]
    fn debug_short_forms() {
        assert_eq!(
            req(
                "debug",
                &["break", "scripts/helm.ts:67", "--if", "steer < 0"]
            ),
            (
                "debug.breakpoints.set".into(),
                json!({"file": "scripts/helm.ts", "line": 67, "condition": "steer < 0"})
            )
        );
        assert_eq!(
            req("debug", &["eval", "bearing", "*", "180"]),
            ("debug.eval".into(), json!({"expr": "bearing * 180"}))
        );
        assert_eq!(
            req("debug", &["watch", "Sloop.Boat.hoist"]),
            (
                "debug.watch".into(),
                json!({"entity": "Sloop", "component": "Boat", "field": "hoist"})
            )
        );
        assert_eq!(
            req("debug", &["watch", "3.Log"]),
            (
                "debug.watch".into(),
                json!({"entity": 3, "component": "Log"})
            )
        );
        assert_eq!(
            req("debug", &["step"]),
            ("debug.step".into(), json!({"kind": "over"}))
        );
        assert_eq!(
            req("debug", &["rewind", "120", "--bundle", "snapshot"]),
            (
                "debug.rewind".into(),
                json!({"tick": 120, "bundle": "snapshot"})
            )
        );
        // The exact parameters still work, for every method.
        assert_eq!(
            req(
                "debug",
                &["breakpoints.set", r#"{"file": "scripts/x.ts", "line": 3}"#]
            ),
            (
                "debug.breakpoints.set".into(),
                json!({"file": "scripts/x.ts", "line": 3})
            )
        );
        assert_eq!(
            req("debug", &["set", "r", "r", "+", "1", "--frame", "1"]),
            (
                "debug.set".into(),
                json!({"name": "r", "value": "r + 1", "frame": 1})
            )
        );
        let raw = vec!["step".to_owned(), "sideways".to_owned()];
        let a = Args::parse(&raw, FLAGS).unwrap();
        assert_eq!(debug_request(&a).unwrap_err().code, "check.usage");
    }

    /// `pocket player`: the `player.*` methods, observations as text unless `--json`, the
    /// actions alone for `act`.
    #[test]
    fn player_short_forms() {
        assert_eq!(
            req("player", &["observe"]),
            ("player.observe".into(), json!({"projection": "text"}))
        );
        assert_eq!(
            req("player", &["observe", "--json"]),
            ("player.observe".into(), json!({}))
        );
        assert_eq!(
            req("player", &["wait", "--ticks", "120"]),
            ("player.wait".into(), json!({"ticks": 120}))
        );
        let (m, p) = req(
            "player",
            &[
                "act",
                r#"[{"do": "start", "intent": "sail_to", "target": "Mark1"}]"#,
            ],
        );
        assert_eq!(m, "player.act");
        assert_eq!(p["actions"][0]["intent"], json!("sail_to"));
        let raw: Vec<String> = ["observ"].iter().map(|s| (*s).to_owned()).collect();
        let e = request("player", &Args::parse(&raw, FLAGS).unwrap()).unwrap_err();
        assert!(format!("{e:?}").contains("observe"), "{e:?}");
    }

    /// Field-level reads (finding 5): fields with paths, line ranges, a sampled step.
    #[test]
    fn field_level_short_forms() {
        assert_eq!(
            req(
                "world",
                &["get", "Sloop", "Boat.heading_deg,Boat.rudder", "Crew"]
            ),
            (
                "world.get".into(),
                json!({"entity": "Sloop", "components": ["Crew"],
                       "fields": ["Boat.heading_deg", "Boat.rudder"]})
            )
        );
        assert_eq!(
            req("scripts", &["read", "helm.ts", "--lines", "50-80"]),
            (
                "scripts.read".into(),
                json!({"path": "helm.ts", "lines": "50-80", "numbered": true})
            )
        );
        assert_eq!(
            req(
                "step",
                &[
                    "300",
                    "--sample",
                    "Sloop.Boat.heading_deg,Sloop.Boat.rudder",
                    "--every",
                    "30"
                ]
            ),
            (
                "time.step".into(),
                json!({"ticks": 300, "sample": {"fields": ["Sloop.Boat.heading_deg",
                       "Sloop.Boat.rudder"], "every": 30}})
            )
        );
        assert_eq!(
            req("snapshots", &["restore", "0", "--bundle", "snapshot"]),
            (
                "snapshots.restore".into(),
                json!({"tick": 0, "bundle": "snapshot"})
            )
        );
        assert_eq!(
            req("docs", &["data", "breakpoint"]),
            ("docs.search".into(), json!({"query": "data breakpoint"}))
        );
    }

    /// A paused state as text: the place, the innermost frame's locals without `ctx`, one line
    /// per other frame; the step that stopped names where.
    #[test]
    fn debug_state_reads_in_a_few_lines() {
        let state = json!({
            "state": "paused", "reason": "breakpoint", "tick": 6, "system": "helm",
            "location": {"file": "scripts/helm.ts", "line": 67, "column": 9},
            "hit_breakpoints": ["bp1"],
            "frames": [
                {"frame": 0, "function": "run", "returned": false,
                 "location": {"file": "scripts/helm.ts", "line": 67},
                 "locals": [{"name": "bearing", "type": "number", "value": 1.3438123456789},
                            {"name": "ctx", "type": "object", "value": {"big": [1, 2, 3]},
                             "description": "Object"},
                            {"name": "later", "type": "undefined", "value": null},
                            {"name": "q", "type": "object", "value": {},
                             "description": "Float64Array(1)"}],
                 "closure": [{"name": "GAIN", "type": "number", "value": 0.6}]},
                {"frame": 2, "function": "each", "returned": false,
                 "location": {"file": "scripts/helm.ts", "line": 40},
                 "locals": [{"name": "i", "type": "number", "value": 0}], "closure": []}
            ],
            "breakpoints": [{"id": "bp1", "locations": [{"file": "scripts/helm.ts", "line": 67}],
                             "condition": null, "log": null}],
            "watches": []
        });
        let t = debug_state_text(&state, false);
        assert!(
            t.starts_with("paused at scripts/helm.ts:67 (breakpoint bp1 in helm, tick 6)\n"),
            "{t}"
        );
        assert!(t.contains("    bearing = 1.343812\n"), "{t}");
        assert!(t.contains("    later = undefined\n"), "{t}");
        assert!(t.contains("    q = Float64Array(1)\n"), "{t}");
        assert!(
            !t.contains("ctx") && !t.contains("GAIN") && !t.contains("i = 0"),
            "{t}"
        );
        assert!(t.contains("#2 each scripts/helm.ts:40\n"), "{t}");
        let full = debug_state_text(&state, true);
        assert!(full.contains("ctx = Object") && full.contains("(closure) GAIN = 0.6"));
        let step = json!({"tick": 6, "world_hash": null, "errors": [], "paused": true,
                          "stopped_by": {"reason": "breakpoint", "tick": 6, "system": "helm",
                                         "breakpoint": "bp1", "location": state["location"]}});
        assert_eq!(
            text("time.step", &step, false),
            "tick 6 hash - | stopped at scripts/helm.ts:67 (breakpoint bp1 in helm, tick 6): \
             pocket debug state | pocket debug continue\n"
        );
    }

    #[test]
    fn samples_and_long_arrays_read_as_text() {
        let v = json!({"tick": 60, "world_hash": "abc", "errors": [],
                       "samples": {"columns": ["tick", "Sloop.Boat.rudder"],
                                   "rows": [[30, 0.25], [60, -0.5]], "every": 30}});
        assert_eq!(
            text("time.step", &v, false),
            "tick 60 hash abc\ntick  Sloop.Boat.rudder\n30    0.25\n60    -0.5\n"
        );
        let points: Vec<Value> = (0..70).map(|i| json!(i)).collect();
        assert_eq!(
            compact(&shorten(&json!({ "points": points }))),
            r#"{"points":[0,1,2,"... 67 more"]}"#
        );
        assert_eq!(
            timeout_for(Some(5.0), &json!({"timeout_ms": 30000})),
            Duration::from_secs(40)
        );
        assert_eq!(timeout_for(Some(5.0), &json!({})), Duration::from_secs(5));
    }
}
