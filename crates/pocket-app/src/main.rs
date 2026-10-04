//! F0's executable verification slice. No scripts, physics, GPU or server yet.
mod scenario;

use pocket_contract::{Problem, detail};
use serde_json::json;

include!(concat!(env!("OUT_DIR"), "/source.rs"));

fn version() {
    let files: Vec<_> = SOURCE_FILES
        .iter()
        .map(|(name, bytes)| ((*name).to_owned(), bytes.to_vec()))
        .collect();
    let mut version = pocket_persist::EngineVersion::unbuilt();
    version.commit = BUILD_COMMIT.to_owned();
    version.source = pocket_persist::version::source_hash(&files);
    version.target = BUILD_TARGET.to_owned();
    version.profile = BUILD_PROFILE.to_owned();
    version.contract = "pocketengine-foundation-v1".to_owned();
    version.c_compiler = "not recorded in F0".to_owned();
    pocket_persist::EngineVersion::install(version).expect("installed before any snapshot");
}

fn usage(message: impl Into<String>) -> Problem {
    Problem::new("check.usage", message, detail([]))
}

fn run(args: &[String]) -> Result<(), Problem> {
    if args.is_empty() || matches!(args[0].as_str(), "help" | "--help") {
        println!("pocket foundation [--ticks 600] [--seed 7] [--json]");
        return Ok(());
    }
    if args[0] != "foundation" {
        return Err(usage("F0 provides only 'foundation'; use pocket help"));
    }
    let mut ticks = 600;
    let mut seed = 7;
    let mut json_output = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => json_output = true,
            "--ticks" | "--seed" => {
                let flag = &args[i];
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| usage(format!("{flag} needs a u64")))?;
                let value = value
                    .parse()
                    .map_err(|_| usage(format!("{flag} needs a u64")))?;
                if flag == "--ticks" {
                    ticks = value;
                } else {
                    seed = value;
                }
                i += 1;
            }
            flag => return Err(usage(format!("unknown option '{flag}'; use pocket help"))),
        }
        i += 1;
    }
    if !(3..=100_000).contains(&ticks) {
        return Err(usage("--ticks must be between 3 and 100000"));
    }
    version();
    let report = scenario::verify_foundation(seed, ticks)?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("report is JSON")
        );
    } else {
        println!(
            "PASS F0 | seed {} | {} ticks | hash {}",
            report.seed, report.ticks, report.world_hash
        );
        for check in &report.checks {
            println!("PASS {check}");
        }
    }
    Ok(())
}

fn main() {
    if let Err(problem) = run(&std::env::args().skip(1).collect::<Vec<_>>()) {
        println!("{}", json!({"error": problem}));
        std::process::exit(if problem.code == "check.usage" { 2 } else { 1 });
    }
}
