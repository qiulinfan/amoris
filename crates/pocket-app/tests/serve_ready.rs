//! `pocket serve`'s readiness line (docs/spec/server.md, docs/spec/desktop.md): the project it
//! reports, in the line, in `.pocket/host.json` and in `project.info`, is the canonical directory in
//! the plain form other programs resolve it to. On Windows `canonicalize` gives `\\?\C:\...`, which
//! the desktop's `fs.realpath` comparison never equals; the host now drops that prefix.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The host process, killed when the test ends however it ends.
struct Host(Child);

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn project_info(url: &str) -> Value {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into();
    let body = json!({"id": 1, "method": "project.info", "params": {}}).to_string();
    let text = agent
        .post(format!("{url}/api/call"))
        .content_type("application/json")
        .send(&body)
        .unwrap()
        .body_mut()
        .read_to_string()
        .unwrap();
    serde_json::from_str(&text).unwrap()
}

#[test]
fn serve_reports_the_project_as_a_plain_canonical_path() {
    let dir = std::env::temp_dir().join(format!("pocket-serve-ready-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let project = dir.join("Sailing");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/sailing"),
        &project,
    );
    let child = Command::new(env!("CARGO_BIN_EXE_pocket"))
        .args(["serve", project.to_str().unwrap(), "--port", "0"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut host = Host(child);
    let stdout = host.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = rx
        .recv_timeout(Duration::from_secs(120))
        .expect("a readiness line");
    let ready: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"));
    let reported = ready["project"].as_str().unwrap().to_owned();
    assert!(!reported.starts_with(r"\\?\"), "{reported}");
    let canonical = std::fs::canonicalize(&project).unwrap();
    assert_eq!(std::fs::canonicalize(&reported).unwrap(), canonical);
    assert_eq!(
        PathBuf::from(&reported),
        pocket_server::hostfile::project_root(&project).unwrap()
    );
    let file = pocket_server::hostfile::read(&pocket_server::hostfile::path(&project)).unwrap();
    assert_eq!(file.project, reported);
    assert_eq!(file.pid, host.0.id());
    let info = project_info(ready["url"].as_str().unwrap());
    assert_eq!(info["result"]["root"], json!(reported), "{info}");
    drop(host);
    let _ = std::fs::remove_dir_all(&dir);
}
