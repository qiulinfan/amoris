//! The native binary `amoris` (docs/spec/architecture.md 4.13). Slice 1's subcommands:
//!
//! - `amoris run <project> --headless [--ticks N] [--seed S] [--inputs FILE] [--json]`
//! - `amoris check <project> [--json] [--seeds N,...] [--only CHECK,...]` (checks.md 2 and 8)
//! - `amoris replay --verify <file> [--json]` (checks.md 8.4)
//! - `amoris hashes <project> [--seed S] [--ticks N] [--inputs FILE]`: the hash after every tick
//!   as JSON, which the web build compares against
//! - `amoris version`: the engine version every replay, snapshot and save records, as JSON
//!   (versions.md 3.1; computed by the build script, `crates/engine_version.rs`)
//!
//! - `amoris play <project>`: the game in a native window (window.rs)
//! - `amoris serve <project> [--port 7878]`: the project's game (real time, paused) with the host's
//!   server on 127.0.0.1 (docs/spec/server.md); `amoris mcp <project>`: MCP over stdio
//! - the client of a running host: `amoris call <method> '<json>'`, `amoris status`, `amoris world
//!   ...`, `amoris step ...`, `amoris play start|stop`, `amoris undo`, ... (`amoris help`)
//!
//! Everything runs on a thread whose stack holds the script host's limit (script-sandbox.md 4.3).
//!
//! On Windows the binary is a GUI program, so a double-clicked game opens no console window; a
//! subcommand run from a terminal writes to that terminal's console (`attach_parent_console`).

#![cfg_attr(windows, windows_subsystem = "windows")]

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
    "version",
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
    "profile",
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
        Some("version") => version(),
        Some("serve") => serve::serve(&rest),
        Some("mcp") => serve::mcp(&rest),
        Some(cmd) if client::is_client(cmd) => client::run(cmd, &rest),
        None => client::run("help", &rest),
        other => {
            let name = other.unwrap_or("");
            let s = amoris_contract::suggest_names(name, SUBCOMMANDS.iter().copied());
            let p = cli::usage(
                format!("amoris has no subcommand '{name}'; did you mean {s:?}? (amoris help)"),
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

/// `amoris version`: the installed `EngineVersion`, its source as 64 hex digits.
fn version() -> Outcome {
    let v = amoris_runtime::EngineVersion::current();
    let json = serde_json::json!({
        "semver": v.semver,
        "commit": v.commit,
        "source": v.source.to_hex(),
        "target": v.target,
        "profile": v.profile,
        "contract": v.contract,
        "c_compiler": v.c_compiler,
    });
    Outcome {
        stdout: serde_json::to_string_pretty(&json).unwrap_or_default(),
        code: 0,
    }
}

/// A Windows GUI program starts with no console. Run from a terminal, it takes the terminal's
/// console for whichever standard handles are not already redirected (to a pipe or a file), so the
/// subcommands print as before. The terminal does not wait for a GUI program: its prompt may come
/// back before the output.
#[cfg(windows)]
fn attach_parent_console() {
    use std::ffi::c_void;
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    const STD_INPUT_HANDLE: u32 = -10i32 as u32;
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const FILE_SHARE_READ: u32 = 1;
    const FILE_SHARE_WRITE: u32 = 2;
    const OPEN_EXISTING: u32 = 3;
    const INVALID_HANDLE_VALUE: *mut c_void = -1isize as *mut c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn AttachConsole(process: u32) -> i32;
        fn GetStdHandle(which: u32) -> *mut c_void;
        fn SetStdHandle(which: u32, handle: *mut c_void) -> i32;
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            security: *const c_void,
            disposition: u32,
            flags: u32,
            template: *mut c_void,
        ) -> *mut c_void;
    }
    let missing = |which: u32| {
        // SAFETY: GetStdHandle only reads the process's standard handle table.
        let h = unsafe { GetStdHandle(which) };
        h.is_null() || h == INVALID_HANDLE_VALUE
    };
    let (input, output, error) = (
        missing(STD_INPUT_HANDLE),
        missing(STD_OUTPUT_HANDLE),
        missing(STD_ERROR_HANDLE),
    );
    if !(input || output || error) {
        return;
    }
    // SAFETY: attaching fails harmlessly when the parent has no console (a double-click).
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } == 0 {
        return;
    }
    let open = |name: &str, access: u32| {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: a valid NUL-terminated name; the handle stays open for the process's life.
        unsafe {
            CreateFileW(
                wide.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        }
    };
    let set = |which: u32, handle: *mut c_void| {
        if handle != INVALID_HANDLE_VALUE {
            // SAFETY: installs a console handle opened above as this process's standard handle.
            unsafe { SetStdHandle(which, handle) };
        }
    };
    if output || error {
        let out = open("CONOUT$", GENERIC_READ | GENERIC_WRITE);
        if output {
            set(STD_OUTPUT_HANDLE, out);
        }
        if error {
            set(STD_ERROR_HANDLE, out);
        }
    }
    if input {
        set(
            STD_INPUT_HANDLE,
            open("CONIN$", GENERIC_READ | GENERIC_WRITE),
        );
    }
}

fn main() {
    #[cfg(windows)]
    attach_parent_console();
    // Before anything records: replays carry this source, and `Verify` and the reuse of a
    // recorded bundle's compiled modules compare it (versions.md 3.1, replay.md 2.4).
    if let Err(v) = amoris_runtime::install_engine_version!() {
        eprintln!("warning: the engine version was not installed; {v:?} is in place");
    }
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty()
        && let Some(game) = std::env::current_exe()
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
    // A window's event loop must run on the main thread (macOS); its game runs on its own thread.
    // `amoris play start|stop` is the client's Play of a running host; `amoris play <project>` opens
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
        .name("amoris-main".into())
        .stack_size(amoris_runtime::GAME_STACK_BYTES)
        .spawn(move || dispatch(args));
    let outcome = match worker.map(|h| h.join()) {
        Ok(Ok(o)) => o,
        Ok(Err(_)) => Outcome {
            stdout: "{\"error\": {\"code\": \"internal.error\", \"message\": \"amoris panicked\"}}"
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
        let exe = root.join("Test Town.app/Contents/MacOS/amoris");
        assert!(bundled_game(&exe).is_none());
        let game = root.join("Test Town.app/Contents/Resources/game");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("project.toml"), "name = 'Town'\n").unwrap();
        assert_eq!(bundled_game(&exe), Some(game));
        assert!(bundled_game(&root.join("bin/amoris")).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
