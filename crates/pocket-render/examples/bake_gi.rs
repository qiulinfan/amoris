//! Offline world-space diffuse probe baking.
//! cargo run --release -p pocket-render --example bake_gi -- PROJECT --output FILE
//!   --origin x,y,z --spacing x,y,z --dims n,n,n --rays N --bounces N --seed N
//!   [--distance-resolution N] [--max-distance N]

#[cfg(all(not(target_arch = "wasm32"), feature = "import"))]
fn main() {
    if let Err(error) = run() {
        eprintln!("bake_gi: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(all(not(target_arch = "wasm32"), feature = "import")))]
fn main() {
    eprintln!("bake_gi requires native target and the import feature");
}

#[cfg(all(not(target_arch = "wasm32"), feature = "import"))]
fn run() -> Result<(), String> {
    use pocket_render::gi::bake::{BakeOptions, bake_project};
    use std::path::PathBuf;

    const USAGE: &str = "usage: bake_gi PROJECT --output FILE --origin x,y,z --spacing x,y,z --dims n,n,n --rays N --bounces N --seed N [--distance-resolution N] [--max-distance N]";
    let mut args = std::env::args().skip(1);
    let project = args.next().ok_or(USAGE)?;
    if project == "--help" || project == "-h" {
        println!("{USAGE}");
        return Ok(());
    }
    if project.starts_with('-') {
        return Err(USAGE.into());
    }
    let mut options = BakeOptions::default();
    let mut output = None;
    let mut seen = std::collections::BTreeSet::new();
    while let Some(flag) = args.next() {
        if !seen.insert(flag.clone()) {
            return Err(format!("duplicate option {flag}"));
        }
        let value = args
            .next()
            .ok_or_else(|| format!("{flag} requires a value"))?;
        match flag.as_str() {
            "--output" => output = Some(PathBuf::from(value)),
            "--origin" => options.origin = triple(&value, "origin")?,
            "--spacing" => options.spacing = triple(&value, "spacing")?,
            "--dims" => options.dimensions = triple(&value, "dimensions")?,
            "--rays" => options.rays_per_probe = number(&value, "rays")?,
            "--bounces" => options.bounces = number(&value, "bounces")?,
            "--seed" => options.seed = number(&value, "seed")?,
            "--distance-resolution" => {
                options.distance_resolution = number(&value, "distance resolution")?
            }
            "--max-distance" => options.max_distance = number(&value, "max distance")?,
            _ => return Err(format!("unknown option {flag}; {USAGE}")),
        }
    }
    let output = output.ok_or("--output FILE is required")?;
    options.validate()?;
    let (asset, stats) = bake_project(&PathBuf::from(project), &options)?;
    let bytes = serde_json::to_vec(&asset).map_err(|e| e.to_string())?;
    if bytes.len() > pocket_assets::gi::MAX_JSON_BYTES {
        return Err("serialized asset exceeds 64 MiB; split the volume".into());
    }
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    // Write the completed asset only after all geometry, options, and probe values are valid.
    std::fs::write(&output, bytes).map_err(|e| format!("{}: {e}", output.display()))?;
    let mut report = serde_json::to_value(&stats).map_err(|e| e.to_string())?;
    report["output"] = serde_json::json!(output.display().to_string());
    println!(
        "{}",
        serde_json::to_string(&report).map_err(|e| e.to_string())?
    );
    Ok(())
}

#[cfg(all(not(target_arch = "wasm32"), feature = "import"))]
fn number<T: std::str::FromStr>(value: &str, name: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("{name}: invalid number {value}"))
}

#[cfg(all(not(target_arch = "wasm32"), feature = "import"))]
fn triple<T: std::str::FromStr>(value: &str, name: &str) -> Result<[T; 3], String> {
    let values = value
        .split(',')
        .map(|v| number(v, name))
        .collect::<Result<Vec<_>, _>>()?;
    values
        .try_into()
        .map_err(|_| format!("{name}: expected three comma-separated numbers"))
}
