//! Running the tools the check drives (cargo, git, python, wasm-bindgen) as processes, with every
//! command, its working directory and the environment xtask adds written at the top of its log, so
//! a failure can be rerun by hand (docs/spec/checks.md 13).

use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// What a finished process gave: its exit code (None when killed) and its output.
pub struct Outcome {
    pub code: Option<i32>,
    pub text: String,
}

impl Outcome {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }

    /// The last `n` lines, for a problem's detail.
    pub fn tail(&self, n: usize) -> String {
        let lines: Vec<&str> = self.text.lines().collect();
        lines[lines.len().saturating_sub(n)..].join("\n")
    }
}

/// The places and tools of one run of the check.
pub struct Env {
    pub root: PathBuf,
    pub out: PathBuf,
    pub cargo: PathBuf,
    pub host: String,
    pub llvm: PathBuf,
}

impl Env {
    pub fn new(root: &Path) -> Env {
        let cargo = std::env::var_os("CARGO").map_or_else(|| PathBuf::from("cargo"), PathBuf::from);
        let host = host_triple().unwrap_or_else(|| "unknown".into());
        let llvm = std::env::var_os("POCKET_LLVM")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".pocket-tools").join("llvm-23.1.2"));
        Env {
            root: root.to_path_buf(),
            out: root.join("out").join("check"),
            cargo,
            host,
            llvm,
        }
    }

    /// A cargo command in the workspace root with the C compilers of architecture.md 7.3 and
    /// checks.md 6.3 set: clang-cl for QuickJS-ng on the MSVC host, clang for the web target.
    pub fn cargo<I, S>(&self, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = Command::new(&self.cargo);
        cmd.args(args).current_dir(&self.root);
        for (k, v) in self.c_env() {
            cmd.env(k, v);
        }
        cmd
    }

    pub fn c_env(&self) -> Vec<(String, String)> {
        let mut env = Vec::new();
        let tool = |name: &str| {
            let p = self
                .llvm
                .join("bin")
                .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
            p.is_file().then(|| p.to_string_lossy().into_owned())
        };
        if self.host == "x86_64-pc-windows-msvc"
            && let Some(cl) = tool("clang-cl")
        {
            env.push(("CC_x86_64_pc_windows_msvc".into(), cl));
        }
        for (var, name) in [("CC", "clang"), ("CXX", "clang++"), ("AR", "llvm-ar")] {
            if let Some(p) = tool(name) {
                env.push((format!("{var}_wasm32_unknown_unknown"), p));
            }
        }
        env
    }

    pub fn clang_for_web(&self) -> Option<PathBuf> {
        let p = self
            .llvm
            .join("bin")
            .join(format!("clang{}", std::env::consts::EXE_SUFFIX));
        p.is_file().then_some(p)
    }

    pub fn log_path(&self, step: &str) -> PathBuf {
        self.out.join(format!("{step}.log"))
    }

    /// Starts a step's log afresh.
    pub fn new_log(&self, step: &str) -> PathBuf {
        let path = self.log_path(step);
        let _ = fs::create_dir_all(&self.out);
        let _ = File::create(&path);
        path
    }

    /// The log's path relative to the root, as the report names it.
    pub fn rel(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }
}

pub fn home() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
}

fn host_triple() -> Option<String> {
    let out = Command::new("rustc").arg("-vV").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .find_map(|l| l.strip_prefix("host: ").map(|h| h.trim().to_string()))
}

/// The problem for a command that could not start: a missing program is `check.tool_missing`,
/// which makes the step inconclusive; anything else is `check.command_failed`.
pub fn start_failed(line: &str, e: &io::Error) -> crate::report::Problem {
    let program = line.split_whitespace().next().unwrap_or(line);
    if e.kind() == io::ErrorKind::NotFound {
        crate::report::Problem::new(
            "check.tool_missing",
            format!("{program} is not installed or not on PATH"),
            serde_json::json!({"tool": program, "command": line}),
        )
    } else {
        crate::report::Problem::new(
            "check.command_failed",
            format!("{line} could not start: {e}"),
            serde_json::json!({"command": line}),
        )
    }
}

/// The command as a shell line, for logs and details.
pub fn describe(cmd: &Command) -> String {
    let mut parts = vec![cmd.get_program().to_string_lossy().into_owned()];
    for a in cmd.get_args() {
        let a = a.to_string_lossy();
        parts.push(if a.contains(' ') || a.is_empty() {
            format!("\"{a}\"")
        } else {
            a.into_owned()
        });
    }
    parts.join(" ")
}

fn header(cmd: &Command, log: &mut File) -> io::Result<()> {
    writeln!(log, "$ {}", describe(cmd))?;
    if let Some(dir) = cmd.get_current_dir() {
        writeln!(log, "# cwd: {}", dir.display())?;
    }
    for (k, v) in cmd.get_envs() {
        writeln!(
            log,
            "# env: {}={}",
            k.to_string_lossy(),
            v.map(|v| v.to_string_lossy()).unwrap_or_default()
        )?;
    }
    log.flush()
}

fn append(log: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(log)
}

/// Runs a command with stdout and stderr in one pipe, in the order the process wrote them (so a
/// test's output follows cargo's `Running` line), copying every line to the log as it comes.
/// `on_line` sees each line, for progress.
pub fn run_merged(
    mut cmd: Command,
    log: &Path,
    on_line: &mut dyn FnMut(&str),
) -> io::Result<Outcome> {
    let mut file = append(log)?;
    header(&cmd, &mut file)?;
    let (reader, writer) = io::pipe()?;
    cmd.stdout(writer.try_clone()?)
        .stderr(writer)
        .stdin(Stdio::null());
    let mut child = cmd.spawn()?;
    drop(cmd); // closes the parent's copies of the pipe's write end, so reading ends with the child
    let mut text = String::new();
    let mut reader = BufReader::new(reader);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        if reader.read_until(b'\n', &mut buf)? == 0 {
            break;
        }
        let line = String::from_utf8_lossy(&buf);
        file.write_all(line.as_bytes())?;
        on_line(line.trim_end());
        text.push_str(&line);
    }
    let status = child.wait()?;
    writeln!(file, "# exit: {:?}\n", status.code())?;
    Ok(Outcome {
        code: status.code(),
        text,
    })
}

/// Runs a command whose stdout is data (JSON): stdout is returned, stderr goes to the log.
pub fn run_stdout(mut cmd: Command, log: &Path) -> io::Result<Outcome> {
    let mut file = append(log)?;
    header(&cmd, &mut file)?;
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    let mut child = cmd.spawn()?;
    let mut stderr = child.stderr.take().expect("piped");
    let errors = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = stderr.read_to_end(&mut s);
        s
    });
    let mut stdout = Vec::new();
    child
        .stdout
        .take()
        .expect("piped")
        .read_to_end(&mut stdout)?;
    let status = child.wait()?;
    let errors = errors.join().unwrap_or_default();
    file.write_all(&errors)?;
    writeln!(file, "# exit: {:?}\n", status.code())?;
    Ok(Outcome {
        code: status.code(),
        text: String::from_utf8_lossy(&stdout).into_owned(),
    })
}

/// A quick command whose output is a short answer (`git rev-parse`, `rustc -vV`); None on failure.
pub fn capture(program: &str, args: &[&str], dir: &Path) -> Option<String> {
    let out = Command::new(program)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The Python that runs `tools/*.py`: `POCKET_PYTHON`, else `python`, else `python3` (on Windows
/// `python3` may be the Store's stub, which the version probe rejects).
pub fn python() -> Option<String> {
    let mut candidates: Vec<String> = std::env::var("POCKET_PYTHON").ok().into_iter().collect();
    candidates.extend(["python".to_string(), "python3".to_string()]);
    candidates.into_iter().find(|p| {
        Command::new(p)
            .arg("--version")
            .stdin(Stdio::null())
            .output()
            .is_ok_and(|o| {
                o.status.success() && String::from_utf8_lossy(&o.stdout).starts_with("Python 3")
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_quotes_arguments_with_spaces() {
        let mut cmd = Command::new("cargo");
        cmd.args(["clippy", "a b", ""]);
        assert_eq!(describe(&cmd), "cargo clippy \"a b\" \"\"");
    }

    #[test]
    fn merged_output_keeps_both_streams_and_the_exit_code() {
        let dir = std::env::temp_dir().join(format!("xtask-run-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let log = dir.join("t.log");
        let mut cmd = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        cmd.arg("--version");
        let out = run_merged(cmd, &log, &mut |_| {}).unwrap();
        assert!(out.ok());
        assert!(out.text.starts_with("cargo "));
        assert!(
            fs::read_to_string(&log)
                .unwrap()
                .contains("# exit: Some(0)")
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
