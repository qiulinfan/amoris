//! `cargo xtask`: the local check command, the definition of done (charter 3.7; docs/spec/checks.md),
//! code generation (`gen`) and the web checker (`webcheck`). It links no engine crate: it drives
//! cargo, the built `pocket` binary and Chrome as processes (architecture.md 4.15).

mod cli;
mod config;
mod files;
mod machine;
mod report;
mod run;
mod serve;
mod steps;

use report::{Problem, Report, StepResult, Verdict};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const COMMANDS: &[&str] = &["check", "gen", "webcheck"];

/// The workspace root: the parent of xtask's manifest directory, which `cargo run` also passes at
/// run time, so the built binary can be run directly (as `CARGO_MANIFEST_DIR=<root>/xtask xtask
/// check`) while a broken member manifest keeps `cargo xtask` itself from starting.
fn root() -> PathBuf {
    let dir = std::env::var_os("CARGO_MANIFEST_DIR")
        .map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")), PathBuf::from);
    dir.parent()
        .expect("xtask sits in the workspace root")
        .to_path_buf()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = root();
    match args.first().map(String::as_str) {
        Some("check") => check(&root, &args[1..]),
        Some("gen") => generate(&root, &args[1..]),
        Some("webcheck") => {
            let env = run::Env::new(&root);
            match steps::web::main(&env, &args[1..]) {
                Ok(true) => ExitCode::SUCCESS,
                Ok(false) => ExitCode::from(1),
                Err(p) => usage(&p),
            }
        }
        Some(other) => usage(&cli::unknown("command", other, COMMANDS)),
        None => {
            eprintln!(
                "cargo xtask check [--json] [--quick] [--only STEP,...] [--skip STEP,...] [--jobs N] [--record]"
            );
            eprintln!("cargo xtask gen [--check]");
            eprintln!("cargo xtask webcheck <page> --serve DIR [--isolation on|off] [--timeout S]");
            ExitCode::from(2)
        }
    }
}

fn usage(p: &Problem) -> ExitCode {
    eprintln!("{}: {}", p.code, p.message);
    ExitCode::from(2)
}

fn generate(root: &Path, args: &[String]) -> ExitCode {
    let opts = match cli::parse_gen(args) {
        Ok(o) => o,
        Err(p) => return usage(&p),
    };
    match steps::generate::main(root, &opts) {
        Ok(problems) if problems.is_empty() => ExitCode::SUCCESS,
        Ok(problems) => {
            for p in &problems {
                eprintln!("{}: {}", p.code, p.message);
            }
            ExitCode::from(1)
        }
        Err(p) => usage(&p),
    }
}

fn check(root: &Path, args: &[String]) -> ExitCode {
    let started_utc = rfc3339(SystemTime::now());
    let start = Instant::now();
    let command = format!("cargo xtask check {}", args.join(" "))
        .trim_end()
        .to_string();
    let json = args.iter().any(|a| a == "--json");
    let env = run::Env::new(root);
    let report = |steps: Vec<StepResult>, verdict: Verdict| Report {
        format: 1,
        command: command.clone(),
        commit: commit(root),
        machine: machine::fingerprint(root),
        started_utc: started_utc.clone(),
        duration_ms: start.elapsed().as_millis() as u64,
        verdict,
        steps,
    };
    // Exit code 2: the command could not run as asked (checks.md 11), before any step runs.
    let refused = |p: Problem| {
        let mut step = StepResult::new("check");
        step.summary = p.message.clone();
        step.error(p);
        let r = report(vec![step], Verdict::Fail);
        print(&r, json, &env);
        ExitCode::from(2)
    };
    let opts = match cli::parse_check(args) {
        Ok(o) => o,
        Err(p) => return refused(p),
    };
    let prep = match steps::prepare(&env) {
        Ok(p) => p,
        Err(p) => return refused(p),
    };
    let results = steps::run(&env, &opts, &prep);
    let verdict = report::overall(&results);
    let r = report(results, verdict);
    print(&r, json, &env);
    if opts.record {
        let mode = match (opts.only.is_some() || !opts.skip.is_empty(), opts.quick) {
            (true, _) => "partial",
            (false, true) => "quick",
            (false, false) => "full",
        };
        let line = report::trailer(&r, mode);
        if json {
            eprintln!("{line}")
        } else {
            println!("{line}")
        }
    }
    ExitCode::from(report::exit_code(verdict))
}

/// Writes `out/check/report.json` and prints the report: one JSON document on stdout with
/// `--json`, the human form otherwise (checks.md 10).
fn print(r: &Report, json: bool, env: &run::Env) {
    let text = serde_json::to_string_pretty(r).expect("a report serializes");
    let path = env.out.join("report.json");
    let _ = std::fs::create_dir_all(&env.out);
    let _ = std::fs::write(&path, &text);
    if json {
        println!("{text}");
    } else {
        print!("{}", report::human(r, &env.rel(&path)));
    }
}

/// HEAD, with "+dirty" when the working tree differs from it.
fn commit(root: &Path) -> Option<String> {
    let head = run::capture("git", &["rev-parse", "--short=12", "HEAD"], root)?;
    let dirty =
        run::capture("git", &["status", "--porcelain"], root).is_some_and(|s| !s.trim().is_empty());
    Some(format!(
        "{}{}",
        head.trim(),
        if dirty { "+dirty" } else { "" }
    ))
}

/// RFC 3339 in UTC, from the days-from-civil algorithm (Howard Hinnant's `civil_from_days`).
fn rfc3339(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn rfc3339_dates() {
        assert_eq!(rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            rfc3339(UNIX_EPOCH + Duration::from_secs(951_782_400)),
            "2000-02-29T00:00:00Z"
        );
        assert_eq!(
            rfc3339(UNIX_EPOCH + Duration::from_secs(1_791_072_245)),
            "2026-10-04T00:04:05Z"
        );
    }
}
