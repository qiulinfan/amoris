//! The project's files as commands see them (docs/spec/host-protocol.md 4): `project.info`,
//! `project.save` (the world written back as `scene.json`), and `scripts.list`, `scripts.read` and
//! `scripts.write` under `scripts/`. Paths are relative to the project root and may not leave it.
//! None of these changes the world; `scripts.apply` is what loads what was written.

use std::path::{Component, Path, PathBuf};

use bevy_ecs::prelude::World;
use pocket_contract::{Problem, detail};
use pocket_sim::registry::FieldType;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::edit::components_json;
use crate::scene::{SCENE_FORMAT, project_component};

/// `scripts.read`'s parameters.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScriptReadParams {
    /// Under `scripts/`: `scripts/rules.ts` or `rules.ts`.
    pub path: String,
    /// Only these lines, 1-based and inclusive: `50-80`, `50-` (to the end), `-20` or `67`.
    #[serde(default)]
    pub lines: Option<String>,
    /// Each line prefixed with its number (`67| `), as breakpoints and errors count them.
    #[serde(default)]
    pub numbered: bool,
}

/// A text's lines `spec` (`a-b`, `a-`, `-b`, `a`; 1-based, inclusive), numbered if asked:
/// `(text, first, last, total)`. The host's server reads files the same way while the game is
/// held at a breakpoint (pocket-server `paused.rs`).
pub fn slice_lines(
    text: &str,
    spec: Option<&str>,
    numbered: bool,
) -> Result<(String, usize, usize, usize), Problem> {
    let all: Vec<&str> = text.lines().collect();
    let total = all.len();
    let bad = |why: String| {
        Problem::new(
            "request.invalid_value",
            why,
            detail([("path", json!("/lines")), ("lines", json!(total))]),
        )
    };
    let (first, last) = match spec.map(str::trim) {
        None | Some("") => (1, total),
        Some(s) => {
            let num = |t: &str| -> Result<usize, Problem> {
                t.trim()
                    .parse::<usize>()
                    .map_err(|_| bad(format!("'{s}' is not a line range (50-80, 50-, -20, 67).")))
            };
            match s.split_once('-') {
                Some((a, b)) => (
                    if a.trim().is_empty() { 1 } else { num(a)? },
                    if b.trim().is_empty() { total } else { num(b)? },
                ),
                None => {
                    let n = num(s)?;
                    (n, n)
                }
            }
        }
    };
    if first == 0 || first > last.max(1) || (first > total && total > 0) {
        return Err(bad(format!(
            "Lines {first} to {last} are not in the file, which has {total} lines."
        )));
    }
    let last = last.min(total);
    let width = last.to_string().len();
    let mut out = String::new();
    for (i, line) in all.iter().enumerate().take(last).skip(first - 1) {
        if numbered {
            out.push_str(&format!("{:>width$}| ", i + 1));
        }
        out.push_str(line);
        out.push('\n');
    }
    Ok((out, first, last, total))
}

/// `scripts.write`'s parameters.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScriptWriteParams {
    /// Under `scripts/`: `scripts/rules.ts` or `rules.ts`.
    pub path: String,
    /// The file's whole text.
    pub text: String,
}

/// `project.no_root`: a game built from data, not from a project directory.
pub fn no_root(command: &str) -> Problem {
    Problem::new(
        "project.no_root",
        format!("{command} needs a project directory; this game was built from data."),
        detail([("command", json!(command))]),
    )
}

fn io_problem(path: &Path, e: &std::io::Error) -> Problem {
    let code = if e.kind() == std::io::ErrorKind::NotFound {
        "project.missing_file"
    } else {
        "project.unreadable"
    };
    Problem::new(
        code,
        format!("{}: {e}.", path.display()),
        detail([
            ("path", json!(path.display().to_string())),
            ("error", json!(e.to_string())),
        ]),
    )
}

/// A script path normalized to `scripts/<rel>` and checked to stay inside `scripts/`.
pub fn script_path(path: &str) -> Result<String, Problem> {
    let rel = path.strip_prefix("scripts/").unwrap_or(path);
    let ok = !rel.is_empty()
        && Path::new(rel)
            .components()
            .all(|c| matches!(c, Component::Normal(_)));
    if !ok {
        return Err(Problem::new(
            "request.invalid_value",
            format!("'{path}' is not a path inside scripts/."),
            detail([("path", json!(path))]),
        ));
    }
    Ok(format!("scripts/{rel}"))
}

/// Every file under `dir`, as paths relative to `root` with `/`, sorted.
pub fn walk(root: &Path, dir: &Path) -> Vec<(String, u64)> {
    fn go(root: &Path, dir: &Path, out: &mut Vec<(String, u64)>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = rd.filter_map(Result::ok).collect();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for e in entries {
            let p = e.path();
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if p.is_dir() {
                go(root, &p, out);
            } else if let Ok(rel) = p.strip_prefix(root) {
                let key = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                out.push((key, e.metadata().map(|m| m.len()).unwrap_or(0)));
            }
        }
    }
    let mut out = Vec::new();
    go(root, dir, &mut out);
    out
}

/// The compile errors of a `scripts.refused` problem as diagnostics:
/// `[{file, line, column, code, message, severity}]`.
pub fn diagnostics(p: &Problem) -> Vec<Value> {
    let Some(errors) = p.detail.get("errors").and_then(Value::as_array) else {
        return vec![json!({"code": p.code, "message": p.message, "severity": "error"})];
    };
    errors
        .iter()
        .map(|e| {
            let loc = e.pointer("/detail/location");
            json!({
                "file": loc.and_then(|l| l.get("file")).cloned().unwrap_or(Value::Null),
                "line": loc.and_then(|l| l.get("line")).cloned().unwrap_or(Value::Null),
                "column": loc.and_then(|l| l.get("column")).cloned().unwrap_or(Value::Null),
                "code": e.get("code").cloned().unwrap_or(Value::Null),
                "message": e.get("message").cloned().unwrap_or(Value::Null),
                "severity": "error",
            })
        })
        .collect()
}

/// `scripts.list`: the files under `scripts/` with the last diagnostics that name them.
pub fn list(root: &Path, last: &[Value]) -> Value {
    let files: Vec<Value> = walk(root, &root.join("scripts"))
        .into_iter()
        .map(|(path, bytes)| {
            let diags: Vec<&Value> = last
                .iter()
                .filter(|d| d["file"].as_str() == Some(path.as_str()))
                .collect();
            json!({"path": path, "bytes": bytes, "diagnostics": diags})
        })
        .collect();
    Value::Array(files)
}

/// `scripts.read`: the whole text, or the lines asked for with `first`, `last` and `total`.
pub fn read(root: &Path, p: &ScriptReadParams) -> Result<Value, Problem> {
    let rel = script_path(&p.path)?;
    let full = root.join(&rel);
    let text = std::fs::read_to_string(&full).map_err(|e| io_problem(&full, &e))?;
    if p.lines.is_none() && !p.numbered {
        return Ok(json!({"path": rel, "text": text}));
    }
    let (text, first, last, total) = slice_lines(&text, p.lines.as_deref(), p.numbered)?;
    Ok(json!({"path": rel, "text": text, "first": first, "last": last, "total": total}))
}

/// Writes a script file; returns its normalized path.
pub fn write(root: &Path, p: &ScriptWriteParams) -> Result<String, Problem> {
    let rel = script_path(&p.path)?;
    let full = root.join(&rel);
    if let Some(dir) = full.parent() {
        std::fs::create_dir_all(dir).map_err(|e| io_problem(dir, &e))?;
    }
    std::fs::write(&full, &p.text).map_err(|e| io_problem(&full, &e))?;
    Ok(rel)
}

/// `project.info`.
pub fn info(root: &Path, name: &str, rate: u32) -> Value {
    let rel = |dir: &str| -> Vec<String> {
        walk(root, &root.join(dir))
            .into_iter()
            .map(|(p, _)| p)
            .collect()
    };
    json!({
        "name": name,
        "root": root.display().to_string(),
        "rate": rate,
        "scenes": if root.join("scene.json").is_file() { vec!["scene.json"] } else { vec![] },
        "scripts": rel("scripts"),
        "assets": rel("assets"),
    })
}

/// The world as a scene: every live entity in id order with its name and every component whole.
/// A scene spawns its entities in file order from id 1, so entity fields are renumbered to the
/// entities' places in the file (a field naming a destroyed entity becomes `null`).
pub fn scene_of(world: &World) -> Value {
    let live: Vec<(pocket_sim::EntityId, bevy_ecs::prelude::Entity)> =
        world.resource::<pocket_sim::EntityIndex>().iter().collect();
    let place = |id: u64| -> Value {
        live.iter()
            .position(|(i, _)| i.get() == id)
            .map_or(Value::Null, |n| json!(n + 1))
    };
    let mut entities = Vec::with_capacity(live.len());
    for (_, e) in &live {
        let mut comps = components_json(world, *e);
        for (cname, v) in comps.iter_mut() {
            let Some((schema, _)) = project_component(world, cname) else {
                continue;
            };
            for f in schema.fields.iter().filter(|f| f.ty == FieldType::Entity) {
                if let Some(slot) = v.get_mut(&*f.name)
                    && let Some(id) = slot.as_u64()
                {
                    *slot = place(id);
                }
            }
        }
        let mut ent = Map::new();
        if let Some(n) = world.get::<pocket_sim::Name>(*e) {
            ent.insert("name".into(), json!(n.as_str()));
        }
        ent.insert("components".into(), Value::Object(comps));
        entities.push(Value::Object(ent));
    }
    json!({"format": SCENE_FORMAT, "version": 1, "entities": entities})
}

/// The scene text: one entity per line, so a diff shows what changed.
pub fn scene_text(scene: &Value) -> String {
    let mut out =
        format!("{{\n  \"format\": \"{SCENE_FORMAT}\",\n  \"version\": 1,\n  \"entities\": [\n");
    let empty = Vec::new();
    let entities = scene["entities"].as_array().unwrap_or(&empty);
    for (i, e) in entities.iter().enumerate() {
        out.push_str("    ");
        out.push_str(&serde_json::to_string(e).unwrap_or_default());
        if i + 1 < entities.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("  ]\n}\n");
    out
}

/// `project.save`: writes the world to `scene.json`.
pub fn save(world: &World, root: &Path) -> Result<Value, Problem> {
    let scene = scene_of(world);
    let path: PathBuf = root.join("scene.json");
    std::fs::write(&path, scene_text(&scene)).map_err(|e| io_problem(&path, &e))?;
    let n = scene["entities"].as_array().map_or(0, Vec::len);
    Ok(json!({"files": ["scene.json"], "entities": n}))
}
