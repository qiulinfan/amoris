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

/// Parse clang-style diagnostics (`file:line:col: error: message`) out of compiler output, and
/// the linkers' and Ninja's own errors (`lld-link: error: ...`), which name no source line.
pub fn parse_compiler_diagnostics(text: &str) -> Vec<Diagnostic> {
    let mut out = vec![];
    for line in text.lines() {
        for tool in ["lld-link: error:", "ld.lld: error:", "ld: error:", "ninja: error:", "clang++: error:", "clang: error:"] {
            if let Some(msg) = line.strip_prefix(tool) {
                out.push(Diagnostic { severity: "error".into(), message: format!("{} {}", tool.trim_end_matches(" error:"), msg.trim()), file: None, line: None, column: None });
            }
        }
        // A Windows path starts with a drive letter and its colon (`C:\x.cpp:10:5: error: ...`).
        let b = line.as_bytes();
        let drive = b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/');
        let (prefix, body) = if drive { line.split_at(2) } else { ("", line) };
        let mut parts = body.splitn(4, ':');
        let (Some(file), Some(l), Some(c), Some(rest)) = (parts.next(), parts.next(), parts.next(), parts.next()) else { continue };
        let file = format!("{prefix}{file}");
        let (Ok(l), Ok(c)) = (l.trim().parse::<u32>(), c.trim().parse::<u32>()) else { continue };
        let rest = rest.trim();
        let (sev, msg) = match rest.split_once(':') {
            Some((s, m)) if ["error", "warning", "note", "fatal error"].contains(&s.trim()) => (s.trim().to_string(), m.trim().to_string()),
            _ => continue,
        };
        out.push(Diagnostic { severity: sev, message: msg, file: Some(file), line: Some(l), column: Some(c) });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::parse_compiler_diagnostics;

    #[test]
    fn diagnostics_keep_a_drive_letter() {
        let d = parse_compiler_diagnostics("C:\\src\\a.cpp:10:5: error: no member named 'x'\n/usr/b.cpp:3:1: warning: unused\n");
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].file.as_deref(), Some("C:\\src\\a.cpp"));
        assert_eq!((d[0].line, d[0].column), (Some(10), Some(5)));
        assert_eq!(d[0].message, "no member named 'x'");
        assert_eq!(d[1].file.as_deref(), Some("/usr/b.cpp"));
    }

    #[test]
    fn linker_errors_are_diagnostics() {
        let d = parse_compiler_diagnostics("lld-link: error: undefined symbol: foo\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].message, "lld-link: undefined symbol: foo");
        assert!(d[0].file.is_none());
    }
}
