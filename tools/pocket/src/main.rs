//! `pocket`: the Pocket3D build tool.
//!
//! One binary that reads the workspace manifest (`pocket.toml`) and every
//! `module.toml`, resolves the module graph, fetches or builds third-party
//! dependencies into `.pocket/`, writes a Ninja build graph, runs it, and
//! exposes the same operations with `--json` output for agents.

mod check;
mod commands;
mod deps;
mod gen;
mod graph;
mod manifest;
mod mcp;
mod ninja;
mod report;
mod sdkdoc;
mod toolchain;
mod pack;
mod ts;
mod watch;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "pocket", version, about = "Pocket3D build tool")]
struct Cli {
    /// Workspace root (defaults to the nearest ancestor containing pocket.toml).
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    /// Emit one JSON document instead of human-readable text.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Fetch prebuilt dependencies and build foreign (CMake) dependencies into .pocket/.
    Setup {
        /// Re-fetch and rebuild even when the stamp matches.
        #[arg(long)]
        force: bool,
        /// Target to set dependencies up for: native (default) or wasm (Emscripten).
        #[arg(long, default_value = "native")]
        target: String,
    },
    /// Report toolchain and dependency status.
    Doctor,
    /// Generate the Ninja graph and build the requested targets.
    Build {
        /// Module names to build (default: everything).
        targets: Vec<String>,
        #[arg(long, default_value = "debug")]
        config: String,
        /// Only write build.ninja and compile_commands.json.
        #[arg(long)]
        generate_only: bool,
    },
    /// Build and run an executable module or a sample project.
    Run {
        target: String,
        #[arg(long, default_value = "debug")]
        config: String,
        /// Keep running; rebundle and hot reload the project when its sources change.
        #[arg(long)]
        watch: bool,
        /// Arguments passed to the executable after `--`.
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Run a project's perception benchmarks (benches/*.ts): what each answer costs through the instruments.
    Bench {
        /// Project name under samples/ or a directory with project.toml.
        target: String,
        /// Build configuration of the runtime (release by default; debug is sanitized and slower).
        #[arg(long, default_value = "release")]
        config: String,
        /// One benchmark file instead of every file under <project>/benches/.
        #[arg(long)]
        file: Option<String>,
        /// Frame budget per run; a benchmark still running at the end fails.
        #[arg(long, default_value_t = 1800)]
        frames: i64,
        /// Substring filter on questions.
        #[arg(long)]
        only: Option<String>,
    },
    /// Run a project's gameplay scenarios (scenarios/*.ts) at several seeds and summarise.
    Scenario {
        /// Project name under samples/ or a directory with project.toml.
        target: String,
        /// Build configuration of the runtime (release by default; debug is sanitized and slower).
        #[arg(long, default_value = "release")]
        config: String,
        /// One scenario file instead of every file under <project>/scenarios/.
        #[arg(long)]
        file: Option<String>,
        /// How many seeds to run each scenario with (1..N).
        #[arg(long, default_value_t = 5)]
        seeds: u64,
        /// Frame budget per run; a scenario still running at the end fails.
        #[arg(long, default_value_t = 1800)]
        frames: i64,
        /// Substring filter on scenario names.
        #[arg(long)]
        only: Option<String>,
    },
    /// Build and run every test module, then summarise.
    Test {
        #[arg(long, default_value = "debug")]
        config: String,
        /// Substring filter on test module names.
        #[arg(long)]
        filter: Option<String>,
    },
    /// Type-check TypeScript against the SDK's types (TypeScript 7): a project, or the whole
    /// workspace (the SDK, the editor, the TypeScript tests and every sample) without one.
    Check {
        /// Project directory (or a file in it); the workspace when left out.
        project: Option<PathBuf>,
    },
    /// Transform and bundle a TypeScript project into one JavaScript file.
    Ts {
        /// Project directory containing project.toml, or an entry .ts file.
        project: PathBuf,
        /// Output bundle path (default: build/ts/<project>.js).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Remove build outputs (keeps .pocket/ dependencies).
    Clean,
    /// Print the resolved module graph.
    Graph,
    /// Serve the Model Context Protocol over stdio: build, test, run and drive live sessions.
    Mcp,
    /// Send one command to a running runtime's control server and print its JSON result.
    Rpc {
        /// Command name, e.g. world.tree, world.spawn, step, capture (the command `commands` lists them).
        method: String,
        /// Parameters as a JSON object (default {}).
        params: Option<String>,
        /// The control server's base url (default: $POCKET_RPC_URL), e.g. http://127.0.0.1:4711.
        #[arg(long)]
        url: Option<String>,
    },
    /// Create a project: project.toml, a scene, a prefab, assets/ and a first script; or a copy of a sample to start from (--from).
    New {
        name: Option<String>,
        /// Parent directory (default projects/).
        #[arg(long, default_value = "projects")]
        dir: PathBuf,
        /// Start from a sample (its scene, scripts, assets, prefabs and scenarios), renamed; `--list` shows them.
        #[arg(long)]
        from: Option<String>,
        /// List the samples a project can start from.
        #[arg(long)]
        list: bool,
    },
    /// Pack a project into a self-contained folder (dist/<name>) that runs without the repository.
    Pack {
        target: String,
        /// Build configuration for the runtime inside the pack (--web uses the wasm configuration).
        #[arg(long)]
        config: Option<String>,
        /// Output directory (default dist/<name>, or dist/web/<name> with --web).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Also write dist/<name>.zip.
        #[arg(long)]
        zip: bool,
        /// Pack for the browser: a static folder (index.html, the wasm runtime, the packaged project) to host anywhere.
        #[arg(long)]
        web: bool,
        /// With --web: ship the editor too; the page opens the project in the editor, paused.
        #[arg(long)]
        editor: bool,
    },
    /// Open a project in the Pocket editor (a window with the scene, hierarchy, inspector and console).
    Editor {
        target: String,
        #[arg(long, default_value = "debug")]
        config: String,
        /// Rebundle and hot reload the project (and the editor) when sources change.
        #[arg(long)]
        watch: bool,
        /// Arguments passed to the runtime after `--` (for example --serve 7777 or --size 1600x1000).
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Regenerate code from component metadata (C++, TypeScript, docs).
    Gen {
        /// Fail if any generated file is out of date instead of writing it.
        #[arg(long)]
        check: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    if let Command::Rpc { method, params, url } = &cli.command {
        std::process::exit(mcp::rpc_cli(method, params.as_deref(), url.as_deref()));
    }
    let json = cli.json;
    // `pocket run x -- --json`: the runtime's report is the output (a file or a pipe reads it as one
    // JSON document), so the tool's own summary goes to stderr.
    let runtime_json = matches!(&cli.command, Command::Run { args, .. } if args.iter().any(|a| a == "--json"));
    let result = run(cli);
    match result {
        Ok(rep) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&rep).unwrap());
            } else if runtime_json {
                eprint!("{}", rep.human());
            } else {
                print!("{}", rep.human());
            }
            if !rep.ok {
                std::process::exit(1);
            }
        }
        Err(err) => {
            if json {
                let rep = report::Report::failure("error", format!("{err:#}"));
                println!("{}", serde_json::to_string_pretty(&rep).unwrap());
            } else {
                eprintln!("pocket: error: {err:#}");
            }
            std::process::exit(2);
        }
    }
}

// The engine's workspace: --root, else the nearest pocket.toml above the current directory, else,
// for a game kept outside the engine (an agent's copy, a project of its own), POCKET_ROOT or the
// workspace this tool was built in or installed into (.pocket/pocket sits in it).
fn workspace_root(given: Option<std::path::PathBuf>) -> Result<std::path::PathBuf> {
    if let Some(r) = given {
        return Ok(r);
    }
    let cwd = std::env::current_dir()?;
    let found = manifest::find_root(&cwd);
    if found.is_ok() {
        return found;
    }
    if let Ok(env) = std::env::var("POCKET_ROOT") {
        let p = std::path::PathBuf::from(env);
        if p.join("pocket.toml").exists() {
            return Ok(p);
        }
    }
    if let Some(r) = std::env::current_exe().ok().and_then(|exe| exe.parent().and_then(|d| manifest::find_root(d).ok())) {
        return Ok(r);
    }
    found
}

fn run(cli: Cli) -> Result<report::Report> {
    let root = workspace_root(cli.root)?;
    let ws = manifest::Workspace::load(&root)?;
    match cli.command {
        Command::Setup { force, target } => commands::setup(&ws, force, &target),
        Command::Doctor => commands::doctor(&ws),
        Command::Build { targets, config, generate_only } => commands::build(&ws, &config, &targets, generate_only),
        Command::Run { target, config, watch, args } => if watch { watch::watch(&ws, &config, &target, &args, false) } else { commands::run(&ws, &config, &target, &args) },
        Command::Test { config, filter } => commands::test(&ws, &config, filter.as_deref()),
        Command::Scenario { target, config, file, seeds, frames, only } => commands::scenario(&ws, &config, &target, file.as_deref(), seeds, frames, only.as_deref()),
        Command::Bench { target, config, file, frames, only } => commands::bench(&ws, &config, &target, file.as_deref(), frames, only.as_deref()),
        Command::Ts { project, out } => commands::ts_bundle(&ws, &project, out.as_deref()),
        Command::Check { project } => check::check(&ws, project.as_deref()),
        Command::Clean => commands::clean(&ws),
        Command::Graph => commands::graph(&ws),
        Command::Gen { check } => commands::gen(&ws, check),
        Command::New { name, dir, from, list } => {
            if list {
                commands::list_templates(&ws)
            } else if let Some(name) = name {
                match from {
                    Some(from) => commands::new_from_template(&ws, &name, &dir, &from),
                    None => commands::new_project(&ws, &name, &dir),
                }
            } else {
                Ok(report::Report::failure("new", "give the project's name, or --list to see the samples to start from"))
            }
        }
        Command::Pack { target, config, out, zip, web, editor } => {
            if web {
                pack::pack_web(&ws, config.as_deref().unwrap_or("wasm"), &target, out.as_deref(), zip, editor)
            } else if editor {
                Ok(report::Report::failure("pack", "--editor needs --web (native packs run the editor with `pocket editor`)"))
            } else {
                pack::pack(&ws, config.as_deref().unwrap_or("release"), &target, out.as_deref(), zip)
            }
        }
        Command::Editor { target, config, watch, args } => if watch { watch::watch(&ws, &config, &target, &args, true) } else { commands::editor(&ws, &config, &target, &args) },
        Command::Mcp => {
            mcp::serve(&ws)?;
            Ok(report::Report::success("mcp", "stdio session ended"))
        }
        Command::Rpc { .. } => unreachable!("handled before the workspace is opened"),
    }
}
