//! Compiler diagnostics from cargo's `--message-format=json` lines, which the `clippy`, `build`,
//! `wasm` and `test` steps turn into problems.

use serde::Deserialize;
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Diag {
    pub package: String,
    pub level: String,
    pub lint: Option<String>,
    pub message: String,
    pub path: Option<String>,
    pub line: Option<u64>,
}

impl Diag {
    pub fn at(&self) -> String {
        match (&self.path, self.line) {
            (Some(p), Some(l)) => format!("{p}:{l}"),
            (Some(p), None) => p.clone(),
            _ => String::new(),
        }
    }
}

#[derive(Deserialize)]
struct Message {
    reason: String,
    #[serde(default)]
    package_id: String,
    message: Option<Compiler>,
}

#[derive(Deserialize)]
struct Compiler {
    message: String,
    level: String,
    code: Option<Code>,
    #[serde(default)]
    spans: Vec<Span>,
}

#[derive(Deserialize)]
struct Code {
    code: String,
}

#[derive(Deserialize)]
struct Span {
    file_name: String,
    line_start: u64,
    is_primary: bool,
}

/// The package's name from a cargo package id: `path+file:///…/crates/pocket-sim#0.1.0`,
/// `registry+https://…#serde@1.0.229`, or the older `name version (source)`.
pub fn package_name(id: &str) -> String {
    match id.split_once('#') {
        Some((url, frag)) => match frag.split_once('@') {
            Some((name, _)) => name.to_string(),
            None => url
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(url)
                .to_string(),
        },
        None => id.split_whitespace().next().unwrap_or(id).to_string(),
    }
}

/// A diagnostic from one line of cargo's JSON output, if the line is an error or a warning that
/// points at code (the "aborting due to" and "N warnings emitted" summaries are left out).
pub fn parse(line: &str) -> Option<Diag> {
    if !line.starts_with('{') {
        return None;
    }
    let m: Message = serde_json::from_str(line).ok()?;
    if m.reason != "compiler-message" {
        return None;
    }
    let c = m.message?;
    if c.level != "error" && c.level != "warning" {
        return None;
    }
    let summary = c.message.starts_with("aborting due to") || c.message.contains(" emitted");
    if c.spans.is_empty() && summary {
        return None;
    }
    let span = c.spans.iter().find(|s| s.is_primary).or(c.spans.first());
    Some(Diag {
        package: package_name(&m.package_id),
        level: c.level,
        lint: c.code.map(|x| x.code),
        message: c.message,
        path: span.map(|s| s.file_name.replace('\\', "/")),
        line: span.map(|s| s.line_start),
    })
}

/// Every distinct diagnostic of an output (one per lib and test target is reported once).
pub fn all(text: &str) -> Vec<Diag> {
    let set: BTreeSet<Diag> = text.lines().filter_map(parse).collect();
    set.into_iter().collect()
}

/// The lines that are not cargo's JSON messages: what cargo and the tools printed themselves.
pub fn plain_tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.starts_with('{')).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_ids() {
        assert_eq!(
            package_name("path+file:///C:/r/crates/pocket-sim#0.1.0"),
            "pocket-sim"
        );
        assert_eq!(package_name("path+file:///C:/r/xtask#xtask@0.1.0"), "xtask");
        assert_eq!(
            package_name("registry+https://github.com/rust-lang/crates.io-index#serde@1.0.229"),
            "serde"
        );
        assert_eq!(package_name("serde 1.0.0 (registry+https://x)"), "serde");
    }

    #[test]
    fn a_clippy_message_becomes_a_diagnostic() {
        let line = r#"{"reason":"compiler-message","package_id":"path+file:///C:/r/crates/pocket-sim#0.1.0","manifest_path":"x","target":{},"message":{"rendered":"r","$message_type":"diagnostic","children":[],"level":"error","message":"use of a disallowed method `f64::sin`","spans":[{"byte_end":1,"byte_start":0,"column_end":1,"column_start":1,"expansion":null,"file_name":"crates\\pocket-sim\\src\\lib.rs","is_primary":true,"label":null,"line_end":4,"line_start":4,"suggested_replacement":null,"suggestion_applicability":null,"text":[]}],"code":{"code":"clippy::disallowed_methods","explanation":null}}}"#;
        let d = parse(line).unwrap();
        assert_eq!(d.package, "pocket-sim");
        assert_eq!(d.lint.as_deref(), Some("clippy::disallowed_methods"));
        assert_eq!(d.at(), "crates/pocket-sim/src/lib.rs:4");
        let summary = r#"{"reason":"compiler-message","package_id":"p#a@1","message":{"level":"error","message":"aborting due to 1 previous error","spans":[],"code":null}}"#;
        assert!(parse(summary).is_none());
        assert!(parse("   Compiling x").is_none());
        assert_eq!(all(&format!("{line}\n{line}\n")).len(), 1);
    }
}
