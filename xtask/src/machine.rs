//! The machine fingerprint every report carries (docs/spec/budgets.md 3), from what the system
//! answers quickly without administrator rights. The GPUs' backends come from `wgpu`, which xtask
//! does not link: they stay empty until `pocket` reports them.

use crate::report::{GpuInfo, Machine};
use crate::run::capture;
use std::path::Path;

/// R1's CPU and discrete GPU (budgets.md 3).
const R1_CPU: &str = "AMD Ryzen 9 270";
const R1_GPU: &str = "NVIDIA GeForce RTX 5060 Laptop GPU";

pub fn fingerprint(root: &Path) -> Machine {
    let mut m = Machine {
        logical_cores: std::thread::available_parallelism().map_or(0, |n| n.get() as u32),
        os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        rustc: capture("rustc", &["--version"], root)
            .map(|s| s.trim().to_string())
            .unwrap_or_default(),
        ..Machine::default()
    };
    if cfg!(windows) {
        windows(&mut m, root);
    } else if let Ok(text) = std::fs::read_to_string("/proc/cpuinfo") {
        m.cpu = text
            .lines()
            .find_map(|l| {
                l.strip_prefix("model name")
                    .and_then(|r| r.split_once(':'))
                    .map(|(_, v)| v.trim().to_string())
            })
            .unwrap_or_default();
    } else if let Some(cpu) = capture("sysctl", &["-n", "machdep.cpu.brand_string"], root) {
        m.cpu = cpu.trim().to_string();
    }
    let r1 = m.cpu.starts_with(R1_CPU) && m.gpus.iter().any(|g| g.name == R1_GPU);
    m.id = if r1 { "r1".into() } else { "other".into() };
    m
}

/// The values called `name` in `reg query` output, each with the key it sits under.
fn reg_values(text: &str, name: &str) -> Vec<(String, String)> {
    let mut key = String::new();
    let mut out = Vec::new();
    for l in text.lines() {
        if l.starts_with("HKEY_") {
            key = l.trim().to_string();
            continue;
        }
        let mut parts = l.split_whitespace();
        if parts.next() == Some(name) && parts.next().is_some_and(|t| t.starts_with("REG_")) {
            out.push((key.clone(), parts.collect::<Vec<_>>().join(" ")));
        }
    }
    out
}

fn windows(m: &mut Machine, root: &Path) {
    let cpu_key = r"HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0";
    if let Some(text) = capture(
        "reg",
        &["query", cpu_key, "/v", "ProcessorNameString"],
        root,
    ) {
        m.cpu = reg_values(&text, "ProcessorNameString")
            .into_iter()
            .next()
            .map(|(_, v)| v)
            .unwrap_or_default();
    }
    let display =
        r"HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";
    if let Some(text) = capture("reg", &["query", display, "/s", "/v", "DriverDesc"], root) {
        let versions = capture(
            "reg",
            &["query", display, "/s", "/v", "DriverVersion"],
            root,
        )
        .unwrap_or_default();
        let versions = reg_values(&versions, "DriverVersion");
        for (key, name) in reg_values(&text, "DriverDesc") {
            let driver = versions
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.clone());
            m.gpus.push(GpuInfo {
                name,
                driver,
                backend: None,
            });
        }
    }
    if let Some(text) = capture("powercfg", &["/getactivescheme"], root) {
        m.power_plan = text
            .rsplit_once('(')
            .map(|(_, p)| p.trim().trim_end_matches(')').to_string());
    }
    m.on_ac_power = ac_power();
    let chrome = Path::new(r"C:\Program Files\Google\Chrome\Application");
    if let Ok(entries) = std::fs::read_dir(chrome) {
        m.chrome = entries
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .find(|n| n.split('.').count() == 4 && n.split('.').all(|p| p.parse::<u32>().is_ok()))
            .map(|v| format!("Google Chrome {v}"));
    }
}

#[cfg(windows)]
fn ac_power() -> Option<bool> {
    #[repr(C)]
    #[derive(Default)]
    struct SystemPowerStatus {
        ac_line_status: u8,
        battery_flag: u8,
        battery_life_percent: u8,
        system_status_flag: u8,
        battery_life_time: u32,
        battery_full_life_time: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetSystemPowerStatus(status: *mut SystemPowerStatus) -> i32;
    }
    let mut status = SystemPowerStatus::default();
    // SAFETY: the struct has the layout of SYSTEM_POWER_STATUS and outlives the call.
    let ok = unsafe { GetSystemPowerStatus(&mut status) } != 0;
    match (ok, status.ac_line_status) {
        (true, 0) => Some(false),
        (true, 1) => Some(true),
        _ => None,
    }
}

#[cfg(not(windows))]
fn ac_power() -> Option<bool> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_values_are_read() {
        let text = "\r\nHKEY_LOCAL_MACHINE\\X\\0000\r\n    DriverDesc    REG_SZ    AMD Radeon 780M Graphics\r\n\r\n\
                    HKEY_LOCAL_MACHINE\\X\\0001\r\n    DriverDesc    REG_SZ    NVIDIA GeForce RTX 5060 Laptop GPU\r\n";
        let found: Vec<String> = reg_values(text, "DriverDesc")
            .into_iter()
            .map(|(k, v)| format!("{}={v}", k.rsplit('\\').next().unwrap()))
            .collect();
        assert_eq!(
            found,
            [
                "0000=AMD Radeon 780M Graphics",
                "0001=NVIDIA GeForce RTX 5060 Laptop GPU"
            ]
        );
    }

    #[test]
    fn the_fingerprint_names_the_host() {
        let m = fingerprint(Path::new("."));
        assert!(m.logical_cores > 0);
        assert!(m.rustc.starts_with("rustc "), "{}", m.rustc);
        assert!(m.id == "r1" || m.id == "other");
    }
}
