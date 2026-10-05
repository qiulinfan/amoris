//! `.pocket/host.json` (docs/spec/server.md, CLI): where a running host serves a project, so the
//! `pocket` CLI and `pocket mcp` find it from the project's directory.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file's content.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HostFile {
    /// `http://127.0.0.1:<port>`.
    pub url: String,
    pub port: u16,
    pub pid: u32,
    /// The project's directory.
    pub project: String,
}

/// Where the file of `project` is.
pub fn path(project: &Path) -> PathBuf {
    project.join(".pocket").join("host.json")
}

/// Writes the file for a host on `port`.
pub fn write(project: &Path, port: u16) -> std::io::Result<HostFile> {
    let f = HostFile {
        url: format!("http://127.0.0.1:{port}"),
        port,
        pid: std::process::id(),
        project: project.display().to_string(),
    };
    let p = path(project);
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(&f).unwrap_or_default();
    std::fs::write(&p, text + "\n")?;
    Ok(f)
}

/// Removes the file if it still names this process.
pub fn remove(project: &Path) {
    let p = path(project);
    if read(&p).is_some_and(|f| f.pid == std::process::id()) {
        let _ = std::fs::remove_file(p);
    }
}

/// Reads a host file.
pub fn read(p: &Path) -> Option<HostFile> {
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

/// The nearest host file at or above `dir`.
pub fn find(dir: &Path) -> Option<(PathBuf, HostFile)> {
    dir.ancestors().find_map(|d| {
        let p = path(d);
        read(&p).map(|f| (p, f))
    })
}
