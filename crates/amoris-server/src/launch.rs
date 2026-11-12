//! Agent programs the host starts (docs/spec/server.md 3.7): the process owner registers them in
//! a file outside the project (`amoris serve --agents <file>`), and `agent.session.start {agent,
//! goal, seat?}` starts one of them by name as an agent session of this host. The engine never
//! picks a program, a model or a credential: what runs, and with which keys, is the file's
//! command and the environment the host itself was started with.
//!
//! A started program is a session of transport `launch`, opened before the process starts: the
//! process gets the host's URL, the session's id and handle and the goal in its environment, so an
//! `amoris mcp --attach` it runs calls as that session; each line it writes is a `message` event of
//! the session, and the session closes when the process exits. `agent.session.stop` ends it.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use amoris_contract::{Candidate, CheckOptions, Pointer, Problem, codes, detail};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::Host;
use crate::sessions::{Spec, Step, Transport};

/// Programs the host runs at once.
pub const RUNNING: usize = 4;
/// The longest goal, in characters.
const GOAL_MAX: usize = 4000;
/// The longest line kept of a program's output, in bytes.
const LINE_MAX: usize = 1000;
/// How long `agent.session.stop` waits for the process to go.
const STOP_WAIT: Duration = Duration::from_secs(5);

/// The agents file: `{"agents": [{"name", "command", "label"?, "env"?}]}`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct File {
    agents: Vec<Entry>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Entry {
    /// What the editor and `agent.session.start` call it: `^[a-z0-9][a-z0-9_.-]*$`.
    name: String,
    /// The program and its arguments, run without a shell in the project's directory.
    command: Vec<String>,
    /// The sessions' label; default: the name.
    #[serde(default)]
    label: Option<String>,
    /// More environment for the process, beside the host's own and the session's.
    #[serde(default)]
    env: BTreeMap<String, String>,
}

/// One registered program.
#[derive(Clone, Debug)]
pub(crate) struct Launcher {
    name: String,
    label: String,
    command: Vec<String>,
    env: BTreeMap<String, String>,
}

/// The programs a host may start, as the agents file gives them.
#[derive(Clone, Debug, Default)]
pub struct Launchers {
    list: Vec<Launcher>,
}

fn invalid(file: &str, reason: impl Into<String>) -> Problem {
    let reason = reason.into();
    Problem::new(
        "host.agents_invalid",
        format!("The agents file {file} is refused: {reason}."),
        detail([("file", json!(file)), ("reason", json!(reason))]),
    )
}

fn named(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_.-".contains(c))
}

impl Launchers {
    /// The programs a file's text registers, or `host.agents_invalid`: text that is not JSON, a
    /// field the file does not take, no program, a name that is not `^[a-z0-9][a-z0-9_.-]*$` or
    /// is given twice, an empty command.
    pub fn parse(text: &str, file: &str) -> Result<Launchers, Problem> {
        let value: Value = serde_json::from_str(text)
            .map_err(|e| invalid(file, format!("it is not JSON ({e})")))?;
        let parsed = amoris_contract::decode::<File>(&value, &CheckOptions::new("the agents file"))
            .map_err(|p| {
                let mut r = invalid(file, p.message.trim_end_matches('.').to_owned());
                r.detail.insert("problem".into(), json!(p));
                r
            })?
            .value;
        if parsed.agents.is_empty() {
            return Err(invalid(file, "it registers no agent program"));
        }
        let mut list: Vec<Launcher> = Vec::new();
        for e in parsed.agents {
            if !named(&e.name) {
                return Err(invalid(
                    file,
                    format!("'{}' is not a name: ^[a-z0-9][a-z0-9_.-]*$", e.name),
                ));
            }
            if list.iter().any(|l| l.name == e.name) {
                return Err(invalid(file, format!("'{}' is registered twice", e.name)));
            }
            if e.command.first().is_none_or(|c| c.is_empty()) {
                return Err(invalid(file, format!("'{}' has no command", e.name)));
            }
            let label = e
                .label
                .map(|l| l.trim().to_owned())
                .filter(|l| !l.is_empty())
                .unwrap_or_else(|| e.name.clone());
            list.push(Launcher {
                name: e.name,
                label,
                command: e.command,
                env: e.env,
            });
        }
        Ok(Launchers { list })
    }

    /// [`Launchers::parse`] of the file at `path`, which must lie outside `project` (whose files
    /// the host serves and agents write): an agent must not choose what the host runs.
    pub fn load(path: &Path, project: &Path) -> Result<Launchers, Problem> {
        let shown = path.display().to_string();
        let full = path
            .canonicalize()
            .map_err(|e| invalid(&shown, format!("it cannot be read ({e})")))?;
        if let Ok(root) = project.canonicalize()
            && full.starts_with(&root)
        {
            return Err(invalid(
                &shown,
                "it lies inside the project, whose files the host serves and agents write; \
                 keep it outside",
            ));
        }
        let text = std::fs::read_to_string(&full)
            .map_err(|e| invalid(&shown, format!("it cannot be read ({e})")))?;
        Launchers::parse(&text, &shown)
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

/// `agent.session.start`'s parameters.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct StartParams {
    /// A program the host registered (agent.session.agents lists them).
    agent: String,
    /// What the agent is asked to do: AMORIS_AGENT_GOAL in its environment.
    goal: String,
    /// The seat it plays (its calls are that seat's player's); without it a developer.
    #[serde(default)]
    seat: Option<String>,
}

/// `agent.session.stop`'s parameters.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct StopParams {
    /// The started session's id (`mcp-3`).
    session: String,
}

/// The host's started programs, by session id.
#[derive(Default)]
pub(crate) struct Running(Mutex<BTreeMap<String, Arc<Mutex<Child>>>>);

fn decode<T: serde::de::DeserializeOwned + JsonSchema>(
    v: &Value,
    owner: &str,
) -> Result<T, Problem> {
    let empty = json!({});
    let v = if v.is_null() { &empty } else { v };
    amoris_contract::decode::<T>(v, &CheckOptions::new(owner)).map(|d| d.value)
}

/// `line` cut to [`LINE_MAX`] bytes at a character's edge.
fn cut(line: &str) -> String {
    if line.len() <= LINE_MAX {
        return line.to_owned();
    }
    let mut end = LINE_MAX;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &line[..end])
}

/// The program to run for a command's first word, found as a shell finds it. On Windows a name
/// without an extension is looked up with each of `PATHEXT`'s extensions (`.exe`, `.cmd`, ...), in
/// the `PATH` the program is given, as `cmd.exe` does: the standard library's lookup adds `.exe`
/// alone and would miss `claude.cmd`, which npm installs. Elsewhere, and for a name not found
/// that way, the word as it is (whose failure to start is then the host's answer).
fn program(first: &str, env: &BTreeMap<String, String>) -> std::path::PathBuf {
    let given = std::path::PathBuf::from(first);
    if !cfg!(windows) || given.extension().is_some() {
        return given;
    }
    let var = |name: &str| {
        env.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| std::ffi::OsString::from(v))
            .or_else(|| std::env::var_os(name))
    };
    let exts = var("PATHEXT")
        .and_then(|e| e.into_string().ok())
        .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_owned());
    let bare = given.components().count() == 1 && !given.has_root();
    let dirs: Vec<std::path::PathBuf> = if bare {
        var("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default()
    } else {
        vec![std::path::PathBuf::new()]
    };
    dirs.iter()
        .flat_map(|d| {
            exts.split(';')
                .filter(|e| !e.is_empty())
                .map(move |e| d.join(format!("{first}{e}")))
        })
        .find(|p| p.is_file())
        .unwrap_or(given)
}

/// The host's words for a program's end: `exit status: N` on every platform (the standard
/// library's Windows text is `exit code: N`), else how it ended (`signal: 9 (SIGKILL)`).
fn exit_words(status: std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit status: {code}"),
        None => status.to_string(),
    }
}

/// On Windows, ends the processes `child` started before it is killed itself: a `.cmd` program
/// runs its real one as a child of `cmd.exe`, which would outlive a kill of `cmd.exe` and keep its
/// output open. Elsewhere nothing: the program is killed alone (SIGKILL), as it was started.
fn end_tree(child: &mut Child) {
    if cfg!(windows) {
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

impl Host {
    /// Registers the programs `agent.session.start` may run; once, before the host serves.
    pub fn set_launchers(&self, launchers: Launchers) {
        if let Ok(mut l) = self.0.launchers.write() {
            *l = launchers;
        }
    }

    fn launchers(&self) -> Launchers {
        self.0
            .launchers
            .read()
            .map(|l| l.clone())
            .unwrap_or_default()
    }

    /// `agent.session.agents`: the registered programs by name and label (never their commands).
    pub(crate) fn agents(&self) -> Value {
        let list: Vec<Value> = self
            .launchers()
            .list
            .iter()
            .map(|l| json!({"name": l.name, "label": l.label}))
            .collect();
        let running = self.0.running.0.lock().map_or(0, |r| r.len());
        json!({"agents": list, "running": running, "limit": RUNNING})
    }

    /// `agent.session.start`.
    pub(crate) async fn start_agent(&self, params: &Value) -> Result<Value, Problem> {
        let method = "agent.session.start";
        let p: StartParams = decode(params, method)?;
        if self.locked() {
            return Err(Problem::new(
                "host.not_supported",
                "A locked host starts no agent program: a started program would need a grant of \
                 its own (server.md 3.7).",
                detail([("method", json!(method))]),
            ));
        }
        let launchers = self.launchers();
        if launchers.is_empty() {
            return Err(Problem::new(
                "host.no_agents",
                "This host has no agent programs: the process owner registers them with \
                 `amoris serve --agents <file>` (server.md 3.7).",
                detail([]),
            ));
        }
        let Some(launcher) = launchers.list.iter().find(|l| l.name == p.agent).cloned() else {
            let valid: Vec<Candidate<'_>> = launchers
                .list
                .iter()
                .map(|l| Candidate::new(&l.name))
                .collect();
            return Err(codes::invalid_value(
                &Pointer::root().key("agent"),
                &p.agent,
                &valid,
            ));
        };
        let goal = p.goal.trim().to_owned();
        if goal.is_empty() || goal.chars().count() > GOAL_MAX {
            return Err(Problem::new(
                "request.out_of_range",
                format!("A goal is 1 to {GOAL_MAX} characters."),
                detail([
                    ("path", json!("/goal")),
                    ("got", json!(goal.chars().count())),
                    ("max", json!(GOAL_MAX)),
                ]),
            ));
        }
        let held = self.0.running.0.lock().map_or(0, |r| r.len());
        if held >= RUNNING {
            return Err(Problem::new(
                "session.limit_reached",
                format!("This host already runs {held} agent programs; the limit is {RUNNING}."),
                detail([
                    ("limit", json!(RUNNING)),
                    ("value", json!(held)),
                    ("what", json!("agent programs")),
                ]),
            ));
        }
        // The seat is looked up before anything starts.
        if let Some(seat) = &p.seat {
            self.player_client(seat).await?;
        }
        let Some(url) = self.0.url.get().cloned() else {
            return Err(Problem::new(
                "host.not_supported",
                "This host does not serve HTTP, which a started program calls back on.",
                detail([]),
            ));
        };
        let opened = self.session_open(Spec {
            transport: Transport::Launch,
            seat: p.seat.clone(),
            label: launcher.label.clone(),
            live: true,
            shared: false,
        });
        let id = opened.id.clone();
        let mut cmd = Command::new(program(&launcher.command[0], &launcher.env));
        cmd.args(&launcher.command[1..])
            .current_dir(&self.0.project)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(&launcher.env)
            .env("AMORIS_HOST", &url)
            .env("AMORIS_PROJECT", &self.0.project)
            .env("AMORIS_AGENT", &launcher.name)
            .env("AMORIS_AGENT_SESSION", &id)
            .env(
                "AMORIS_AGENT_HANDLE",
                opened.handle.as_deref().unwrap_or(""),
            )
            .env("AMORIS_AGENT_GOAL", &goal);
        match &p.seat {
            Some(seat) => cmd.env("AMORIS_AGENT_SEAT", seat),
            None => cmd.env_remove("AMORIS_AGENT_SEAT"),
        };
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let why = format!("{} did not start: {e}", launcher.command[0]);
                self.agent_event(
                    &id,
                    Step::Message {
                        text: &why,
                        stream: "host",
                    },
                );
                self.session_close(&id);
                return Err(Problem::new(
                    "host.agent_failed",
                    format!("The agent program '{}' did not start: {e}.", launcher.name),
                    detail([("agent", json!(launcher.name)), ("session", json!(id))]),
                ));
            }
        };
        let pid = child.id();
        let out = child.stdout.take();
        let err = child.stderr.take();
        let child = Arc::new(Mutex::new(child));
        if let Ok(mut r) = self.0.running.0.lock() {
            r.insert(id.clone(), child.clone());
        }
        self.agent_event(
            &id,
            Step::Message {
                text: &format!("started {} (pid {pid}): {goal}", launcher.name),
                stream: "host",
            },
        );
        let readers: Vec<std::thread::JoinHandle<()>> = [
            out.map(|s| (Box::new(s) as Box<dyn Read + Send>, "stdout")),
            err.map(|s| (Box::new(s) as Box<dyn Read + Send>, "stderr")),
        ]
        .into_iter()
        .flatten()
        .map(|(stream, name)| {
            let host = self.clone();
            let id = id.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stream).lines() {
                    let Ok(line) = line else { break };
                    if !line.trim().is_empty() {
                        host.agent_event(
                            &id,
                            Step::Message {
                                text: &cut(&line),
                                stream: name,
                            },
                        );
                    }
                }
            })
        })
        .collect();
        let host = self.clone();
        let session = id.clone();
        std::thread::spawn(move || {
            let status = loop {
                let polled = child.lock().map(|mut c| c.try_wait());
                match polled {
                    Ok(Ok(Some(status))) => break exit_words(status),
                    Ok(Ok(None)) => std::thread::sleep(Duration::from_millis(50)),
                    Ok(Err(e)) => break format!("lost: {e}"),
                    Err(_) => break "lost".to_owned(),
                }
            };
            // Its last lines before its end.
            for r in readers {
                let _ = r.join();
            }
            host.agent_event(
                &session,
                Step::Message {
                    text: &status,
                    stream: "host",
                },
            );
            host.session_close(&session);
            if let Ok(mut r) = host.0.running.0.lock() {
                r.remove(&session);
            }
        });
        let mut row = json!(opened.row);
        row["agent"] = json!(launcher.name);
        row["pid"] = json!(pid);
        Ok(row)
    }

    /// `agent.session.stop`: ends a started program and answers its session's row once closed.
    pub(crate) async fn stop_agent(&self, params: &Value) -> Result<Value, Problem> {
        let p: StopParams = decode(params, "agent.session.stop")?;
        let child = self
            .0
            .running
            .0
            .lock()
            .ok()
            .and_then(|r| r.get(&p.session).cloned());
        let Some(child) = child else {
            let known = self.sessions().row(&p.session).cloned();
            return match known {
                // Its program already ended: the row as it is.
                Some(row) if row.transport == Transport::Launch && !row.live => Ok(json!(row)),
                _ => Err(Problem::new(
                    "host.session_unknown",
                    format!(
                        "This host runs no agent program as the session '{}'; \
                         agent.session.list lists the sessions (transport launch).",
                        p.session
                    ),
                    detail([("session", json!(p.session))]),
                )),
            };
        };
        self.agent_event(
            &p.session,
            Step::Message {
                text: "stopping",
                stream: "host",
            },
        );
        if let Ok(mut c) = child.lock() {
            end_tree(&mut c);
            let _ = c.kill();
        }
        let waited = std::time::Instant::now();
        loop {
            let row = self.sessions().row(&p.session).cloned();
            match row {
                Some(row) if !row.live => return Ok(json!(row)),
                None => {
                    return Err(Problem::new(
                        "host.session_unknown",
                        format!("The session '{}' is no longer listed.", p.session),
                        detail([("session", json!(p.session))]),
                    ));
                }
                Some(_) if waited.elapsed() > STOP_WAIT => {
                    return Err(Problem::new(
                        "host.agent_failed",
                        format!(
                            "The agent program of '{}' did not end within {} s.",
                            p.session,
                            STOP_WAIT.as_secs()
                        ),
                        detail([("session", json!(p.session))]),
                    ));
                }
                Some(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn a_bare_name_is_found_with_pathext_as_cmd_finds_it() {
        let dir = std::env::temp_dir().join(format!("amoris-launch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("agent.cmd"), "@echo hi\r\n").unwrap();
        // The program's own PATH, as its `env` gives it, under any case.
        let env = BTreeMap::from([("Path".to_owned(), dir.display().to_string())]);
        let found = program("agent", &env);
        assert!(
            found
                .to_string_lossy()
                .eq_ignore_ascii_case(&dir.join("agent.cmd").to_string_lossy()),
            "{}",
            found.display()
        );
        // A name with an extension, or one found nowhere, is left as it is.
        assert_eq!(program("agent.cmd", &env), Path::new("agent.cmd"));
        assert_eq!(program("nothing-here", &env), Path::new("nothing-here"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_agents_file_is_read_strictly() {
        let ok = Launchers::parse(
            r#"{"agents": [{"name": "claude", "command": ["claude", "-p"], "label": "Claude Code"},
                           {"name": "echo", "command": ["sh", "-c", "echo hi"]}]}"#,
            "f",
        )
        .unwrap();
        assert_eq!(ok.len(), 2);
        assert_eq!(ok.list[1].label, "echo");
        for (text, says) in [
            ("{", "not JSON"),
            (r#"{"agents": []}"#, "no agent program"),
            (
                r#"{"agents": [{"name": "A", "command": ["x"]}]}"#,
                "not a name",
            ),
            (
                r#"{"agents": [{"name": "a", "command": []}]}"#,
                "no command",
            ),
            (
                r#"{"agents": [{"name": "a", "command": ["x"]}, {"name": "a", "command": ["y"]}]}"#,
                "twice",
            ),
            (
                r#"{"agents": [{"name": "a", "command": ["x"], "model": "m"}]}"#,
                "model",
            ),
        ] {
            let p = Launchers::parse(text, "f").unwrap_err();
            assert_eq!(p.code, "host.agents_invalid", "{text}");
            assert!(p.message.contains(says), "{text}: {}", p.message);
        }
    }
}
