//! The native binary `pocket` (docs/spec/architecture.md 4.13). Slice 1's subcommands:
//!
//! - `pocket run <project> --headless [--ticks N] [--seed S] [--inputs FILE] [--json]`
//! - `pocket check <project> [--json] [--seeds N,...] [--only CHECK,...]` (checks.md 2 and 8)
//! - `pocket replay --verify <file> [--json]` (checks.md 8.4)
//! - `pocket hashes <project> [--seed S] [--ticks N] [--inputs FILE]`: the hash after every tick
//!   as JSON, which the web build compares against
//!
//! - `pocket play <project>`: the game in a native window (window.rs)
//! - `pocket serve <project> [--port 7878]`: the project's game (real time, paused) with the host's
//!   server on 127.0.0.1 (docs/spec/server.md); `pocket mcp <project>`: MCP over stdio
//! - the client of a running host: `pocket call <method> '<json>'`, `pocket status`, `pocket world
//!   ...`, `pocket step ...`, `pocket play start|stop`, `pocket undo`, ... (`pocket help`)
//!
//! Everything runs on a thread whose stack holds the script host's limit (script-sandbox.md 4.3).

mod check;
mod cli;
mod client;
mod present;
mod run;
mod serve;
mod window;

use check::Outcome;

const SUBCOMMANDS: &[&str] = &[
    "run",
    "play",
    "check",
    "replay",
    "hashes",
    "serve",
    "mcp",
    "call",
    "status",
    "world",
    "step",
    "time",
    "undo",
    "redo",
    "history",
    "scripts",
    "events",
    "logs",
    "snapshots",
    "help",
];

fn dispatch(args: Vec<String>) -> Outcome {
    let rest = args.get(1..).map(<[String]>::to_vec).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("run") => run::run(&rest),
        Some("check") => check::check(&rest),
        Some("replay") => check::replay(&rest),
        Some("hashes") => check::hashes(&rest),
        Some("serve") => serve::serve(&rest),
        Some("mcp") => serve::mcp(&rest),
        Some(cmd) if client::is_client(cmd) => client::run(cmd, &rest),
        None => client::run("help", &rest),
        other => {
            let name = other.unwrap_or("");
            let s = pocket_contract::suggest_names(name, SUBCOMMANDS.iter().copied());
            let p = cli::usage(
                format!("pocket has no subcommand '{name}'; did you mean {s:?}? (pocket help)"),
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
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        if let Some(game) = std::env::current_exe()
            .ok()
            .as_deref()
            .and_then(bundled_game)
        {
            args = vec![
                "play".into(),
                game.to_string_lossy().into_owned(),
                "--walk".into(),
                "--vsync".into(),
            ];
        }
    }
    // A window's event loop must run on the main thread (macOS); its game runs on its own thread.
    // `pocket play start|stop` is the client's Play of a running host; `pocket play <project>` opens
    // a window.
    let client_play = matches!(args.get(1).map(String::as_str), Some("start" | "stop"));
    if args.first().map(String::as_str) == Some("play") && !client_play {
        let outcome = window::play(&args[1..]);
        if !outcome.stdout.is_empty() {
            println!("{}", outcome.stdout);
        }
        std::process::exit(outcome.code);
    }
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
    if !outcome.stdout.is_empty() {
        println!("{}", outcome.stdout);
    }
    std::process::exit(outcome.code);
}

/// An app bundle carries the project beside MacOS/ in Contents/Resources/game/.
/// Plain CLI binaries keep their no-argument help behavior.
fn bundled_game(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    let game = exe.parent()?.parent()?.join("Resources/game");
    game.join("project.toml").is_file().then_some(game)
}

#[cfg(test)]
mod bundle_tests {
    use super::*;

    #[test]
    fn no_argument_bundle_discovery_requires_a_project_beside_the_executable() {
        let root = std::env::temp_dir().join(format!(
            "amoris bundle test {} {}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let exe = root.join("Test Town.app/Contents/MacOS/pocket");
        assert!(bundled_game(&exe).is_none());
        let game = root.join("Test Town.app/Contents/Resources/game");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("project.toml"), "name = 'Town'\n").unwrap();
        assert_eq!(bundled_game(&exe), Some(game));
        assert!(bundled_game(&root.join("bin/pocket")).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
