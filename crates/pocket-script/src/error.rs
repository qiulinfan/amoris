//! Script errors (docs/spec/script-sandbox.md 5): every failure reaches agents and tools as the
//! contract's `{code, message, detail}`, with the TypeScript file, line and column through the
//! modules' source maps, and the tick, system and entity when known.

use std::fmt;

use pocket_contract::{Detail, Problem};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// Where in the pipeline an error arose.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorPhase {
    Compile,
    Lint,
    Load,
    Run,
    WriteBack,
}

/// A TypeScript position: 1-based line, 1-based column in UTF-16 code units (as `tsc` counts).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SourceLocation {
    pub file: String,
    pub line: u32,
    pub column: u32,
}

/// One frame of a mapped stack: project frames by their file, prelude frames under `pocket`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct StackFrame {
    pub at: SourceLocation,
    pub function: Option<String>,
}

/// The JavaScript error a failure came from: `{name: "TypeError", message}`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct JsError {
    pub name: String,
    pub message: String,
}

/// What a script error carries beside its code and message (script-sandbox.md 5.1).
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct ScriptErrorDetail {
    pub phase: Option<ErrorPhase>,
    /// The first frame in project code.
    pub location: Option<SourceLocation>,
    /// Project and prelude frames, innermost first.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub stack: Vec<StackFrame>,
    pub tick: Option<u64>,
    pub system: Option<String>,
    /// The entity concerned, when known.
    pub entity: Option<f64>,
    pub component: Option<String>,
    pub field: Option<String>,
    pub value: Option<Value>,
    pub hint: Option<String>,
    pub js_error: Option<JsError>,
    /// The nearest names for an unknown one.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub suggestions: Vec<String>,
    /// Fields particular to the code (a binding's name, a budget, an import).
    #[serde(skip_serializing_if = "Map::is_empty", default)]
    pub extra: Map<String, Value>,
}

/// A script error: `{code, message, detail}`.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct ScriptError {
    pub code: String,
    /// One plain sentence.
    pub message: String,
    /// Boxed: errors travel in `Result`s on every path, and the detail is large.
    pub detail: Box<ScriptErrorDetail>,
}

impl ScriptError {
    pub fn new(code: &str, message: impl Into<String>, phase: ErrorPhase) -> ScriptError {
        ScriptError {
            code: code.to_owned(),
            message: message.into(),
            detail: Box::new(ScriptErrorDetail {
                phase: Some(phase),
                ..ScriptErrorDetail::default()
            }),
        }
    }

    pub fn at(mut self, file: &str, line: u32, column: u32) -> ScriptError {
        self.detail.location = Some(SourceLocation {
            file: file.to_owned(),
            line,
            column,
        });
        self
    }

    pub fn with(mut self, key: &str, value: Value) -> ScriptError {
        self.detail.extra.insert(key.to_owned(), value);
        self
    }

    pub fn hint(mut self, hint: impl Into<String>) -> ScriptError {
        self.detail.hint = Some(hint.into());
        self
    }

    pub fn component(mut self, name: &str) -> ScriptError {
        self.detail.component = Some(name.to_owned());
        self
    }

    pub fn field(mut self, name: &str) -> ScriptError {
        self.detail.field = Some(name.to_owned());
        self
    }

    pub fn entity(mut self, id: f64) -> ScriptError {
        self.detail.entity = Some(id);
        self
    }

    pub fn value(mut self, v: Value) -> ScriptError {
        self.detail.value = Some(v);
        self
    }

    pub fn suggest(mut self, names: Vec<String>) -> ScriptError {
        self.detail.suggestions = names;
        self
    }

    /// Whether this error is a fault (script-sandbox.md 4.3): platform-dependent, so it poisons
    /// the world inside a tick instead of failing one call.
    pub fn is_fault(&self) -> bool {
        matches!(
            self.code.as_str(),
            "script.out_of_memory" | "script.stack_overflow" | "sim.internal"
        )
    }

    /// The contract's problem object: the detail as JSON, empty fields left out.
    pub fn to_problem(&self) -> Problem {
        let mut detail = Detail::new();
        if let Ok(Value::Object(m)) = serde_json::to_value(&self.detail) {
            for (k, v) in m {
                if k == "extra" {
                    continue;
                }
                if !v.is_null() {
                    detail.insert(k, v);
                }
            }
        }
        for (k, v) in &self.detail.extra {
            detail.insert(k.clone(), v.clone());
        }
        Problem::new(&self.code, self.message.clone(), detail)
    }
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)?;
        if let Some(l) = &self.detail.location {
            write!(f, " ({}:{}:{})", l.file, l.line, l.column)?;
        }
        Ok(())
    }
}

impl std::error::Error for ScriptError {}

/// A number for a detail: JSON holds finite numbers only, so others are written as text.
pub fn num(x: f64) -> Value {
    if x.is_finite() {
        pocket_contract::codes::num(x)
    } else if x.is_nan() {
        json!("NaN")
    } else if x > 0.0 {
        json!("Infinity")
    } else {
        json!("-Infinity")
    }
}

/// 1-based line and column (UTF-16 code units) of a byte offset in a source text.
pub fn line_col(source: &str, offset: u32) -> (u32, u32) {
    let offset = usize::try_from(offset)
        .unwrap_or(usize::MAX)
        .min(source.len());
    let mut cut = offset;
    while !source.is_char_boundary(cut) {
        cut -= 1;
    }
    let before = &source[..cut];
    let line = before.matches('\n').count() + 1;
    let start = before.rfind('\n').map_or(0, |i| i + 1);
    let column: usize = before[start..].chars().map(char::len_utf16).sum::<usize>() + 1;
    (to_u32(line), to_u32(column))
}

/// A count as `u32`, saturating.
pub fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Hints for the mistakes master's agents made and those this design invites
/// (script-sandbox.md 5.3), keyed by the JavaScript error's name and message.
pub fn hint_for(name: &str, message: &str) -> Option<String> {
    const REMOVED: &[(&str, &str)] = &[
        (
            "Date",
            "Date is not available in game scripts: use ctx.time or ctx.tick",
        ),
        (
            "performance",
            "performance is not available in game scripts: use ctx.time",
        ),
        (
            "Promise",
            "Promises are not available: keep waiting state in a component",
        ),
        (
            "queueMicrotask",
            "There is no job queue: keep pending work in a component",
        ),
        (
            "setTimeout",
            "There are no timers: count ticks down in a component",
        ),
        (
            "setInterval",
            "There are no timers: count ticks down in a component",
        ),
        (
            "WeakRef",
            "WeakRef is not available: it depends on garbage collection",
        ),
        (
            "FinalizationRegistry",
            "FinalizationRegistry is not available",
        ),
        ("eval", "eval is not available: code comes through modules"),
        (
            "SharedArrayBuffer",
            "SharedArrayBuffer is not available in game scripts",
        ),
        ("Atomics", "Atomics is not available in game scripts"),
        (
            "require",
            "require is not available: use import from \"pocket\" or ./paths",
        ),
        ("process", "process is not available in game scripts"),
    ];
    if name == "ReferenceError" {
        for (global, hint) in REMOVED {
            if message.starts_with(&format!("{global} is not defined"))
                || message.starts_with(&format!("'{global}' is not defined"))
            {
                return Some((*hint).to_owned());
            }
        }
        const POCKET: &[&str] = &["game", "system", "component", "field", "freeze"];
        for export in POCKET {
            if message.starts_with(&format!("{export} is not defined"))
                || message.starts_with(&format!("'{export}' is not defined"))
            {
                return Some(format!("import {{ {export} }} from \"pocket\""));
            }
        }
    }
    if name == "TypeError" && (message.contains("read-only") || message.contains("not extensible"))
    {
        return Some(
            "module values and world.get copies are frozen: copy the value first, keep state in a \
             component, and write with ctx.world.set"
                .to_owned(),
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_count_utf16_units() {
        let src = "a\n\u{1F600}x = 1";
        assert_eq!(line_col(src, 2), (2, 1));
        // The emoji is two UTF-16 units and four bytes.
        assert_eq!(line_col(src, 6), (2, 3));
    }

    #[test]
    fn problems_carry_the_detail() {
        let e = ScriptError::new("script.exception", "boom", ErrorPhase::Run)
            .at("scripts/a.ts", 3, 7)
            .with("budget", json!(10));
        let p = e.to_problem();
        assert_eq!(p.code, "script.exception");
        assert_eq!(p.detail["location"]["line"], json!(3));
        assert_eq!(p.detail["budget"], json!(10));
        assert_eq!(p.detail["phase"], json!("run"));
    }
}
