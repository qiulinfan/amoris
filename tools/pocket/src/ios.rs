//! `pocket run <project> --ios`: the project packed as an app for the iOS Simulator
//! (`pocket pack --ios`), installed on a simulated device and started there (docs/build-system.md,
//! iOS). The device is the one named, else one already booted, else the first iPhone there is; it
//! is booted and shown in Simulator. The arguments after `--` go to the runtime, so
//! `-- --serve 4711` opens its control server, which the Mac reaches at 127.0.0.1:4711 (a
//! simulator shares the Mac's network).
use crate::manifest::Workspace;
use crate::report::Report;
use crate::toolchain;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::time::Instant;

fn simctl(args: &[&str]) -> Result<String> {
    let mut all = vec!["simctl"];
    all.extend_from_slice(args);
    toolchain::xcrun(&all)
}

/// The device to run on: (name, udid, booted).
fn pick_device(want: Option<&str>) -> Result<(String, String, bool)> {
    let list: Value = serde_json::from_str(&simctl(&["list", "devices", "available", "-j"])?).context("parsing simctl's device list")?;
    let mut devices = vec![];
    for (runtime, entries) in list["devices"].as_object().into_iter().flatten() {
        if !runtime.contains("iOS") {
            continue;
        }
        for d in entries.as_array().into_iter().flatten() {
            let name = d["name"].as_str().unwrap_or("").to_string();
            let udid = d["udid"].as_str().unwrap_or("").to_string();
            let booted = d["state"].as_str() == Some("Booted");
            devices.push((name, udid, booted));
        }
    }
    if devices.is_empty() {
        bail!("no iOS simulator is installed (xcodebuild -downloadPlatform iOS installs one)");
    }
    if let Some(w) = want {
        return devices.into_iter().find(|(n, u, _)| n == w || u == w).ok_or_else(|| anyhow!("no iOS simulator named '{w}' (xcrun simctl list devices available)"));
    }
    if let Some(d) = devices.iter().find(|d| d.2) {
        return Ok(d.clone());
    }
    Ok(devices.iter().find(|d| d.0.starts_with("iPhone")).cloned().unwrap_or_else(|| devices[0].clone()))
}

pub fn run_ios(ws: &Workspace, config: &str, target: &str, device: Option<&str>, args: &[String]) -> Result<Report> {
    let t0 = Instant::now();
    let packed = crate::pack::pack_ios(ws, config, target, None)?;
    if !packed.ok {
        return Ok(packed);
    }
    let app = packed.data["app"].as_str().unwrap_or("").to_string();
    let bundle_id = packed.data["bundle_id"].as_str().unwrap_or("").to_string();
    let (name, udid, booted) = pick_device(device)?;
    if !booted {
        simctl(&["boot", &udid])?;
    }
    // Shown in Simulator (Xcode's own app; it opens on the booted device).
    let simulator = std::path::Path::new(&toolchain::xcode_dir()?).join("Applications").join("Simulator.app");
    let _ = std::process::Command::new("open").arg("-a").arg(&simulator).args(["--args", "-CurrentDeviceUDID", &udid]).status();
    simctl(&["install", &udid, &app])?;
    let mut launch: Vec<&str> = vec!["launch", "--terminate-running-process", &udid, &bundle_id];
    launch.extend(args.iter().map(|s| s.as_str()));
    let started = simctl(&launch)?;
    // "dev.pocket.hello: 12345"
    let pid = started.rsplit(':').next().and_then(|p| p.trim().parse::<i64>().ok());
    let mut rep = Report::success("run", format!("{target} is running on {name} (iOS Simulator){}", pid.map(|p| format!(", pid {p}")).unwrap_or_default()));
    rep.data = json!({ "app": app, "bundle_id": bundle_id, "device": name, "udid": udid, "pid": pid, "args": args });
    rep.elapsed_ms = t0.elapsed().as_millis();
    Ok(rep)
}
