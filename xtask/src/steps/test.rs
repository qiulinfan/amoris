//! `test`: `cargo test --workspace --release --locked --no-fail-fast`, read from libtest's stable
//! text output (docs/spec/checks.md 6.2). The exit status of `cargo test` is authoritative: a
//! failure the parser does not recognize is still a failure.

use super::diag;
use crate::report::{Problem, StepResult};
use crate::run::{self, Env};
use serde_json::json;

#[derive(Debug, Default, PartialEq)]
pub struct Results {
    pub failed: Vec<Failure>,
    pub passed: u64,
    pub ignored: u64,
    pub suites: u64,
}

#[derive(Debug, PartialEq)]
pub struct Failure {
    pub krate: String,
    pub test: String,
    pub message: String,
}

/// The crate a suite belongs to, from cargo's `Running unittests src/lib.rs (…/deps/pocket_sim-0a1b…)`
/// or `Doc-tests pocket_sim` line.
fn suite_name(line: &str) -> Option<String> {
    let t = line.trim();
    if let Some(rest) = t.strip_prefix("Doc-tests ") {
        return Some(rest.trim().to_string());
    }
    let rest = t.strip_prefix("Running ")?;
    let inner = rest
        .rsplit_once('(')
        .map_or(rest, |(_, p)| p.trim_end_matches(')'));
    let file = inner.rsplit(['/', '\\']).next()?;
    let stem = file.strip_suffix(".exe").unwrap_or(file);
    Some(
        stem.rsplit_once('-')
            .map_or(stem, |(name, _)| name)
            .to_string(),
    )
}

/// Reads libtest's output (stdout and stderr merged in order).
pub fn parse(text: &str) -> Results {
    let mut r = Results::default();
    let mut krate = String::new();
    let mut failing: Vec<(String, String)> = Vec::new(); // (crate, test) in the order they failed
    let mut current: Option<(String, Vec<String>)> = None;
    let mut messages: Vec<(String, String, String)> = Vec::new();
    for line in text.lines() {
        if line.starts_with('{') {
            continue; // cargo's JSON messages
        }
        if let Some(name) = suite_name(line) {
            krate = name;
            r.suites += 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix("---- ")
            && let Some(name) = rest
                .strip_suffix(" stdout ----")
                .or_else(|| rest.strip_suffix(" stderr ----"))
        {
            if let Some((t, lines)) = current.take() {
                messages.push((krate.clone(), t, lines.join("\n")));
            }
            current = Some((name.to_string(), Vec::new()));
            continue;
        }
        if let Some((_, lines)) = current.as_mut() {
            if line == "failures:" || line.starts_with("test result:") {
                let (t, lines) = current.take().expect("current");
                messages.push((krate.clone(), t, lines.join("\n")));
            } else {
                lines.push(line.to_string());
                continue;
            }
        }
        if let Some(rest) = line.strip_prefix("test ")
            && let Some(name) = rest.strip_suffix(" ... FAILED")
        {
            failing.push((krate.clone(), name.to_string()));
        }
        if let Some(rest) = line.strip_prefix("test result: ") {
            for part in rest.split(';') {
                let words: Vec<&str> = part.split_whitespace().collect();
                let n = words
                    .iter()
                    .rev()
                    .nth(1)
                    .and_then(|w| w.parse::<u64>().ok())
                    .unwrap_or(0);
                match words.last() {
                    Some(&"passed") => r.passed += n,
                    Some(&"ignored") => r.ignored += n,
                    _ => {}
                }
            }
        }
    }
    if let Some((t, lines)) = current.take() {
        messages.push((krate.clone(), t, lines.join("\n")));
    }
    for (k, test) in failing {
        let message = messages
            .iter()
            .find(|(mk, mt, _)| *mt == test && *mk == k)
            .map(|(_, _, m)| {
                m.lines()
                    .filter(|l| !l.trim().is_empty())
                    .take(20)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        r.failed.push(Failure {
            krate: k,
            test,
            message,
        });
    }
    r
}

pub fn run(env: &Env, quick: bool) -> StepResult {
    let mut step = StepResult::new("test");
    let log = env.new_log("test");
    step.log = Some(env.rel(&log));
    let mut args: Vec<&str> = vec![
        "test",
        "--workspace",
        "--release",
        "--locked",
        "--no-fail-fast",
        "--message-format=json",
    ];
    if !quick {
        args.extend(["--", "--include-ignored"]);
    }
    let cmd = env.cargo(args);
    let line = run::describe(&cmd);
    let out = match run::run_merged(cmd, &log, &mut |_| {}) {
        Ok(o) => o,
        Err(e) => {
            step.problem(run::start_failed(&line, &e));
            return step;
        }
    };
    let compile: Vec<_> = diag::all(&out.text)
        .into_iter()
        .filter(|d| d.level == "error")
        .collect();
    for d in &compile {
        step.error(Problem::new(
            "test.failed",
            format!("{} does not compile its tests: {}: {}", d.package, d.at(), d.message),
            json!({"crate": d.package, "test": "(compile)", "message": format!("{}: {}", d.at(), d.message)}),
        ));
    }
    let r = parse(&out.text);
    for f in &r.failed {
        step.error(Problem::new(
            "test.failed",
            format!("{} {} failed", f.krate, f.test),
            json!({"crate": f.krate, "test": f.test, "message": f.message}),
        ));
    }
    if !out.ok() && r.failed.is_empty() && compile.is_empty() {
        step.error(Problem::new(
            "check.command_failed",
            format!(
                "{line} failed (exit {:?}) without a failure the step reads",
                out.code
            ),
            json!({"command": line, "status": out.code, "tail": diag::plain_tail(&out.text, 20)}),
        ));
    }
    step.measure("test.passed", r.passed as f64, "count");
    step.measure("test.failed", r.failed.len() as f64, "count");
    step.measure("test.ignored", r.ignored as f64, "count");
    step.summary = format!(
        "{} passed, {} failed, {} ignored in {} suites{}",
        r.passed,
        r.failed.len(),
        r.ignored,
        r.suites,
        if quick {
            ""
        } else {
            ", ignored tests included"
        }
    );
    step
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTPUT: &str = "\
{\"reason\":\"compiler-artifact\"}
     Running unittests src\\lib.rs (C:\\t\\release\\deps\\pocket_sim-0a1b2c3d.exe)

running 3 tests
test tests::a ... ok
test tests::b ... FAILED
test tests::slow ... ignored

failures:

---- tests::b stdout ----

thread 'tests::b' panicked at crates\\pocket-sim\\src\\lib.rs:9:5:
assertion `left == right` failed
  left: 1
 right: 2
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    tests::b

test result: FAILED. 1 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/fork.rs (C:\\t\\release\\deps\\fork-99.exe)

running 1 test
test forks_match ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

   Doc-tests pocket_sim

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
";

    #[test]
    fn libtest_output_is_read() {
        let r = parse(OUTPUT);
        assert_eq!(r.passed, 2);
        assert_eq!(r.ignored, 1);
        assert_eq!(r.suites, 3);
        assert_eq!(r.failed.len(), 1);
        let f = &r.failed[0];
        assert_eq!(
            (f.krate.as_str(), f.test.as_str()),
            ("pocket_sim", "tests::b")
        );
        assert!(
            f.message.starts_with("thread 'tests::b' panicked at"),
            "{}",
            f.message
        );
        assert!(f.message.contains("right: 2"));
        assert!(!f.message.contains("failures:"));
    }

    #[test]
    fn suite_names() {
        assert_eq!(
            suite_name("     Running unittests src/main.rs (target/release/deps/xtask-abc)")
                .unwrap(),
            "xtask"
        );
        assert_eq!(
            suite_name("   Doc-tests pocket_link").unwrap(),
            "pocket_link"
        );
        assert!(suite_name("running 3 tests").is_none());
    }
}
