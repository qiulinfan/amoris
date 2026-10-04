//! The sailing sample in real time with the script debugger's Chrome DevTools Protocol endpoint
//! (docs/spec/debugger.md 9): attach Chrome DevTools or VS Code (editors/vscode/launch.json) and
//! break in `samples/sailing/scripts/*.ts`.
//!
//! ```sh
//! cargo run -p pocket-app --example debug_sailing -- [--project samples/sailing] [--port 9229]
//!     [--wait] [--speed 1] [--seconds 0] [--file-sources]
//! ```
//!
//! It prints one JSON line with the WebSocket and DevTools URLs, then a status line a second.
//! `--wait` holds the game before its first tick until a client sends
//! `Runtime.runIfWaitingForDebugger` (as `node --inspect-brk`); `--seconds 0` runs until killed;
//! `--file-sources` names the TypeScript in the source maps by absolute `file://` URL instead of
//! `pocket:///`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pocket_debug::{CdpOptions, DebugHub, HubOptions, SourceRoot};
use pocket_link::SnapshotView;
use pocket_runtime::thread::{GameThread, ThreadOptions};
use pocket_runtime::{Game, Project};
use serde_json::json;

fn value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() {
    if let Err(e) = run() {
        eprintln!("debug_sailing: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = value(&args, "--project")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("samples/sailing"));
    let dir = dir
        .canonicalize()
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    let port: u16 = value(&args, "--port")
        .and_then(|p| p.parse().ok())
        .unwrap_or(9229);
    let speed: f64 = value(&args, "--speed")
        .and_then(|p| p.parse().ok())
        .unwrap_or(1.0);
    let seconds: f64 = value(&args, "--seconds")
        .and_then(|p| p.parse().ok())
        .unwrap_or(0.0);
    let wait = args.iter().any(|a| a == "--wait");
    let file_sources = args.iter().any(|a| a == "--file-sources");

    let project = Project::load(&dir).map_err(|p| p.message)?;
    let setup = Arc::new(project.setup(false).map_err(|p| p.message)?);
    let seed = project.manifest.seed;
    let hub = DebugHub::new(HubOptions {
        source_root: if file_sources {
            SourceRoot::Project(dir.clone())
        } else {
            SourceRoot::Relative
        },
        title: format!("Pocket3D: {}", project.manifest.name),
    });
    let cdp = hub
        .serve_cdp(CdpOptions {
            port,
            ..CdpOptions::default()
        })
        .map_err(|e| format!("cannot listen on 127.0.0.1:{port}: {e}"))?;
    println!(
        "{}",
        json!({"ws": cdp.ws_url(), "devtools": cdp.devtools_url(), "project": dir, "wait": wait})
    );

    let start = Instant::now();
    let clock: pocket_runtime::thread::Clock =
        Arc::new(move || start.elapsed().as_secs_f64() * 1000.0);
    let h = hub.clone();
    let handle = GameThread::spawn(
        move || {
            let mut game = Game::new(setup, seed)?;
            game.set_script_debugger(Some(h.hook()));
            Ok(game)
        },
        ThreadOptions::new(clock),
    )
    .map_err(|p| p.message)?;
    let reader = handle.reader();
    let mut dev = handle.developer();
    if wait {
        eprintln!("debug_sailing: waiting for a debugger (Runtime.runIfWaitingForDebugger)");
        hub.wait_for_debugger(None);
    }
    dev.call(
        "time_control",
        json!({"pacing": {"real_time": {"speed": speed}}}),
    )
    .map_err(|p| p.message)?;
    let began = Instant::now();
    while seconds <= 0.0 || began.elapsed().as_secs_f64() < seconds {
        std::thread::sleep(Duration::from_secs(1));
        let snap = reader.latest();
        let view = SnapshotView::new(&snap);
        let boats = view.entities().map(|e| e.len()).unwrap_or(0);
        println!(
            "{}",
            json!({"tick": snap.state().0, "paused_in_debugger": hub.is_paused(), "entities": boats})
        );
    }
    hub.shutdown();
    let _ = dev.call("time_control", json!({"pause": true}));
    drop(dev);
    handle.shutdown(2000).map_err(|p| p.message)?;
    cdp.stop();
    Ok(())
}
