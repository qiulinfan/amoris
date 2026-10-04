//! The native binary `pocket` (docs/spec/architecture.md 4.13). Slice 1's subcommands:
//!
//! - `pocket run <project> --headless [--ticks N] [--seed S] [--inputs FILE] [--json]`
//! - `pocket check <project> [--json] [--seeds N,...] [--only CHECK,...]` (checks.md 2 and 8)
//! - `pocket replay --verify <file> [--json]` (checks.md 8.4)
//! - `pocket hashes <project> [--seed S] [--ticks N] [--inputs FILE]`: the hash after every tick
//!   as JSON, which the web build compares against
//!
//! Everything runs on a thread whose stack holds the script host's limit (script-sandbox.md 4.3).

mod check;
mod cli;
mod run;

use check::Outcome;

const SUBCOMMANDS: &[&str] = &["run", "check", "replay", "hashes"];

fn dispatch(args: Vec<String>) -> Outcome {
    let rest = args.get(1..).map(<[String]>::to_vec).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("run") => run::run(&rest),
        Some("check") => check::check(&rest),
        Some("replay") => check::replay(&rest),
        Some("hashes") => check::hashes(&rest),
        other => {
            let name = other.unwrap_or("");
            let p = cli::usage(
                format!(
                    "pocket has no subcommand '{name}'; it has {SUBCOMMANDS:?}: pocket run|check|replay|hashes ..."
                ),
                name,
                SUBCOMMANDS,
            );
            Outcome {
                stdout: serde_json::to_string_pretty(&serde_json::json!({"error": p}))
                    .unwrap_or_default(),
                code: 2,
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let worker = std::thread::Builder::new()
        .name("pocket-main".into())
        .stack_size(pocket_runtime::GAME_STACK_BYTES)
        .spawn(move || dispatch(args));
    let outcome = match worker.map(|h| h.join()) {
        Ok(Ok(o)) => o,
        Ok(Err(_)) => Outcome {
            stdout: "{\"error\": {\"code\": \"internal.error\", \"message\": \"pocket panicked\"}}"
                .into(),
            code: 1,
        },
        Err(e) => Outcome {
            stdout: format!(
                "{{\"error\": {{\"code\": \"internal.error\", \"message\": \"{e}\"}}}}"
            ),
            code: 1,
        },
    };
    println!("{}", outcome.stdout);
    std::process::exit(outcome.code);
}
