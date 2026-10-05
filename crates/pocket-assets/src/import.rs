//! glTF 2.0 import (charter 4.1): a `.gltf` or `.glb` file to a [`ModelAsset`]. Meshes are split
//! per primitive (one material each), positions and normals are taken as stored, missing normals
//! are computed flat, missing tangents are computed; images are decoded to RGBA8. Runs on whatever
//! thread calls it: the host imports on a worker thread so a large file never freezes a frame.

// Asset import for the renderer: its floats never reach the world or its hash.
#![allow(clippy::disallowed_methods)]

use std::path::Path;

use gltf::image::Format;
use pocket_contract::{Problem, detail};
use serde_json::json;

use crate::mesh::{
    AlphaMode, AnimationClip, Channel, ChannelPath, ImageData, Interpolation, MaterialData,
    MeshData, ModelAsset, NodeData, SkeletonNode, SkinAsset, SkinWeights, Vertex,
};

fn failed(path: &Path, why: impl std::fmt::Display) -> Problem {
    Problem::new(
        "asset.import_failed",
        format!("{} does not import: {why}.", path.display()),
        detail([("path", json!(path.display().to_string()))]),
    )
}

/// Imports the file at `path`.
pub fn import_gltf(path: &Path) -> Result<ModelAsset, Problem> {
    let (doc, buffers, images) = gltf::import(path).map_err(|e| failed(path, e))?;
    Ok(convert(&doc, &buffers, &images))
}

/// Imports a `.glb` held in memory (the browser fetches it); external files are not resolved.
pub fn import_glb_bytes(bytes: &[u8]) -> Result<ModelAsset, Problem> {
    let (doc, buffers, images) =
        gltf::import_slice(bytes).map_err(|e| failed(Path::new("<memory>"), e))?;
    Ok(convert(&doc, &buffers, &images))
}

fn convert(
    doc: &gltf::Document,
    buffers: &[gltf::buffer::Data],
    images: &[gltf::image::Data],
) -> ModelAsset {
    let mut asset = ModelAsset::default();
    // Which images are color data: base color and emissive are sRGB; the rest are linear.
    let mut srgb = vec![false; images.len()];
    for m in doc.materials() {
        let pbr = m.pbr_metallic_roughness();
        if let Some(t) = pbr.base_color_texture() {
            srgb[t.texture().source().index()] = true;
        }
        if let Some(t) = m.emissive_texture() {
            srgb[t.texture().source().index()] = true;
        }
    }
    for (i, img) in images.iter().enumerate() {
        asset.images.push(to_rgba8(img, srgb[i], i));
    }
    for m in doc.materials() {
        let pbr = m.pbr_metallic_roughness();
        let tex = |t: Option<gltf::texture::Info<'_>>| t.map(|t| t.texture().source().index());
        let strength = m.emissive_strength().unwrap_or(1.0);
        let e = m.emissive_factor();
        asset.materials.push(MaterialData {
            name: m.name().unwrap_or("").to_owned(),
            base_color: pbr.base_color_factor(),
            metallic: pbr.metallic_factor(),
            roughness: pbr.roughness_factor(),
            transmission: m.transmission().map_or(0.0, |t| t.transmission_factor()),
            ior: m.ior().unwrap_or(1.5),
            emissive: [e[0] * strength, e[1] * strength, e[2] * strength],
            base_color_texture: tex(pbr.base_color_texture()),
            metallic_roughness_texture: tex(pbr.metallic_roughness_texture()),
            normal_texture: m.normal_texture().map(|t| t.texture().source().index()),
            emissive_texture: tex(m.emissive_texture()),
            occlusion_texture: m.occlusion_texture().map(|t| t.texture().source().index()),
            transmission_texture: m.transmission().and_then(|t| tex(t.transmission_texture())),
            normal_scale: m.normal_texture().map_or(1.0, |t| t.scale()),
            occlusion_strength: m.occlusion_texture().map_or(1.0, |t| t.strength()),
            alpha_mode: match m.alpha_mode() {
                gltf::material::AlphaMode::Opaque => AlphaMode::Opaque,
                gltf::material::AlphaMode::Mask => AlphaMode::Mask,
                gltf::material::AlphaMode::Blend => AlphaMode::Blend,
            },
            alpha_cutoff: m.alpha_cutoff().unwrap_or(0.5),
            double_sided: m.double_sided(),
        });
    }
    // glTF mesh index -> our mesh indices (one per primitive).
    let mut mesh_parts: Vec<Vec<usize>> = Vec::new();
    for mesh in doc.meshes() {
        let mut parts = Vec::new();
        for (pi, prim) in mesh.primitives().enumerate() {
            if prim.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let r = prim.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
            let Some(positions) = r.read_positions() else {
                continue;
            };
            let positions: Vec<[f32; 3]> = positions.collect();
            let normals: Option<Vec<[f32; 3]>> = r.read_normals().map(Iterator::collect);
            let uvs: Option<Vec<[f32; 2]>> = r.read_tex_coords(0).map(|t| t.into_f32().collect());
            let tangents: Option<Vec<[f32; 4]>> = r.read_tangents().map(Iterator::collect);
            let joints: Option<Vec<[u16; 4]>> = r.read_joints(0).map(|j| j.into_u16().collect());
            let weights: Option<Vec<[f32; 4]>> = r.read_weights(0).map(|w| w.into_f32().collect());
            let indices: Vec<u32> = match r.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => (0..positions.len() as u32).collect(),
            };
            let mut vertices: Vec<Vertex> = positions
                .iter()
                .enumerate()
                .map(|(i, p)| Vertex {
                    position: *p,
                    normal: normals.as_ref().map_or([0.0, 1.0, 0.0], |n| n[i]),
                    uv: uvs.as_ref().map_or([0.0, 0.0], |u| u[i]),
                    tangent: tangents.as_ref().map_or([1.0, 0.0, 0.0, 1.0], |t| t[i]),
                })
                .collect();
            // Flat normals duplicate vertices; skinned meshes keep their vertices (and weights).
            let skinned = joints.is_some() && weights.is_some();
            let (vertices, indices) = if normals.is_none() && !skinned {
                flat_normals(&vertices, &indices)
            } else {
                (std::mem::take(&mut vertices), indices)
            };
            let name = match (mesh.name(), mesh.primitives().len()) {
                (Some(n), 1) => n.to_owned(),
                (Some(n), _) => format!("{n}.{pi}"),
                (None, _) => format!("mesh{}.{pi}", mesh.index()),
            };
            let mut m = MeshData::new(&name, vertices, indices);
            m.material = prim.material().index();
            if let (Some(j), Some(w)) = (joints, weights)
                && j.len() == m.vertices.len()
                && w.len() == m.vertices.len()
            {
                m.skin = Some(SkinWeights {
                    joints: j,
                    weights: w,
                });
            }
            if tangents.is_none() {
                m.compute_tangents();
            }
            parts.push(asset.meshes.len());
            asset.meshes.push(m);
        }
        mesh_parts.push(parts);
    }
    let scene = doc.default_scene().or_else(|| doc.scenes().next());
    if let Some(scene) = scene {
        for node in scene.nodes() {
            walk(&node, IDENTITY, &mesh_parts, &mut asset.nodes);
        }
    }
    // The whole hierarchy (bones are nodes), the skins and the clips.
    let mut parent = vec![None; doc.nodes().len()];
    for n in doc.nodes() {
        for c in n.children() {
            parent[c.index()] = Some(n.index());
        }
    }
    asset.skeleton = doc
        .nodes()
        .map(|n| {
            let (t, r, s) = n.transform().decomposed();
            SkeletonNode {
                name: n.name().unwrap_or("").to_owned(),
                parent: parent[n.index()],
                translation: t,
                rotation: r,
                scale: s,
            }
        })
        .collect();
    asset.skins = doc
        .skins()
        .map(|sk| {
            let r = sk.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
            let joints: Vec<usize> = sk.joints().map(|j| j.index()).collect();
            let inverse_bind = r
                .read_inverse_bind_matrices()
                .map(|m| m.map(bytemuck_flatten).collect())
                .unwrap_or_else(|| vec![IDENTITY; joints.len()]);
            SkinAsset {
                joints,
                inverse_bind,
            }
        })
        .collect();
    asset.animations = doc
        .animations()
        .enumerate()
        .map(|(i, a)| {
            let mut duration = 0.0f32;
            let channels = a
                .channels()
                .filter_map(|ch| {
                    let r = ch.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));
                    let times: Vec<f32> = r.read_inputs()?.collect();
                    duration = duration.max(times.last().copied().unwrap_or(0.0));
                    let (path, values): (ChannelPath, Vec<[f32; 4]>) = match r.read_outputs()? {
                        gltf::animation::util::ReadOutputs::Translations(v) => (
                            ChannelPath::Translation,
                            v.map(|x| [x[0], x[1], x[2], 0.0]).collect(),
                        ),
                        gltf::animation::util::ReadOutputs::Scales(v) => (
                            ChannelPath::Scale,
                            v.map(|x| [x[0], x[1], x[2], 0.0]).collect(),
                        ),
                        gltf::animation::util::ReadOutputs::Rotations(v) => {
                            (ChannelPath::Rotation, v.into_f32().collect())
                        }
                        gltf::animation::util::ReadOutputs::MorphTargetWeights(_) => return None,
                    };
                    Some(Channel {
                        node: ch.target().node().index(),
                        path,
                        interpolation: match ch.sampler().interpolation() {
                            gltf::animation::Interpolation::Step => Interpolation::Step,
                            gltf::animation::Interpolation::Linear => Interpolation::Linear,
                            gltf::animation::Interpolation::CubicSpline => Interpolation::Cubic,
                        },
                        times,
                        values,
                    })
                })
                .collect();
            AnimationClip {
                name: a.name().map_or_else(|| format!("{i}"), str::to_owned),
                duration,
                channels,
            }
        })
        .collect();
    asset
}

const IDENTITY: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

fn mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut out = [0.0; 16];
    for c in 0..4 {
        for r in 0..4 {
            out[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum();
        }
    }
    out
}

fn walk(node: &gltf::Node<'_>, parent: [f32; 16], parts: &[Vec<usize>], out: &mut Vec<NodeData>) {
    let local: [f32; 16] = bytemuck_flatten(node.transform().matrix());
    let world = mul(&parent, &local);
    if let Some(mesh) = node.mesh() {
        for &m in &parts[mesh.index()] {
            out.push(NodeData {
                name: node.name().unwrap_or("").to_owned(),
                mesh: m,
                transform: world,
                skin: node.skin().map(|s| s.index()),
            });
        }
    }
    for child in node.children() {
        walk(&child, world, parts, out);
    }
}

fn bytemuck_flatten(m: [[f32; 4]; 4]) -> [f32; 16] {
    let mut out = [0.0; 16];
    for (c, col) in m.iter().enumerate() {
        out[c * 4..c * 4 + 4].copy_from_slice(col);
    }
    out
}

fn flat_normals(vs: &[Vertex], is: &[u32]) -> (Vec<Vertex>, Vec<u32>) {
    let mut out = Vec::with_capacity(is.len());
    for t in is.as_chunks::<3>().0 {
        let [a, b, c] = t.map(|i| vs[i as usize]);
        let e1 = [
            b.position[0] - a.position[0],
            b.position[1] - a.position[1],
            b.position[2] - a.position[2],
        ];
        let e2 = [
            c.position[0] - a.position[0],
            c.position[1] - a.position[1],
            c.position[2] - a.position[2],
        ];
        let n = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-12);
        let n = [n[0] / l, n[1] / l, n[2] / l];
        for mut v in [a, b, c] {
            v.normal = n;
            out.push(v);
        }
    }
    let idx = (0..out.len() as u32).collect();
    (out, idx)
}

fn to_rgba8(img: &gltf::image::Data, srgb: bool, index: usize) -> ImageData {
    let (w, h) = (img.width, img.height);
    let px = &img.pixels;
    let n = (w * h) as usize;
    let rgba8 = match img.format {
        Format::R8G8B8A8 => px.clone(),
        Format::R8G8B8 => {
            let mut o = Vec::with_capacity(n * 4);
            for c in px.as_chunks::<3>().0 {
                o.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
            o
        }
        Format::R8G8 => {
            let mut o = Vec::with_capacity(n * 4);
            for c in px.as_chunks::<2>().0 {
                o.extend_from_slice(&[c[0], c[1], 0, 255]);
            }
            o
        }
        Format::R8 => {
            let mut o = Vec::with_capacity(n * 4);
            for &c in px {
                o.extend_from_slice(&[c, c, c, 255]);
            }
            o
        }
        Format::R16G16B16A16 | Format::R16G16B16 | Format::R16G16 | Format::R16 => {
            let ch = match img.format {
                Format::R16G16B16A16 => 4,
                Format::R16G16B16 => 3,
                Format::R16G16 => 2,
                _ => 1,
            };
            let mut o = Vec::with_capacity(n * 4);
            for c in px.chunks_exact(ch * 2) {
                let at = |k: usize| {
                    if k < ch {
                        c[k * 2 + 1]
                    } else if k == 3 {
                        255
                    } else {
                        0
                    }
                };
                o.extend_from_slice(&[at(0), at(1), at(2), at(3)]);
            }
            o
        }
        _ => vec![255; n * 4],
    };
    ImageData {
        name: format!("image{index}"),
        width: w,
        height: h,
        rgba8,
        srgb,
    }
}
