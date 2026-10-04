//! Writes a project's web package (`pocket_web::package`): `project.toml`, `scene.json` and the
//! TypeScript of `scripts/`, compiled natively with the lint on, as the one JSON file the browser
//! form loads. The shipped module has no transpiler (architecture.md 8.3); this stands in for
//! `pocket pack <project> --web` until `pocket-app` has it (threads-slice1.md 14).
//!
//!   cargo run -p pocket-web --example pack --release -- <project> <out.json>
//!
//! Prints `{"package": <out>, "bundle": <hash>, "modules": n, "bytes": n}`; a problem goes to
//! stdout as `{"error": ...}` with exit code 1, usage with exit code 2.

use std::sync::Arc;

use pocket_contract::Problem;
use pocket_runtime::Project;
use pocket_web::package::Package;
use serde_json::json;

fn pack(project: &str, out: &str) -> Result<serde_json::Value, Problem> {
    let p = Project::load(std::path::Path::new(project))?;
    let setup = Arc::new(p.setup(false)?);
    let package = Package::from_setup(&setup, p.manifest.seed);
    let bytes = package.to_bytes();
    std::fs::write(out, &bytes).map_err(|e| {
        Problem::new(
            "project.unreadable",
            format!("{out} cannot be written: {e}."),
            pocket_contract::detail([("path", json!(out))]),
        )
    })?;
    Ok(json!({
        "package": out,
        "bundle": setup.scripts.bundle.hash.to_hex(),
        "modules": setup.scripts.modules.len(),
        "bytes": bytes.len(),
    }))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [project, out] = args.as_slice() else {
        eprintln!("usage: pack <project> <out.json>");
        std::process::exit(2);
    };
    let (project, out) = (project.clone(), out.clone());
    // Compiling runs the lint and the host's checks, which want the script host's stack.
    let worker = std::thread::Builder::new()
        .stack_size(pocket_runtime::GAME_STACK_BYTES)
        .spawn(move || pack(&project, &out))
        .expect("a thread for the packer");
    match worker.join() {
        Ok(Ok(v)) => println!("{v}"),
        Ok(Err(p)) => {
            println!("{}", json!({"error": p}));
            std::process::exit(1);
        }
        Err(_) => {
            println!(
                "{}",
                json!({"error": {"code": "internal.error", "message": "pack panicked"}})
            );
            std::process::exit(1);
        }
    }
}
