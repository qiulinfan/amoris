//! `pocket check`, `pocket replay --verify` and `pocket hashes` (docs/spec/checks.md 2, 3, 8): the
//! project checks with this binary as their child process, a replay verified in a fresh process,
//! and a run's hash per tick as JSON for the web build to compare against.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

use pocket_check::{
    CHECKS, Child, ChildAnswer, ChildRequest, Options, Report, Snapshot, Subject, TickRef,
    VerifySummary, WorldHash, check_project, runs, unknown_check, verify_bytes,
};
use pocket_contract::{Problem, detail};
use serde_json::{Value, json};

use crate::cli::{Args, Flag, hex, unhex};

pub const CHECK_FLAGS: &[Flag] = &[
    ("json", false),
    ("seeds", true),
    ("only", true),
    ("chain-only", false),
    ("snapshot-at", true),
    ("out", true),
];

/// What a subcommand prints and its exit code.
pub struct Outcome {
    pub stdout: String,
    pub code: i32,
}

fn clock() -> Arc<dyn Fn() -> f64 + Send + Sync> {
    let start = Instant::now();
    Arc::new(move || start.elapsed().as_secs_f64() * 1000.0)
}

fn report_out(r: &Report, json_out: bool, code: i32) -> Outcome {
    Outcome {
        stdout: if json_out {
            serde_json::to_string_pretty(r).unwrap_or_default()
        } else {
            r.human()
        },
        code,
    }
}

fn hash_from(s: &str) -> Option<WorldHash> {
    let b = unhex(s)?;
    let arr: [u8; 16] = b.try_into().ok()?;
    Some(WorldHash(arr))
}

/// `check.command_failed {command, status, error, tail}`: the child failed, or printed what a
/// check cannot read.
fn child_failed(
    command: &str,
    status: Option<i32>,
    error: Value,
    tail: &str,
    why: &str,
) -> Problem {
    Problem::new(
        "check.command_failed",
        format!("{command} {why}"),
        detail([
            ("command", json!(command)),
            ("status", json!(status)),
            ("error", error),
            ("tail", json!(tail)),
        ]),
    )
}

/// What a child printed and how it ended.
struct ChildOut {
    command: String,
    status: Option<i32>,
    tail: String,
    json: Value,
}

impl ChildOut {
    fn malformed(&self, why: &str) -> Problem {
        child_failed(&self.command, self.status, Value::Null, &self.tail, why)
    }
}

/// Runs this binary again and reads what it printed ([`read_child`]).
fn rerun(args: &[String], ok: &[i32]) -> Result<ChildOut, Problem> {
    let command = format!("pocket {}", args.join(" "));
    let exe = std::env::current_exe().map_err(|e| {
        child_failed(
            &command,
            None,
            Value::Null,
            "",
            &format!("has no executable: {e}"),
        )
    })?;
    let out = Command::new(&exe)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| {
            let why = format!("could not start {}: {e}", exe.display());
            child_failed(&command, None, Value::Null, "", &why)
        })?;
    read_child(command, out.status.code(), &out.stdout, &out.stderr, ok)
}

/// A child's output as JSON. An exit status outside `ok`, or JSON with an `error` (a load failure,
/// a panic's `internal.error`), is `check.command_failed` carrying the child's own error, so a
/// failing child never reads as a divergence.
fn read_child(
    command: String,
    status: Option<i32>,
    stdout: &[u8],
    stderr: &[u8],
    ok: &[i32],
) -> Result<ChildOut, Problem> {
    let tail: String = {
        let text = String::from_utf8_lossy(stderr);
        let lines: Vec<&str> = text.lines().collect();
        lines[lines.len().saturating_sub(20)..].join("\n")
    };
    let json: Value = serde_json::from_slice(stdout).map_err(|e| {
        let why = format!("printed no JSON (exit {status:?}): {e}");
        child_failed(&command, status, Value::Null, &tail, &why)
    })?;
    if let Some(error) = json.get("error") {
        let message = error["message"].as_str().unwrap_or("no message").to_owned();
        let why = format!("failed (exit {status:?}): {message}");
        return Err(child_failed(&command, status, error.clone(), &tail, &why));
    }
    if !status.is_some_and(|c| ok.contains(&c)) {
        let why = format!("exited with {status:?}");
        return Err(child_failed(&command, status, Value::Null, &tail, &why));
    }
    Ok(ChildOut {
        command,
        status,
        tail,
        json,
    })
}

/// A child's chain, every item `[tick, hash]` or `check.command_failed` naming the first that is
/// not.
fn chain_of(out: &ChildOut) -> Result<Vec<(TickRef, WorldHash)>, Problem> {
    let items = out.json["chain"]
        .as_array()
        .ok_or_else(|| out.malformed("printed no \"chain\" array"))?;
    items
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let tick = p.get(0).and_then(Value::as_u64);
            let hash = p.get(1).and_then(Value::as_str).and_then(hash_from);
            match (tick, hash) {
                (Some(t), Some(h)) => Ok((runs::at(t), h)),
                _ => Err(out.malformed(&format!("printed chain item {i} {p}, not [tick, hash]"))),
            }
        })
        .collect()
}

/// The child process of the cross-process variants: this binary run again.
fn child() -> Child {
    Arc::new(|subject: &Subject, req: &ChildRequest| {
        let dir = subject.dir.display().to_string();
        match req {
            ChildRequest::Chain { seed } => {
                let out = rerun(
                    &[
                        "check".into(),
                        dir,
                        "--chain-only".into(),
                        "--seeds".into(),
                        seed.to_string(),
                        "--json".into(),
                    ],
                    &[0],
                )?;
                Ok(ChildAnswer::Chain(chain_of(&out)?))
            }
            ChildRequest::Snapshot { seed, tick } => {
                let out = rerun(
                    &[
                        "check".into(),
                        dir,
                        "--chain-only".into(),
                        "--seeds".into(),
                        seed.to_string(),
                        "--snapshot-at".into(),
                        tick.to_string(),
                        "--json".into(),
                    ],
                    &[0],
                )?;
                let bytes = out.json["snapshot"]
                    .as_str()
                    .and_then(unhex)
                    .ok_or_else(|| out.malformed("printed no snapshot"))?;
                Ok(ChildAnswer::Snapshot(Snapshot::from_bytes(&bytes)?))
            }
            ChildRequest::Verify { replay } => {
                // A replay that is not identical exits 1 with its summary, which is an answer.
                let out = rerun(
                    &[
                        "replay".into(),
                        "--verify".into(),
                        replay.display().to_string(),
                        "--json".into(),
                    ],
                    &[0, 1],
                )?;
                let s: VerifySummary = serde_json::from_value(out.json.clone())
                    .map_err(|e| out.malformed(&format!("printed no verify summary: {e}")))?;
                Ok(ChildAnswer::Verify(Box::new(s)))
            }
        }
    })
}

fn seeds_of(args: &Args) -> Result<Option<Vec<u64>>, Problem> {
    args.list("seeds")
        .map(|l| {
            l.iter()
                .map(|s| {
                    s.parse::<u64>().map_err(|_| {
                        crate::cli::usage(format!("--seeds takes numbers, not '{s}'"), "seeds", &[])
                    })
                })
                .collect()
        })
        .transpose()
}

/// `pocket check <project> [--json] [--seeds N,...] [--only CHECK,...]`, and the child forms
/// `--chain-only [--snapshot-at T]`.
pub fn check(raw: &[String]) -> Outcome {
    let command = format!("pocket check {}", raw.join(" "));
    let refuse = |p: Problem, json_out: bool| report_out(&Report::usage(&command, p), json_out, 2);
    let json_out = raw.iter().any(|a| a == "--json");
    let args = match Args::parse(raw, CHECK_FLAGS) {
        Ok(a) => a,
        Err(p) => return refuse(p, json_out),
    };
    let Some(dir) = args.positional.first().map(PathBuf::from) else {
        return refuse(
            Problem::new(
                "check.usage",
                "pocket check needs a project: pocket check <project> [--json] [--seeds N,...] [--only CHECK,...]",
                detail([("checks", json!(CHECKS))]),
            ),
            json_out,
        );
    };
    let seeds = match seeds_of(&args) {
        Ok(s) => s,
        Err(p) => return refuse(p, json_out),
    };
    let only = args.list("only");
    if let Some(bad) = only
        .iter()
        .flatten()
        .find(|c| !CHECKS.contains(&c.as_str()))
    {
        return refuse(unknown_check(bad), json_out);
    }
    if args.has("chain-only") {
        return chain_only(&dir, seeds.as_deref(), &args);
    }
    // A malformed or misspelt check.toml refuses the invocation before any check runs (checks.md
    // 11: exit code 2, one step named `check`); `types` alone does not read it.
    let needs_toml = only.as_ref().is_none_or(|o| o.iter().any(|c| c != "types"));
    if needs_toml
        && let Err(p) = pocket_check::CheckToml::load(&dir)
        && p.code == "check.config_invalid"
    {
        return refuse(p, json_out);
    }
    let opts = Options {
        seeds,
        only,
        child: Some(child()),
        out_dir: args.value("out").map(PathBuf::from),
        command,
        clock: Some(clock()),
    };
    let report = check_project(&dir, &opts);
    let code = report.exit_code();
    report_out(&report, json_out, code)
}

fn problem_out(p: &Problem, code: i32) -> Outcome {
    Outcome {
        stdout: serde_json::to_string(&json!({"error": p})).unwrap_or_default(),
        code,
    }
}

/// The child form: the hash chain of a run of the first seed, or its snapshot at a tick.
fn chain_only(dir: &Path, seeds: Option<&[u64]>, args: &Args) -> Outcome {
    let subject = match Subject::load(dir) {
        Ok(s) => s,
        Err(p) => return problem_out(&p, 1),
    };
    let seed = seeds
        .and_then(|s| s.first().copied())
        .unwrap_or(subject.config.run.seeds[0]);
    match args.number::<u64>("snapshot-at") {
        Err(p) => problem_out(&p, 2),
        Ok(Some(t)) => match runs::run_to(&subject, seed, t).and_then(|g| g.snapshot()) {
            Ok(s) => Outcome {
                stdout: json!({"seed": seed, "tick": t, "snapshot": hex(&s.to_bytes())})
                    .to_string(),
                code: 0,
            },
            Err(p) => problem_out(&p, 1),
        },
        Ok(None) => match runs::chain(&subject, seed, subject.ticks()) {
            Ok(c) => {
                let items: Vec<Value> = c
                    .iter()
                    .map(|(at, h)| json!([at.tick.0, h.to_string()]))
                    .collect();
                Outcome {
                    stdout: json!({"seed": seed, "chain": items}).to_string(),
                    code: 0,
                }
            }
            Err(p) => problem_out(&p, 1),
        },
    }
}

/// `pocket replay --verify <file> [--json]`: replays the recording in this process and compares
/// every tick's hash (replay.md 3.4).
pub fn replay(raw: &[String]) -> Outcome {
    let args = match Args::parse(raw, &[("verify", true), ("json", false)]) {
        Ok(a) => a,
        Err(p) => return problem_out(&p, 2),
    };
    let Some(path) = args.value("verify") else {
        return problem_out(
            &crate::cli::usage(
                "pocket replay --verify <file> [--json]".into(),
                "verify",
                &["verify"],
            ),
            2,
        );
    };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            return problem_out(
                &Problem::new(
                    "replay.unreadable",
                    format!("{path}: {e}"),
                    detail([("path", json!(path))]),
                ),
                1,
            );
        }
    };
    match verify_bytes(&bytes) {
        Ok(s) => {
            let code = if s.identical { 0 } else { 1 };
            let stdout = if args.has("json") {
                serde_json::to_string(&s).unwrap_or_default()
            } else if s.identical {
                format!("identical: {} ticks replayed", s.ticks_run)
            } else {
                format!(
                    "not identical after {} ticks: {}",
                    s.ticks_run,
                    serde_json::to_string(&s).unwrap_or_default()
                )
            };
            Outcome { stdout, code }
        }
        Err(p) => problem_out(&p, 1),
    }
}

/// `pocket hashes <project> [--seed S] [--ticks N] [--inputs FILE]`: the world hash after every
/// tick of a run as JSON (index = tick), which the web build compares with its own.
pub fn hashes(raw: &[String]) -> Outcome {
    let args = match Args::parse(raw, &[("seed", true), ("ticks", true), ("inputs", true)]) {
        Ok(a) => a,
        Err(p) => return problem_out(&p, 2),
    };
    let Some(dir) = args.positional.first().map(PathBuf::from) else {
        return problem_out(
            &crate::cli::usage(
                "pocket hashes <project> [--seed S] [--ticks N] [--inputs FILE]".into(),
                "",
                &[],
            ),
            2,
        );
    };
    let run = || -> Result<Value, Problem> {
        let project = pocket_runtime::Project::load(&dir)?;
        let setup = Arc::new(project.setup(false)?);
        let seed = args.number::<u64>("seed")?.unwrap_or(project.manifest.seed);
        let ticks = args.number::<u64>("ticks")?.unwrap_or(600);
        let inputs = pocket_check::Inputs::load(&dir, args.value("inputs"))?;
        let mut g = pocket_runtime::Game::new(setup, seed)?;
        let mut out = vec![json!(g.world_hash()?.to_string())];
        for _ in 0..ticks {
            out.push(json!(runs::advance(&mut g, &inputs)?.to_string()));
        }
        Ok(
            json!({"project": dir.display().to_string().replace('\\', "/"), "seed": seed,
                  "ticks": ticks, "bundle": g.bundle().to_hex(), "hashes": out}),
        )
    };
    match run() {
        Ok(v) => Outcome {
            stdout: v.to_string(),
            code: 0,
        },
        Err(p) => problem_out(&p, 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(status: i32, stdout: &str) -> Result<ChildOut, Problem> {
        read_child(
            "pocket check x".into(),
            Some(status),
            stdout.as_bytes(),
            b"why",
            &[0],
        )
    }

    #[test]
    fn a_failing_child_is_a_command_failure_with_its_own_error() {
        let e = read(
            1,
            r#"{"error": {"code": "internal.error", "message": "pocket panicked"}}"#,
        )
        .err()
        .unwrap();
        assert_eq!(e.code, "check.command_failed");
        assert_eq!(e.detail["error"]["code"], json!("internal.error"));
        assert_eq!(e.detail["status"], json!(1));
        assert_eq!(e.detail["tail"], json!("why"));
        // A refusal at exit 0 is still a failure; so is an exit status outside the expected.
        assert!(read(0, r#"{"error": {"code": "x.y", "message": "no"}}"#).is_err());
        assert!(read(3, r#"{"chain": []}"#).is_err());
        assert!(read(0, "not json").is_err());
    }

    #[test]
    fn a_chain_is_read_strictly() {
        let h = "00112233445566778899aabbccddeeff";
        let out = read(0, &format!(r#"{{"chain": [[1, "{h}"], [2, "{h}"]]}}"#)).unwrap();
        assert_eq!(chain_of(&out).unwrap().len(), 2);
        for bad in [
            r#"{"seed": 1}"#.to_owned(),
            format!(r#"{{"chain": [[1, "{h}"], [2, "zz"]]}}"#),
            format!(r#"{{"chain": [["one", "{h}"]]}}"#),
        ] {
            let out = read(0, &bad).unwrap();
            let e = chain_of(&out).unwrap_err();
            assert_eq!(e.code, "check.command_failed", "{bad}");
        }
    }
}
