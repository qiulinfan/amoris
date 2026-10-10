//! Command-line parsing for `cargo xtask check` and `cargo xtask gen` (docs/spec/checks.md 3). An
//! unknown flag or step name is refused with a suggestion and exit code 2 (charter 3.4).

use crate::report::Problem;
use serde_json::json;

/// The steps of checks.md 4 in their order, as slice 1 runs them (checks.md 14, Slice 1 entries).
pub const STEPS: &[&str] = &[
    "deps",
    "fmt",
    "docs",
    "clippy",
    "build",
    "gen",
    "test",
    "wasm",
    "types",
    "determinism",
    "fork",
    "replay",
    "reload",
    "contract",
    "web",
    "perf",
];

#[derive(Clone, Debug, PartialEq)]
pub struct CheckOptions {
    pub json: bool,
    pub quick: bool,
    pub only: Option<Vec<String>>,
    pub skip: Vec<String>,
    pub jobs: usize,
    pub record: bool,
}

impl CheckOptions {
    /// Whether a step runs at all: `--only` and `--skip` by request, and `--quick`, which leaves
    /// out `web` and `perf` (checks.md 3).
    pub fn wants(&self, step: &str) -> Option<&'static str> {
        if let Some(only) = &self.only
            && !only.iter().any(|s| s == step)
        {
            return Some("not in --only");
        }
        if self.skip.iter().any(|s| s == step) {
            return Some("--skip");
        }
        if self.quick && (step == "web" || step == "perf") {
            return Some("--quick");
        }
        None
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GenOptions {
    pub check: bool,
}

const CHECK_FLAGS: &[&str] = &[
    "--json", "--quick", "--only", "--skip", "--jobs", "--record",
];
const GEN_FLAGS: &[&str] = &["--check"];

pub fn parse_check(args: &[String]) -> Result<CheckOptions, Problem> {
    let mut o = CheckOptions {
        json: false,
        quick: false,
        only: None,
        skip: Vec::new(),
        jobs: 2,
        record: false,
    };
    let mut i = 0;
    while i < args.len() {
        let (flag, inline) = match args[i].split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (args[i].clone(), None),
        };
        let mut value = || -> Result<String, Problem> {
            if let Some(v) = inline.clone() {
                return Ok(v);
            }
            i += 1;
            args.get(i).cloned().ok_or_else(|| {
                Problem::new(
                    "check.usage",
                    format!("{flag} needs a value"),
                    json!({"flag": flag}),
                )
            })
        };
        match flag.as_str() {
            "--json" => o.json = true,
            "--quick" => o.quick = true,
            "--record" => o.record = true,
            "--only" => o.only = Some(step_list(&value()?)?),
            "--skip" => o.skip = step_list(&value()?)?,
            "--jobs" => {
                let v = value()?;
                o.jobs = match v.parse::<usize>() {
                    Ok(n) if n > 0 => n,
                    _ => {
                        return Err(Problem::new(
                            "check.usage",
                            format!("--jobs takes a positive integer; got '{v}'"),
                            json!({"flag": "--jobs", "got": v}),
                        ));
                    }
                }
            }
            _ => return Err(unknown("flag", &flag, CHECK_FLAGS)),
        }
        i += 1;
    }
    Ok(o)
}

pub fn parse_gen(args: &[String]) -> Result<GenOptions, Problem> {
    let mut o = GenOptions { check: false };
    for a in args {
        match a.as_str() {
            "--check" => o.check = true,
            _ => return Err(unknown("flag", a, GEN_FLAGS)),
        }
    }
    Ok(o)
}

fn step_list(value: &str) -> Result<Vec<String>, Problem> {
    let mut out = Vec::new();
    for name in value.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if !STEPS.contains(&name) {
            return Err(unknown("step", name, STEPS));
        }
        out.push(name.to_string());
    }
    Ok(out)
}

/// `check.usage` with up to three suggestions, best first.
pub fn unknown(what: &str, got: &str, allowed: &[&str]) -> Problem {
    let suggestions = suggest(got, allowed);
    let hint = if suggestions.is_empty() {
        format!("it takes {}", allowed.join(", "))
    } else {
        format!("did you mean {}?", suggestions.join(" or "))
    };
    Problem::new(
        "check.usage",
        format!("There is no {what} '{got}'; {hint}"),
        json!({"got": got, "suggestions": suggestions, "allowed": allowed}),
    )
}

/// Names within an edit distance of a third of the longer name (at least 1), and names that start
/// with what was typed, ordered by distance.
pub fn suggest(got: &str, allowed: &[&str]) -> Vec<String> {
    let mut scored: Vec<(usize, &str)> = allowed
        .iter()
        .filter_map(|c| {
            let d = distance(&got.to_lowercase(), &c.to_lowercase());
            let limit = (got.len().max(c.len()) / 3).max(1);
            (d <= limit || (got.len() >= 2 && c.starts_with(got))).then_some((d, *c))
        })
        .collect();
    scored.sort();
    scored
        .into_iter()
        .take(3)
        .map(|(_, c)| c.to_string())
        .collect()
}

/// Levenshtein distance over characters.
pub fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = if ca == *cb {
                prev
            } else {
                1 + prev.min(row[j]).min(row[j + 1])
            };
            prev = cur;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn flags_parse() {
        let o = parse_check(&args("--json --quick --only deps,fmt --jobs 3 --record")).unwrap();
        assert!(o.json && o.quick && o.record);
        assert_eq!(o.only, Some(vec!["deps".to_string(), "fmt".to_string()]));
        assert_eq!(o.jobs, 3);
        let o = parse_check(&args("--skip=perf,web")).unwrap();
        assert_eq!(o.skip, vec!["perf".to_string(), "web".to_string()]);
        assert_eq!(o.wants("web"), Some("--skip"));
        assert_eq!(o.wants("deps"), None);
        assert!(parse_gen(&args("--check")).unwrap().check);
        assert!(!parse_gen(&args("")).unwrap().check);
        assert_eq!(
            parse_gen(&args("--shared")).unwrap_err().code,
            "check.usage"
        );
    }

    #[test]
    fn unknown_step_is_refused_with_a_suggestion() {
        let p = parse_check(&args("--only clipy")).unwrap_err();
        assert_eq!(p.code, "check.usage");
        assert_eq!(p.detail["suggestions"], json!(["clippy"]));
        let p = parse_check(&args("--quik")).unwrap_err();
        assert_eq!(p.detail["suggestions"], json!(["--quick"]));
        let p = parse_check(&args("--jobs 0")).unwrap_err();
        assert_eq!(p.code, "check.usage");
    }

    #[test]
    fn quick_leaves_out_web_and_perf() {
        let o = parse_check(&args("--quick")).unwrap();
        assert_eq!(o.wants("web"), Some("--quick"));
        assert_eq!(o.wants("perf"), Some("--quick"));
        assert_eq!(o.wants("test"), None);
    }

    #[test]
    fn distance_counts_edits() {
        assert_eq!(distance("clipy", "clippy"), 1);
        assert_eq!(distance("", "abc"), 3);
        assert_eq!(distance("relaod", "reload"), 2);
    }
}
