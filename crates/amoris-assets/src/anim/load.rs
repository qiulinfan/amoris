//! Skeletons and clips from a glTF file (`.glb` or `.gltf` with its buffers), without decoding its
//! images: what the host reads for a project's animation when it loads the project (animation.md
//! 2). Cubic channels keep their values at the keys (their tangents are dropped).

// Import, not a tick: what it reads enters the world only as the library's data.
#![allow(clippy::disallowed_methods)]

use std::path::Path;

use amoris_contract::{Problem, detail};
use serde_json::json;

use super::clip::{Clip, ClipSet, Joint, Keys, Skeleton, Track};
use super::math::Xform;

fn failed(path: &Path, why: impl std::fmt::Display) -> Problem {
    Problem::new(
        "asset.import_failed",
        format!("{} does not import: {why}.", path.display()),
        detail([("path", json!(path.display().to_string()))]),
    )
}

/// The skeleton (every node, in the file's order) and the clips of a glTF file.
pub fn load(path: &Path) -> Result<ClipSet, Problem> {
    let gltf::Gltf { document, blob } = gltf::Gltf::open(path).map_err(|e| failed(path, e))?;
    let buffers =
        gltf::import_buffers(&document, path.parent(), blob).map_err(|e| failed(path, e))?;
    Ok(convert(&document, &buffers))
}

fn convert(doc: &gltf::Document, buffers: &[gltf::buffer::Data]) -> ClipSet {
    let mut parent = vec![None; doc.nodes().len()];
    for n in doc.nodes() {
        for c in n.children() {
            parent[c.index()] = Some(n.index() as u32);
        }
    }
    let joints: Vec<Joint> = doc
        .nodes()
        .map(|n| {
            let (t, r, s) = n.transform().decomposed();
            Joint {
                name: n
                    .name()
                    .map_or_else(|| format!("node{}", n.index()), str::to_owned),
                parent: parent[n.index()],
                rest: Xform { t, r, s },
            }
        })
        .collect();
    let names: Vec<String> = joints.iter().map(|j| j.name.clone()).collect();
    let clips = doc
        .animations()
        .enumerate()
        .map(|(i, a)| {
            let mut tracks: Vec<Track> = Vec::new();
            let mut duration = 0.0f32;
            for ch in a.channels() {
                let r = ch.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
                let Some(times) = r.read_inputs().map(|t| t.collect::<Vec<f32>>()) else {
                    continue;
                };
                duration = duration.max(times.last().copied().unwrap_or(0.0));
                let cubic =
                    ch.sampler().interpolation() == gltf::animation::Interpolation::CubicSpline;
                let step = ch.sampler().interpolation() == gltf::animation::Interpolation::Step;
                // A cubic channel stores (in-tangent, value, out-tangent) per key: keep the values.
                let pick = |n: usize, width: usize, all: Vec<f32>| -> Vec<f32> {
                    if !cubic {
                        return all;
                    }
                    (0..n)
                        .flat_map(|k| all[(3 * k + 1) * width..(3 * k + 2) * width].to_vec())
                        .collect()
                };
                let n = times.len();
                let node = ch.target().node().index();
                let track = match tracks.iter().position(|t| t.joint == names[node]) {
                    Some(i) => i,
                    None => {
                        tracks.push(Track {
                            joint: names[node].clone(),
                            t: Keys::default(),
                            r: Keys::default(),
                            s: Keys::default(),
                        });
                        tracks.len() - 1
                    }
                };
                let keys = |values: Vec<f32>| Keys {
                    times: times.clone(),
                    values,
                    step,
                };
                match r.read_outputs() {
                    Some(gltf::animation::util::ReadOutputs::Translations(v)) => {
                        tracks[track].t = keys(pick(n, 3, v.flatten().collect()));
                    }
                    Some(gltf::animation::util::ReadOutputs::Scales(v)) => {
                        tracks[track].s = keys(pick(n, 3, v.flatten().collect()));
                    }
                    Some(gltf::animation::util::ReadOutputs::Rotations(v)) => {
                        tracks[track].r = keys(pick(n, 4, v.into_f32().flatten().collect()));
                    }
                    _ => {}
                }
            }
            Clip {
                name: a.name().map_or_else(|| format!("{i}"), str::to_owned),
                duration,
                tracks,
            }
        })
        .collect();
    ClipSet {
        skeleton: Skeleton::new(joints),
        clips,
    }
}
