//! Command-line parsing for `pocket` (architecture.md 4.13): subcommands, flags with values, and
//! the refusal of unknown flags with a suggestion (charter 3.4; checks.md 11, exit code 2).

use pocket_contract::{Problem, detail, suggest_names};
use serde_json::json;

/// Parsed flags of one subcommand: positional arguments and `--flag [value]` pairs.
#[derive(Debug, Default)]
pub struct Args {
    pub positional: Vec<String>,
    flags: Vec<(String, Option<String>)>,
}

/// A flag a subcommand takes: its name and whether it takes a value.
pub type Flag = (&'static str, bool);

/// `check.usage {flag, suggestions}`.
pub fn usage(message: String, flag: &str, valid: &[&str]) -> Problem {
    let suggestions = suggest_names(flag, valid.iter().copied());
    Problem::new(
        "check.usage",
        message,
        detail([("flag", json!(flag)), ("suggestions", json!(suggestions))]),
    )
}

impl Args {
    /// Parses `args` against the subcommand's flags.
    pub fn parse(args: &[String], known: &[Flag]) -> Result<Args, Problem> {
        let names: Vec<&str> = known.iter().map(|(n, _)| *n).collect();
        let mut out = Args::default();
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if let Some(flag) = a.strip_prefix("--") {
                let (name, inline) = match flag.split_once('=') {
                    Some((n, v)) => (n, Some(v.to_owned())),
                    None => (flag, None),
                };
                let Some((_, takes)) = known.iter().find(|(n, _)| *n == name) else {
                    let s = suggest_names(name, names.iter().copied());
                    return Err(usage(
                        format!("pocket does not take --{name} here; did you mean {s:?}?"),
                        name,
                        &names,
                    ));
                };
                let value = if *takes {
                    match inline {
                        Some(v) => Some(v),
                        None => {
                            i += 1;
                            Some(args.get(i).cloned().ok_or_else(|| {
                                usage(format!("--{name} needs a value"), name, &names)
                            })?)
                        }
                    }
                } else {
                    None
                };
                out.flags.push((name.to_owned(), value));
            } else {
                out.positional.push(a.clone());
            }
            i += 1;
        }
        Ok(out)
    }

    pub fn has(&self, name: &str) -> bool {
        self.flags.iter().any(|(n, _)| n == name)
    }

    pub fn value(&self, name: &str) -> Option<&str> {
        self.flags
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .and_then(|(_, v)| v.as_deref())
    }

    /// A number flag.
    pub fn number<T: std::str::FromStr>(&self, name: &str) -> Result<Option<T>, Problem> {
        match self.value(name) {
            None => Ok(None),
            Some(v) => v
                .parse::<T>()
                .map(Some)
                .map_err(|_| usage(format!("--{name} takes a number, not '{v}'"), name, &[])),
        }
    }

    /// A comma-separated list flag.
    pub fn list(&self, name: &str) -> Option<Vec<String>> {
        self.value(name).map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
    }
}

/// Lowercase hex of bytes.
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Bytes of lowercase or uppercase hex.
pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_values_and_refusals() {
        let args: Vec<String> = ["proj", "--json", "--seeds", "1,2", "--only=fork"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let a = Args::parse(&args, &[("json", false), ("seeds", true), ("only", true)]).unwrap();
        assert_eq!(a.positional, ["proj"]);
        assert!(a.has("json"));
        assert_eq!(a.list("seeds").unwrap(), ["1", "2"]);
        assert_eq!(a.value("only"), Some("fork"));
        let bad = Args::parse(&["--sed".into()], &[("seeds", true)]).unwrap_err();
        assert_eq!(bad.code, "check.usage");
        assert_eq!(unhex(&hex(&[0, 1, 254])).unwrap(), [0, 1, 254]);
    }
}
