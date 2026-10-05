//! Reading the check's configuration files. A malformed or misspelt one is
//! `check.config_invalid {path, line, message}` and exit code 2 before any step runs
//! (docs/spec/checks.md 11); unknown keys are refused, as the engine refuses unknown fields.

use crate::report::Problem;
use serde::de::DeserializeOwned;
use serde_json::json;
use std::path::Path;

pub fn load<T: DeserializeOwned>(root: &Path, rel: &str) -> Result<T, Problem> {
    let text = std::fs::read_to_string(root.join(rel))
        .map_err(|e| invalid(rel, None, &format!("cannot read it: {e}")))?;
    parse(rel, &text)
}

pub fn parse<T: DeserializeOwned>(rel: &str, text: &str) -> Result<T, Problem> {
    toml::from_str(text).map_err(|e| {
        let line = e
            .span()
            .map(|s| text[..s.start.min(text.len())].matches('\n').count() + 1);
        invalid(rel, line, e.message())
    })
}

pub fn invalid(rel: &str, line: Option<usize>, message: &str) -> Problem {
    let message = message.trim().to_string();
    let at = line.map_or(String::new(), |l| format!(":{l}"));
    Problem::new(
        "check.config_invalid",
        format!("{rel}{at}: {message}"),
        json!({"path": rel, "line": line, "message": message}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct C {
        limit: u32,
    }

    #[test]
    fn errors_name_the_line() {
        let p = parse::<C>("tools/x.toml", "# c\nlimit = 800\nlimt = 3\n").unwrap_err();
        assert_eq!(p.code, "check.config_invalid");
        assert_eq!(p.detail["line"], json!(3));
        assert!(p.message.contains("limt"), "{}", p.message);
    }
}
