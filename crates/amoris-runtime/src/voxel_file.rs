//! Block volumes' cells files (docs/spec/voxels.md 2.3, liquids.md 2): `"AMVX"`, a version, a
//! chunk count, then per chunk in key order its key and 4096 palette indices, and from version 2 a
//! flag byte followed, when it is 1, by the chunk's 4096 state bytes; little-endian, uncompressed.
//! Version 1 files (no states) still read. A scene names one in `Voxels.source`; loading the
//! project reads it into the volume's chunks.

use std::path::Path;

use amoris_contract::{Problem, detail};
use amoris_sim::voxels::{CHUNK_CELLS, Cells, Chunk, base64_encode};
use serde_json::{Value, json};

use crate::scene::Scene;

const MAGIC: &[u8; 4] = b"AMVX";
const VERSION: u32 = 2;

fn refused(path: &str, why: String) -> Problem {
    Problem::new(
        "voxels.file",
        format!("The cells file {path} cannot be read: {why}."),
        detail([("path", json!(path)), ("reason", json!(why))]),
    )
}

/// The chunks of a cells file, checked to be in strictly increasing key order.
pub fn read_cells(path: &str, bytes: &[u8]) -> Result<Vec<Chunk>, Problem> {
    let mut at = 0usize;
    let mut take = |n: usize| -> Result<&[u8], Problem> {
        let part = bytes
            .get(at..at + n)
            .ok_or_else(|| refused(path, format!("it ends after {} bytes", bytes.len())))?;
        at += n;
        Ok(part)
    };
    let word = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    if take(4)? != MAGIC {
        return Err(refused(path, "it does not start with AMVX".into()));
    }
    let version = word(take(4)?);
    if !(1..=VERSION).contains(&version) {
        return Err(refused(
            path,
            format!("version {version} is not 1 or {VERSION}"),
        ));
    }
    let count = usize::try_from(word(take(4)?)).unwrap_or(usize::MAX);
    let mut chunks: Vec<Chunk> = Vec::with_capacity(count.min(1 << 16));
    for _ in 0..count {
        let k = take(12)?;
        let key = [0, 1, 2]
            .map(|a| i32::from_le_bytes([k[4 * a], k[4 * a + 1], k[4 * a + 2], k[4 * a + 3]]));
        if chunks.last().is_some_and(|c| c.key >= key) {
            return Err(refused(path, format!("chunk {key:?} is out of key order")));
        }
        let cells = Cells(take(CHUNK_CELLS)?.into());
        let states = if version >= 2 && take(1)?[0] == 1 {
            Some(Cells(take(CHUNK_CELLS)?.into()))
        } else {
            None
        };
        chunks.push(Chunk {
            key,
            cells,
            edited: 0,
            states,
        });
    }
    if at != bytes.len() {
        return Err(refused(
            path,
            format!("{} bytes follow its {count} chunks", bytes.len() - at),
        ));
    }
    Ok(chunks)
}

/// The bytes of a version 2 cells file holding `chunks` (in key order).
pub fn write_cells(chunks: &[Chunk]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + chunks.len() * (13 + CHUNK_CELLS));
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    let count = u32::try_from(chunks.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&count.to_le_bytes());
    for c in chunks {
        for k in c.key {
            out.extend_from_slice(&k.to_le_bytes());
        }
        out.extend_from_slice(&c.cells.0);
        match &c.states {
            Some(s) => {
                out.push(1);
                out.extend_from_slice(&s.0);
            }
            None => out.push(0),
        }
    }
    out
}

/// Fills each scene volume that names a `source` and holds no chunks with that file's chunks,
/// read through `read` (a project-relative path to its bytes).
pub fn resolve(
    scene: &mut Scene,
    read: impl Fn(&str) -> Result<Vec<u8>, Problem>,
) -> Result<(), Problem> {
    for entity in &mut scene.entities {
        let Some(Value::Object(v)) = entity.components.get_mut("Voxels") else {
            continue;
        };
        let source = match v.get("source") {
            Some(Value::String(s)) if !s.is_empty() => s.clone(),
            _ => continue,
        };
        if v.get("chunks")
            .is_some_and(|c| c.as_array().is_some_and(|a| !a.is_empty()))
        {
            continue;
        }
        let chunks = read_cells(&source, &read(&source)?)?;
        let json: Vec<Value> = chunks
            .iter()
            .map(|c| {
                let mut j = json!({"key": c.key, "cells": base64_encode(&c.cells.0)});
                if let Some(st) = &c.states {
                    j["states"] = json!(base64_encode(&st.0));
                }
                j
            })
            .collect();
        v.insert("chunks".into(), Value::Array(json));
    }
    Ok(())
}

/// [`resolve`] against files under `root`.
pub fn resolve_under(scene: &mut Scene, root: &Path) -> Result<(), Problem> {
    resolve(scene, |rel| {
        let inside = Path::new(rel)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)));
        if !inside {
            return Err(refused(rel, "the path leaves the project".into()));
        }
        std::fs::read(root.join(rel)).map_err(|e| refused(rel, e.to_string()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(key: [i32; 3], fill: u8) -> Chunk {
        Chunk {
            key,
            cells: Cells(vec![fill; CHUNK_CELLS].into_boxed_slice()),
            edited: 0,
            states: None,
        }
    }

    #[test]
    fn cells_files_round_trip_and_refuse_damage() {
        let chunks = vec![chunk([-1, 0, 3], 1), chunk([0, 0, 0], 2)];
        let bytes = write_cells(&chunks);
        assert_eq!(read_cells("w", &bytes).unwrap(), chunks);
        assert!(read_cells("w", &bytes[..bytes.len() - 1]).is_err());
        let mut bad = bytes.clone();
        bad[4] = 3;
        assert!(read_cells("w", &bad).is_err());
        // States round trip; a version 1 file (no flag bytes) still reads.
        let mut stated = chunk([2, 0, 0], 3);
        stated.states = Some(Cells(vec![5; CHUNK_CELLS].into_boxed_slice()));
        let with = vec![chunk([0, 0, 0], 2), stated];
        assert_eq!(read_cells("w", &write_cells(&with)).unwrap(), with);
        let mut v1 = write_cells(&chunks);
        v1[4] = 1;
        v1.retain(|_| true);
        let mut old = v1[..12].to_vec();
        for c in &chunks {
            for k in c.key {
                old.extend_from_slice(&k.to_le_bytes());
            }
            old.extend_from_slice(&c.cells.0);
        }
        assert_eq!(read_cells("w", &old).unwrap(), chunks);
        let swapped = write_cells(&[chunk([0, 0, 0], 2), chunk([-1, 0, 3], 1)]);
        assert_eq!(read_cells("w", &swapped).unwrap_err().code, "voxels.file");
    }

    #[test]
    fn a_scene_volume_takes_its_files_chunks() {
        let mut scene = Scene::from_json(
            &json!({"format": "amoris-scene", "version": 1, "entities": [
                {"name": "World", "components": {"Voxels": {"source": "world.cells",
                  "palette": [{"name": "air"}, {"name": "stone", "solid": true,
                              "faces": ["s", "s", "s"]}]}}}
            ]})
            .to_string(),
        )
        .unwrap();
        let bytes = write_cells(&[chunk([0, 0, 0], 1)]);
        resolve(&mut scene, |p| {
            assert_eq!(p, "world.cells");
            Ok(bytes.clone())
        })
        .unwrap();
        let v: amoris_sim::voxels::Voxels =
            serde_json::from_value(scene.entities[0].components["Voxels"].clone()).unwrap();
        v.validate().unwrap();
        assert_eq!(v.get([3, 4, 5]), 1);
    }
}
