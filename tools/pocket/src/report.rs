//! Uniform command result: the same struct prints as text or JSON.

use serde::Serialize;
use serde_json::Value;

#[derive(Serialize, Debug, Clone)]
pub struct Diagnostic {
    pub severity: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
}

#[derive(Serialize, Debug, Clone)]
pub struct Report {
    pub command: String,
    pub ok: bool,
    pub summary: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub data: Value,
    pub elapsed_ms: u128,
}

impl Report {
    pub fn success(command: &str, summary: impl Into<String>) -> Self {
        Report { command: command.to_string(), ok: true, summary: summary.into(), diagnostics: vec![], data: Value::Null, elapsed_ms: 0 }
    }
    pub fn failure(command: &str, summary: impl Into<String>) -> Self {
        Report { command: command.to_string(), ok: false, summary: summary.into(), diagnostics: vec![], data: Value::Null, elapsed_ms: 0 }
    }
    pub fn human(&self) -> String {
        let mut out = String::new();
        for d in &self.diagnostics {
            match (&d.file, d.line) {
                // A message of several lines (a type error's elaboration) keeps its later lines indented.
                (Some(f), Some(l)) => out.push_str(&format!("{}:{}:{}: {}: {}\n", f, l, d.column.unwrap_or(0), d.severity, d.message.replace('\n', "\n    "))),
                _ => out.push_str(&format!("{}: {}\n", d.severity, d.message.replace('\n', "\n    "))),
            }
        }
        out.push_str(&format!("{}: {}\n", if self.ok { "ok" } else { "FAILED" }, self.summary));
        if !self.data.is_null() {
            if let Some(obj) = self.data.as_object() {
                for (k, v) in obj {
                    if v.is_string() || v.is_number() || v.is_boolean() {
                        out.push_str(&format!("  {k}: {v}\n"));
                    }
                }
            }
        }
        out
    }
}

/// Parse clang-style diagnostics (`file:line:col: error: message`) out of compiler output.
pub fn parse_compiler_diagnostics(text: &str) -> Vec<Diagnostic> {
    let mut out = vec![];
    for line in text.lines() {
        let mut parts = line.splitn(4, ':');
        let (Some(file), Some(l), Some(c), Some(rest)) = (parts.next(), parts.next(), parts.next(), parts.next()) else { continue };
        let (Ok(l), Ok(c)) = (l.trim().parse::<u32>(), c.trim().parse::<u32>()) else { continue };
        let rest = rest.trim();
        let (sev, msg) = match rest.split_once(':') {
            Some((s, m)) if ["error", "warning", "note", "fatal error"].contains(&s.trim()) => (s.trim().to_string(), m.trim().to_string()),
            _ => continue,
        };
        out.push(Diagnostic { severity: sev, message: msg, file: Some(file.to_string()), line: Some(l), column: Some(c) });
    }
    out
}
