//! Block volumes' cells files (docs/spec/voxels.md 2.3): `"AMVX"`, a version, a chunk count, then
//! per chunk in key order its key and 4096 palette indices, little-endian, uncompressed. A scene
//! names one in `Voxels.source`; loading the project reads it into the volume's chunks.

use std::path::Path;

use amoris_contract::{Problem, detail};
use amoris_sim::voxels::{CHUNK_CELLS, Cells, Chunk, base64_encode};
use serde_json::{Value, json};

use crate::scene::Scene;

const MAGIC: &[u8; 4] = b"AMVX";
const VERSION: u32 = 1;
const CHUNK_BYTES: usize = 12 + CHUNK_CELLS;

fn refused(path: &str, why: String) -> Problem {
    Problem::new(
        "voxels.file",
        format!("The cells file {path} cannot be read: {why}."),
        detail([("path", json!(path)), ("reason", json!(why))]),
    )
}

/// The chunks of a cells file, checked to be in strictly increasing key order.
pub fn read_cells(path: &str, bytes: &[u8]) -> Result<Vec<Chunk>, Problem> {
    let word = |at: usize| -> [u8; 4] {
        let mut w = [0; 4];
        w.copy_from_slice(&bytes[at..at + 4]);
        w
    };
    if bytes.len() < 12 || &bytes[..4] != MAGIC {
        return Err(refused(path, "it does not start with AMVX".into()));
    }
    let version = u32::from_le_bytes(word(4));
    if version != VERSION {
        return Err(refused(path, format!("version {version} is not {VERSION}")));
    }
    let count = usize::try_from(u32::from_le_bytes(word(8))).unwrap_or(usize::MAX);
    if count
        .checked_mul(CHUNK_BYTES)
        .and_then(|n| n.checked_add(12))
        != Some(bytes.len())
    {
        return Err(refused(
            path,
            format!("{} bytes do not hold {count} chunks", bytes.len()),
        ));
    }
    let mut chunks: Vec<Chunk> = Vec::with_capacity(count);
    for i in 0..count {
        let at = 12 + i * CHUNK_BYTES;
        let key = [0, 1, 2].map(|a| i32::from_le_bytes(word(at + 4 * a)));
        if chunks.last().is_some_and(|c| c.key >= key) {
            return Err(refused(path, format!("chunk {key:?} is out of key order")));
        }
        chunks.push(Chunk {
            key,
            cells: Cells(bytes[at + 12..at + CHUNK_BYTES].into()),
            edited: 0,
        });
    }
    Ok(chunks)
}

/// The bytes of a cells file holding `chunks` (in key order).
pub fn write_cells(chunks: &[Chunk]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + chunks.len() * CHUNK_BYTES);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    let count = u32::try_from(chunks.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&count.to_le_bytes());
    for c in chunks {
        for k in c.key {
            out.extend_from_slice(&k.to_le_bytes());
        }
        out.extend_from_slice(&c.cells.0);
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
            .map(|c| json!({"key": c.key, "cells": base64_encode(&c.cells.0)}))
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
        }
    }

    #[test]
    fn cells_files_round_trip_and_refuse_damage() {
        let chunks = vec![chunk([-1, 0, 3], 1), chunk([0, 0, 0], 2)];
        let bytes = write_cells(&chunks);
        assert_eq!(read_cells("w", &bytes).unwrap(), chunks);
        assert!(read_cells("w", &bytes[..bytes.len() - 1]).is_err());
        let mut bad = bytes.clone();
        bad[4] = 2;
        assert!(read_cells("w", &bad).is_err());
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
