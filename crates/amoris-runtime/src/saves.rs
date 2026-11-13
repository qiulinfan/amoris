//! Saves as commands see them (docs/spec/versions.md 6; server.md 3): `saves.write` writes the
//! world shown as a save (amoris-persist's `write_save`) to `<project>/.amoris/saves/<name>.save`,
//! `saves.list` lists them, and `saves.load` reads one into the current versions (`load_save`)
//! and restores it ([`crate::Game::load_save`]). A save holds world data only; none of these
//! changes the files of the project itself.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use amoris_contract::{Problem, detail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The folder of a project's saves, under its ignored `.amoris/`.
pub const DIR: &str = ".amoris/saves";

/// The extension of a save file.
const EXT: &str = "save";

/// The longest save name, bytes.
const MAX_NAME: usize = 64;

/// `saves.write`'s parameters.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SavesWriteParams {
    /// The save's name: 1 to 64 letters, digits, `_` and `-`. A save of that name is replaced.
    pub name: String,
    /// A label kept in the save's header (shown by `saves.list`).
    #[serde(default)]
    pub label: Option<String>,
}

/// `saves.load`'s parameters.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SavesLoadParams {
    /// The save's name, as `saves.write` was given it.
    pub name: String,
}

/// `request.invalid_value` for a name a save file cannot have.
fn bad_name(name: &str) -> Problem {
    Problem::new(
        "request.invalid_value",
        format!("'{name}' is not a save name: use 1 to {MAX_NAME} letters, digits, '_' and '-'."),
        detail([("path", json!("/name")), ("got", json!(name))]),
    )
}

/// The file of the save `name` under `root`, once the name is checked.
pub fn path(root: &Path, name: &str) -> Result<PathBuf, Problem> {
    let ok = !name.is_empty()
        && name.len() <= MAX_NAME
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if !ok {
        return Err(bad_name(name));
    }
    Ok(root.join(DIR).join(format!("{name}.{EXT}")))
}

/// The save `name`'s file as answers name it: relative to the project, `/`-separated on every
/// platform, as the host protocol's project paths are.
pub fn shown(name: &str) -> String {
    format!("{DIR}/{name}.{EXT}")
}

fn io_problem(path: &Path, e: &std::io::Error) -> Problem {
    Problem::new(
        "saves.unwritable",
        format!("{}: {e}.", path.display()),
        detail([
            ("path", json!(path.display().to_string())),
            ("error", json!(e.to_string())),
        ]),
    )
}

/// The names of the saves under `root`, ascending.
pub fn names(root: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(root.join(DIR))
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let p = e.ok()?.path();
            if p.extension()? != EXT {
                return None;
            }
            let stem = p.file_stem()?.to_str()?.to_owned();
            path(root, &stem).ok().map(|_| stem)
        })
        .collect();
    out.sort();
    out
}

/// Writes `bytes` as the save `name`, through a temporary file, so a save that fails to write
/// leaves the one it would replace.
pub fn write(root: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, Problem> {
    let file = path(root, name)?;
    let dir = root.join(DIR);
    std::fs::create_dir_all(&dir).map_err(|e| io_problem(&dir, &e))?;
    let tmp = dir.join(format!(".{name}.{EXT}.tmp"));
    std::fs::write(&tmp, bytes).map_err(|e| io_problem(&tmp, &e))?;
    std::fs::rename(&tmp, &file).map_err(|e| io_problem(&file, &e))?;
    Ok(file)
}

/// The bytes of the save `name`; `saves.not_found` with the saves there are.
pub fn read(root: &Path, name: &str) -> Result<Vec<u8>, Problem> {
    let file = path(root, name)?;
    std::fs::read(&file).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            let saves = names(root);
            let mut d = detail([("name", json!(name)), ("saves", json!(saves))]);
            let suggested = amoris_contract::suggest_names(name, saves.iter().map(String::as_str));
            if let Some(best) = suggested.first() {
                d.insert("suggestion".into(), json!(best));
            }
            Problem::new(
                "saves.not_found",
                format!("No save is named '{name}' (saves.list lists them)."),
                d,
            )
        } else {
            io_problem(&file, &e)
        }
    })
}

/// The meta a save is written with: the tick and the world hash it was saved at, which
/// `saves.list` shows without reading the snapshot.
pub fn meta(tick: u64, world_hash: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("tick".to_owned(), tick.to_string()),
        ("world_hash".to_owned(), world_hash.to_owned()),
    ])
}

/// `saves.list`: every save under `root` with its size, label, tick and world hash, or the reason
/// its header does not read.
pub fn list(root: &Path) -> Value {
    let saves: Vec<Value> = names(root)
        .into_iter()
        .map(|name| {
            let mut v = json!({ "name": name });
            match read(root, &name) {
                Ok(bytes) => {
                    v["bytes"] = json!(bytes.len());
                    match amoris_persist::save::save_header(&bytes) {
                        Ok(h) => {
                            v["label"] = json!(h.label);
                            if let Some(t) = h.meta.get("tick").and_then(|t| t.parse::<u64>().ok())
                            {
                                v["tick"] = json!(t);
                            }
                            if let Some(w) = h.meta.get("world_hash") {
                                v["world_hash"] = json!(w);
                            }
                        }
                        Err(p) => v["unreadable"] = json!(p.code),
                    }
                }
                Err(p) => v["unreadable"] = json!(p.code),
            }
            v
        })
        .collect();
    json!({ "dir": DIR, "saves": saves })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked() {
        let root = Path::new("/p");
        assert_eq!(
            path(root, "slot-1_b").unwrap(),
            Path::new("/p/.amoris/saves/slot-1_b.save")
        );
        for bad in ["", "../x", "a/b", "a.b", "a b", "é", &"x".repeat(65)] {
            let p = path(root, bad).unwrap_err();
            assert_eq!(p.code, "request.invalid_value", "{bad}");
        }
        assert!(path(root, &"x".repeat(64)).is_ok());
    }
}
