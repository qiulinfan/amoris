//! `pocket`: the Pocket3D build tool.
//!
//! One binary that reads the workspace manifest (`pocket.toml`) and every
//! `module.toml`, resolves the module graph, fetches or builds third-party
//! dependencies into `.pocket/`, writes a Ninja build graph, runs it, and
//! exposes the same operations with `--json` output for agents.

mod commands;
mod deps;
mod gen;
mod graph;
mod manifest;
mod mcp;
mod ninja;
mod report;
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
    /// Build and run every test module, then summarise.
    Test {
        #[arg(long, default_value = "debug")]
        config: String,
        /// Substring filter on test module names.
        #[arg(long)]
        filter: Option<String>,
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
    /// Create a project: project.toml, a scene, a prefab, assets/ and a first script.
    New {
        name: String,
        /// Parent directory (default projects/).
        #[arg(long, default_value = "projects")]
        dir: PathBuf,
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
    let json = cli.json;
    let result = run(cli);
    match result {
        Ok(rep) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&rep).unwrap());
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

fn run(cli: Cli) -> Result<report::Report> {
    let root = match cli.root {
        Some(r) => r,
        None => manifest::find_root(&std::env::current_dir()?)?,
    };
    let ws = manifest::Workspace::load(&root)?;
    match cli.command {
        Command::Setup { force, target } => commands::setup(&ws, force, &target),
        Command::Doctor => commands::doctor(&ws),
        Command::Build { targets, config, generate_only } => commands::build(&ws, &config, &targets, generate_only),
        Command::Run { target, config, watch, args } => if watch { watch::watch(&ws, &config, &target, &args, false) } else { commands::run(&ws, &config, &target, &args) },
        Command::Test { config, filter } => commands::test(&ws, &config, filter.as_deref()),
        Command::Ts { project, out } => commands::ts_bundle(&ws, &project, out.as_deref()),
        Command::Clean => commands::clean(&ws),
        Command::Graph => commands::graph(&ws),
        Command::Gen { check } => commands::gen(&ws, check),
        Command::New { name, dir } => commands::new_project(&ws, &name, &dir),
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
    }
}
