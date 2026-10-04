//! A project's `check.toml` and its input files (docs/spec/checks.md 8.1).
//!
//! An input file holds one command per line in the form agents send, with the tick it is an input
//! of (applied at boundary `tick - 1`): `{"tick": 30, "source": {"player": 0}, "name": ...,
//! "params": {...}}`. Each source's commands take sequence numbers in file order, and a tick's
//! commands apply in the canonical order of threads.md 5.2.

use std::path::Path;

use pocket_contract::{Problem, detail};
use pocket_link::{Source, source_from_json};
use pocket_persist::replay::{RecordedWrite, canonical_json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// `check.toml`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckToml {
    pub run: RunSection,
    #[serde(default)]
    pub fork: Option<ForkSection>,
    #[serde(default)]
    pub reload: Option<ReloadSection>,
    #[serde(default)]
    pub web: Option<WebSection>,
    #[serde(default)]
    pub expect: Option<ExpectSection>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunSection {
    /// Each check runs once per seed.
    pub seeds: Vec<u64>,
    /// Ticks per run.
    pub ticks: u64,
    /// Commands applied during the run (a path relative to the project).
    #[serde(default)]
    pub inputs: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForkSection {
    /// Fork points (ticks).
    pub at: Vec<u64>,
    /// Ticks each comparison runs after its fork point.
    pub ticks: u64,
    /// Commands only the branch gets; their ticks count from the fork point (1 is the first tick
    /// after it).
    #[serde(default)]
    pub branch_inputs: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReloadSection {
    /// Boundaries where the unchanged scripts are reloaded.
    pub at: Vec<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WebSection {
    pub workload: bool,
}

/// A negative control (checks.md 8.6): the code the check must fail with, and whether the
/// project's scripts compile with the lint off, so a defect the lint would refuse reaches the
/// check that is the backstop.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExpectSection {
    pub fail: String,
    #[serde(default = "yes")]
    pub lint: bool,
}

fn yes() -> bool {
    true
}

/// `check.config_invalid {path, line?, message}` (checks.md 11, exit code 2).
pub fn invalid(path: &Path, line: Option<usize>, message: &str) -> Problem {
    Problem::new(
        "check.config_invalid",
        format!("{}: {message}", path.display()),
        detail([
            ("path", json!(path.display().to_string())),
            ("line", json!(line)),
            ("message", json!(message)),
        ]),
    )
}

/// The line, from 1, of a byte offset into `text`.
fn line_at(text: &str, at: usize) -> usize {
    text.as_bytes()[..at.min(text.len())]
        .iter()
        .filter(|b| **b == b'\n')
        .count()
        + 1
}

/// The line of what a decode refused: where a typed parse of the text puts its error (the
/// misspelt or mistyped key), else the first line that starts with the last key of the problem's
/// path.
fn decode_line(text: &str, p: &Problem) -> Option<usize> {
    if let Err(e) = toml::from_str::<CheckToml>(text)
        && let Some(span) = e.span()
    {
        return Some(line_at(text, span.start));
    }
    let key = p
        .detail
        .get("path")
        .and_then(Value::as_str)?
        .rsplit(['/', '.'])
        .next()
        .filter(|k| !k.is_empty())?;
    text.lines()
        .position(|l| l.trim_start().starts_with(key))
        .map(|i| i + 1)
}

impl CheckToml {
    /// Reads and checks `<project>/check.toml`.
    pub fn load(project: &Path) -> Result<CheckToml, Problem> {
        let path = project.join("check.toml");
        let text = pocket_runtime::read_file(&path)?;
        let value: Value = toml::from_str(&text).map_err(|e| {
            let line = e.span().map(|s| line_at(&text, s.start));
            invalid(&path, line, e.message())
        })?;
        let c: CheckToml = pocket_runtime::decode(&value, "check.toml")
            .map_err(|p| invalid(&path, decode_line(&text, &p), &p.message))?;
        if c.run.seeds.is_empty() {
            return Err(invalid(&path, None, "[run] seeds is empty"));
        }
        Ok(c)
    }
}

/// One input command.
#[derive(Clone, Debug, PartialEq)]
pub struct Input {
    /// Its line in the file, from 1.
    pub line: usize,
    pub tick: u64,
    pub source: Source,
    pub seq: u64,
    pub name: String,
    pub params: Value,
}

impl Input {
    /// As the recorder writes it, and as lockstep and replays apply it.
    pub fn recorded(&self) -> RecordedWrite {
        RecordedWrite {
            source: self.source,
            seq: self.seq,
            name: self.name.clone(),
            params: canonical_json(&self.params),
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Line {
    tick: u64,
    source: Value,
    name: String,
    #[serde(default)]
    params: serde_json::Map<String, Value>,
}

/// A file of inputs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inputs {
    items: Vec<Input>,
}

impl Inputs {
    /// Reads `path` (relative to the project); no path, no inputs.
    pub fn load(project: &Path, path: Option<&str>) -> Result<Inputs, Problem> {
        let Some(rel) = path else {
            return Ok(Inputs::default());
        };
        let full = project.join(rel);
        let text = pocket_runtime::read_file(&full)?;
        Inputs::parse(&text).map_err(|(line, p)| invalid(&full, Some(line), &p.message))
    }

    /// Parses JSON lines; blank lines and lines starting with `#` or `//` are skipped.
    pub fn parse(text: &str) -> Result<Inputs, (usize, Problem)> {
        let mut items = Vec::new();
        let mut seqs: Vec<(Source, u64)> = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let line = i + 1;
            let t = raw.trim();
            if t.is_empty() || t.starts_with('#') || t.starts_with("//") {
                continue;
            }
            let v: Value = serde_json::from_str(t).map_err(|e| {
                (
                    line,
                    Problem::new(
                        "request.malformed",
                        format!("line {line} is not JSON: {e}"),
                        detail([]),
                    ),
                )
            })?;
            let l: Line = pocket_runtime::decode(&v, "an input line").map_err(|p| (line, p))?;
            if l.tick == 0 {
                return Err((
                    line,
                    Problem::new(
                        "request.out_of_range",
                        format!(
                            "line {line}: tick 0 has no boundary before it; inputs start at tick 1"
                        ),
                        detail([]),
                    ),
                ));
            }
            let source = source_from_json(&l.source).map_err(|p| (line, p))?;
            let seq = match seqs.iter_mut().find(|(s, _)| *s == source) {
                Some((_, n)) => {
                    *n += 1;
                    *n
                }
                None => {
                    seqs.push((source, 1));
                    1
                }
            };
            items.push(Input {
                line,
                tick: l.tick,
                source,
                seq,
                name: l.name,
                params: Value::Object(l.params),
            });
        }
        Ok(Inputs { items })
    }

    /// The inputs of tick `tick` in the order a boundary applies them.
    pub fn at(&self, tick: u64) -> Vec<&Input> {
        let mut v: Vec<&Input> = self.items.iter().filter(|i| i.tick == tick).collect();
        v.sort_by_key(|i| (i.source, i.seq));
        v
    }

    /// The same inputs `offset` ticks later (a branch file's relative ticks made absolute).
    pub fn shifted(&self, offset: u64) -> Inputs {
        Inputs {
            items: self
                .items
                .iter()
                .map(|i| Input {
                    tick: i.tick + offset,
                    ..i.clone()
                })
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }
}
