//! `pocket serve <project> [--port 7878]` and `pocket mcp <project>` (docs/spec/server.md): the
//! project's game on its game thread (real time, paused: the edit world) with the host's server on
//! 127.0.0.1, or one MCP session over stdio, against a host already serving the project when there
//! is one and otherwise against a game of its own.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use pocket_contract::{Problem, detail};
use pocket_link::Source;
use pocket_mcp::{Backend, BoxFuture, Caller};
use pocket_runtime::thread::{GameHandle, GameThread, Pacing, ThreadOptions};
use pocket_runtime::{GameBuilder, Project};
use pocket_server::{GameAccess, Host, McpBackend, ServeOptions, hostfile};
use serde_json::{Value, json};

use crate::check::Outcome;
use crate::cli::{Args, Flag};

const SERVE_FLAGS: &[Flag] = &[("port", true), ("editor", true), ("seed", true)];
const MCP_FLAGS: &[Flag] = &[("seed", true), ("own", false)];

fn fail(p: &Problem, code: i32) -> Outcome {
    Outcome {
        stdout: serde_json::to_string_pretty(&json!({"error": p})).unwrap_or_default(),
        code,
    }
}

fn project_dir(args: &Args) -> Result<PathBuf, Problem> {
    let dir = PathBuf::from(args.positional.first().map_or(".", String::as_str));
    dir.canonicalize().map_err(|e| {
        Problem::new(
            "project.missing_file",
            format!("{} is not a project directory: {e}.", dir.display()),
            detail([("path", json!(dir.display().to_string()))]),
        )
    })
}

/// The project's game on its thread, real time and paused, and the host over it.
fn start(
    root: &Path,
    seed: Option<u64>,
    editor: Option<PathBuf>,
) -> Result<(GameHandle, Host), Problem> {
    let feed = pocket_assets::Feed::new();
    let project = Project::load(root)?;
    let seed = seed.unwrap_or(project.manifest.seed);
    let setup = Arc::new(project.setup(false)?);
    let start = Instant::now();
    let clock: pocket_runtime::thread::Clock =
        Arc::new(move || start.elapsed().as_secs_f64() * 1000.0);
    let options = ThreadOptions {
        pacing: Pacing::RealTime { speed: 1.0 },
        paused: true,
        feed: Some(feed.clone()),
        ..ThreadOptions::new(clock)
    };
    let dir = root.to_path_buf();
    let handle = GameThread::spawn(
        move || GameBuilder::new(setup).seed(seed).project(dir).build(),
        options,
    )?;
    let clients = handle.clients();
    let access = GameAccess {
        reader: handle.reader(),
        editor: clients.client(Source::Editor)?,
        api: clients.developer(),
        developer: Arc::new(move || clients.developer()),
    };
    let host = Host::new(access, root.to_path_buf(), editor);
    host.set_capture(Arc::new(crate::present::CaptureServer::start(&feed, root.to_path_buf())));
    host.set_render(Arc::new(crate::present::FeedServer { feed }));
    Ok((handle, host))
}

fn runtime() -> Result<tokio::runtime::Runtime, Problem> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("pocket-server")
        .build()
        .map_err(|e| Problem::new("host.runtime", e.to_string(), detail([])))
}

async fn stopped() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

/// `pocket serve`.
pub fn serve(raw: &[String]) -> Outcome {
    let args = match Args::parse(raw, SERVE_FLAGS) {
        Ok(a) => a,
        Err(p) => return fail(&p, 2),
    };
    match serving(&args) {
        Ok(()) => Outcome {
            stdout: String::new(),
            code: 0,
        },
        Err(p) => fail(&p, 1),
    }
}

fn serving(args: &Args) -> Result<(), Problem> {
    let root = project_dir(args)?;
    let port = args.number::<u16>("port")?.unwrap_or(7878);
    let editor = args
        .value("editor")
        .map(PathBuf::from)
        .or_else(pocket_server::find_editor_dist);
    let (handle, host) = start(&root, args.number::<u64>("seed")?, editor.clone())?;
    let rt = runtime()?;
    let r = rt.block_on(async {
        let (addr, task) = host.serve(&ServeOptions { port }, stopped()).await?;
        let file = hostfile::write(&root, addr.port())
            .map_err(|e| Problem::new("host.file", e.to_string(), detail([])))?;
        println!(
            "{}",
            json!({"url": file.url, "port": addr.port(), "pid": file.pid,
                   "project": file.project, "mcp": format!("{}/mcp", file.url),
                   "editor": editor.as_ref().map(|e| e.display().to_string())})
        );
        eprintln!(
            "pocket: serving {} at {} (editor {}, MCP {}/mcp); Ctrl-C stops",
            root.display(),
            file.url,
            if editor.is_some() {
                "built"
            } else {
                "not built"
            },
            file.url
        );
        let _ = task.await;
        Ok::<(), Problem>(())
    });
    hostfile::remove(&root);
    rt.shutdown_timeout(std::time::Duration::from_secs(2));
    drop(host);
    let _ = handle.shutdown(2000);
    r
}

/// A host serving the project over HTTP, as `pocket mcp` reaches it.
struct Remote {
    url: String,
}

impl Caller for Remote {
    fn call(&self, method: String, params: Value) -> BoxFuture<Result<Value, Problem>> {
        let url = self.url.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || crate::client::call_url(&url, &method, params))
                .await
                .unwrap_or_else(|e| {
                    Err(Problem::new("host.unreachable", e.to_string(), detail([])))
                })
        })
    }
}

impl Backend for Remote {
    fn open(&self) -> Arc<dyn Caller> {
        Arc::new(Remote {
            url: self.url.clone(),
        })
    }
}

/// `pocket mcp`.
pub fn mcp(raw: &[String]) -> Outcome {
    let args = match Args::parse(raw, MCP_FLAGS) {
        Ok(a) => a,
        Err(p) => return fail(&p, 2),
    };
    match mcp_session(&args) {
        Ok(()) => Outcome {
            stdout: String::new(),
            code: 0,
        },
        Err(p) => {
            eprintln!("{}", json!({"error": p}));
            Outcome {
                stdout: String::new(),
                code: 1,
            }
        }
    }
}

fn mcp_session(args: &Args) -> Result<(), Problem> {
    let root = project_dir(args)?;
    let rt = runtime()?;
    let running = (!args.has("own"))
        .then(|| hostfile::read(&hostfile::path(&root)))
        .flatten()
        .filter(|f| crate::client::call_url(&f.url, "status", json!({})).is_ok());
    if let Some(f) = running {
        eprintln!("pocket mcp: using the host at {}", f.url);
        let backend: Arc<dyn Backend> = Arc::new(Remote { url: f.url });
        return rt.block_on(pocket_mcp::serve_stdio(backend));
    }
    let (handle, host) = start(&root, args.number::<u64>("seed")?, None)?;
    eprintln!("pocket mcp: running {} in this process", root.display());
    let backend: Arc<dyn Backend> = Arc::new(McpBackend::new(host.clone()));
    let r = rt.block_on(pocket_mcp::serve_stdio(backend));
    rt.shutdown_timeout(std::time::Duration::from_secs(2));
    drop(host);
    let _ = handle.shutdown(2000);
    r
}
