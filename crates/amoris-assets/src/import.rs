//! glTF 2.0 import (charter 4.1): a `.gltf` or `.glb` file to a [`ModelAsset`]. Meshes are split
//! per primitive (one material each), positions and normals are taken as stored, missing normals
//! are computed flat, missing tangents are computed; each mesh gets its LOD chain and the GPU's
//! vertex order (lod.rs); images are decoded to RGBA8, and one whose sampler magnifies with
//! `NEAREST` is marked as pixel art. Runs on whatever thread calls it: the host imports on a
//! worker thread so a large file never freezes a frame.

// Asset import for the renderer: its floats never reach the world or its hash.
#![allow(clippy::disallowed_methods)]

use std::path::Path;

use amoris_contract::{Problem, detail};
use gltf::image::Format;
use serde_json::json;

use crate::mesh::{
    AlphaMode, AnimationClip, Channel, ChannelPath, ImageData, Interpolation, MaterialData,
    MeshData, ModelAsset, MorphChannel, MorphTargets, NodeData, SkeletonNode, SkinAsset,
    SkinWeights, Vertex,
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
    // Which images a texture magnifies with NEAREST (pixel art), and which a texture samples
    // smoothly: a filter belongs to a texture's sampler, and the renderer applies it per image.
    let mut nearest = vec![false; images.len()];
    let mut smooth = vec![false; images.len()];
    for t in doc.textures() {
        let by = if magnifies_nearest(&t) {
            &mut nearest
        } else {
            &mut smooth
        };
        by[t.source().index()] = true;
    }
    for (i, img) in images.iter().enumerate() {
        let mut image = to_rgba8(img, srgb[i], i);
        image.nearest = nearest[i] && !smooth[i];
        asset.images.push(image);
    }
    // An image sampled both ways is imported twice, the marked copy after the file's images: the
    // file's indices keep their meaning, and each copy gets a texture layer of its own.
    let mut marked: Vec<usize> = (0..images.len()).collect();
    for i in 0..images.len() {
        if nearest[i] && smooth[i] {
            marked[i] = asset.images.len();
            asset.images.push(ImageData {
                nearest: true,
                ..asset.images[i].clone()
            });
        }
    }
    // The image a material's texture takes: the one its own sampler asks for.
    let image = |t: gltf::Texture<'_>| {
        let i = t.source().index();
        if magnifies_nearest(&t) { marked[i] } else { i }
    };
    for m in doc.materials() {
        let pbr = m.pbr_metallic_roughness();
        let tex = |t: Option<gltf::texture::Info<'_>>| t.map(|t| image(t.texture()));
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
            normal_texture: m.normal_texture().map(|t| image(t.texture())),
            emissive_texture: tex(m.emissive_texture()),
            occlusion_texture: m.occlusion_texture().map(|t| image(t.texture())),
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
            uv_scale: pbr
                .base_color_texture()
                .and_then(|t| t.texture_transform())
                .map_or(1.0, |t| 0.5 * (t.scale()[0] + t.scale()[1])),
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
            let morph = read_targets(&mesh, &r, positions.len());
            // Flat normals duplicate vertices; skinned and morphed meshes keep their vertices (and
            // their weights and offsets).
            let skinned = joints.is_some() && weights.is_some();
            let (vertices, indices) = if normals.is_none() && !skinned && morph.is_none() {
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
            m.morph = morph;
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
            // Coarser levels and the GPU's vertex order (lod.rs, docs/spec/lod.md).
            crate::lod::build(&mut m, &crate::lod::LodOptions::default());
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
                weights: node_weights(&n),
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
            let mut morph = Vec::new();
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
                        gltf::animation::util::ReadOutputs::MorphTargetWeights(w) => {
                            morph.push(MorphChannel {
                                node: ch.target().node().index(),
                                interpolation: interpolation(ch.sampler().interpolation()),
                                times,
                                values: w.into_f32().collect(),
                            });
                            return None;
                        }
                    };
                    Some(Channel {
                        node: ch.target().node().index(),
                        path,
                        interpolation: interpolation(ch.sampler().interpolation()),
                        times,
                        values,
                    })
                })
                .collect();
            AnimationClip {
                name: a.name().map_or_else(|| format!("{i}"), str::to_owned),
                duration,
                channels,
                morph,
            }
        })
        .collect();
    asset
}

fn interpolation(i: gltf::animation::Interpolation) -> Interpolation {
    match i {
        gltf::animation::Interpolation::Step => Interpolation::Step,
        gltf::animation::Interpolation::Linear => Interpolation::Linear,
        gltf::animation::Interpolation::CubicSpline => Interpolation::Cubic,
    }
}

/// How many morph targets a glTF mesh has: its first primitive's (glTF requires every primitive to
/// have as many).
pub(crate) fn target_count(mesh: &gltf::Mesh<'_>) -> usize {
    mesh.primitives()
        .next()
        .map_or(0, |p| p.morph_targets().count())
}

/// The names of a glTF mesh's morph targets (animation.md 5a): its `extras.targetNames` when that
/// lists as many strings, else their indices.
pub(crate) fn target_names(mesh: &gltf::Mesh<'_>) -> Vec<String> {
    let count = target_count(mesh);
    let named = mesh.extras().as_ref().and_then(|raw| {
        let v: serde_json::Value = serde_json::from_str(raw.get()).ok()?;
        let list = v.get("targetNames")?.as_array()?;
        if list.len() != count {
            return None;
        }
        list.iter()
            .map(|x| x.as_str().map(str::to_owned))
            .collect::<Option<Vec<String>>>()
    });
    named.unwrap_or_else(|| (0..count).map(|i| i.to_string()).collect())
}

/// A node's default morph weights: the node's `weights`, else its mesh's, else zeros, one per
/// target of its mesh; empty for a node without a mesh with targets.
pub(crate) fn node_weights(node: &gltf::Node<'_>) -> Vec<f32> {
    let Some(mesh) = node.mesh() else {
        return Vec::new();
    };
    let n = target_count(&mesh);
    let given = node.weights().or_else(|| mesh.weights()).unwrap_or(&[]);
    (0..n)
        .map(|i| given.get(i).copied().unwrap_or(0.0))
        .collect()
}

/// A primitive's morph targets, `count` vertices each, or none.
fn read_targets<'a, 's, F>(
    mesh: &gltf::Mesh<'_>,
    r: &gltf::mesh::Reader<'a, 's, F>,
    count: usize,
) -> Option<MorphTargets>
where
    F: Clone + Fn(gltf::Buffer<'a>) -> Option<&'s [u8]>,
{
    const S: usize = MorphTargets::STRIDE;
    let names = target_names(mesh);
    let mut deltas = Vec::new();
    let mut targets = 0;
    for (p, n, t) in r.read_morph_targets() {
        let mut d = vec![0.0f32; count * S];
        let mut put = |at: usize, values: &mut dyn Iterator<Item = [f32; 3]>| {
            for (i, v) in values.take(count).enumerate() {
                d[i * S + at..i * S + at + 3].copy_from_slice(&v);
            }
        };
        if let Some(mut p) = p {
            put(0, &mut p);
        }
        if let Some(mut n) = n {
            put(3, &mut n);
        }
        if let Some(mut t) = t {
            put(6, &mut t);
        }
        deltas.extend_from_slice(&d);
        targets += 1;
    }
    (targets > 0 && targets == names.len()).then_some(MorphTargets { names, deltas })
}

/// Whether a texture's sampler magnifies with glTF's `NEAREST`: its image is pixel art whose
/// pixels keep their edges ([`ImageData::nearest`]). A texture with no sampler, a sampler with no
/// `magFilter` and `LINEAR` are smooth. `minFilter` and the wrap modes are not read.
fn magnifies_nearest(t: &gltf::Texture<'_>) -> bool {
    t.sampler().mag_filter() == Some(gltf::texture::MagFilter::Nearest)
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
                node: Some(node.index()),
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
        nearest: false,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    /// A PNG of `width` x `height` pixels, each `color(x, y)`.
    fn png(width: u32, height: u32, color: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(width, height, |x, y| image::Rgba(color(x, y)));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png)
            .expect("a PNG");
        out.into_inner()
    }

    /// A `.glb` in memory, as the browser imports one: `images` are PNGs held in the binary chunk
    /// (a slice import resolves no external file), under the given samplers, textures and
    /// materials.
    fn glb(images: &[Vec<u8>], samplers: Value, textures: Value, materials: Value) -> Vec<u8> {
        let mut bin = Vec::new();
        let mut views = Vec::new();
        for image in images {
            views.push(json!({"buffer": 0, "byteOffset": bin.len(), "byteLength": image.len()}));
            bin.extend_from_slice(image);
            bin.resize(bin.len().next_multiple_of(4), 0);
        }
        let mut text = json!({
            "asset": {"version": "2.0"},
            "buffers": [{"byteLength": bin.len()}],
            "bufferViews": views,
            "images": (0..images.len())
                .map(|i| json!({"bufferView": i, "mimeType": "image/png"}))
                .collect::<Vec<_>>(),
            "samplers": samplers,
            "textures": textures,
            "materials": materials,
        })
        .to_string()
        .into_bytes();
        text.resize(text.len().next_multiple_of(4), b' ');
        let mut out = Vec::new();
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&((12 + 8 + text.len() + 8 + bin.len()) as u32).to_le_bytes());
        out.extend_from_slice(&(text.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&text);
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&bin);
        out
    }

    /// Pixel art whose every pixel differs from the others.
    fn art(x: u32, y: u32) -> [u8; 4] {
        [x as u8 * 16, y as u8 * 16, 255 - (x + y) as u8 * 8, 255]
    }

    #[test]
    fn a_sampler_that_magnifies_with_nearest_marks_its_image() {
        // One image a material draws through a texture whose sampler is `sampler`.
        let import = |sampler: Option<Value>| {
            let texture = match &sampler {
                Some(_) => json!({"source": 0, "sampler": 1}),
                None => json!({"source": 0}),
            };
            let bytes = glb(
                &[png(16, 16, art)],
                json!([
                    {"magFilter": 9729},
                    sampler.unwrap_or_else(|| json!({}))
                ]),
                json!([texture]),
                json!([{"name": "block", "pbrMetallicRoughness": {
                    "baseColorTexture": {"index": 0}
                }}]),
            );
            import_glb_bytes(&bytes).expect("the model imports")
        };
        // blockcraft's sampler (gen_world.py): NEAREST up close, mipmapped in the distance.
        let pixel = import(Some(json!({"magFilter": 9728, "minFilter": 9986})));
        assert_eq!(pixel.images.len(), 1);
        let image = &pixel.images[0];
        assert!(image.nearest, "NEAREST marks the image");
        assert!(image.srgb, "a base color is color data");
        assert_eq!((image.width, image.height), (16, 16));
        let drawn: Vec<u8> = (0..16)
            .flat_map(|y| (0..16).flat_map(move |x| art(x, y)))
            .collect();
        assert_eq!(image.rgba8, drawn, "the pixels are the file's");
        assert_eq!(pixel.materials[0].base_color_texture, Some(0));
        // Smooth: LINEAR, a sampler that names no magnification filter, a NEAREST minification
        // alone, and a texture with no sampler.
        for sampler in [
            Some(json!({"magFilter": 9729, "minFilter": 9987})),
            Some(json!({})),
            Some(json!({"minFilter": 9728})),
            None,
        ] {
            let smooth = import(sampler.clone());
            assert_eq!(smooth.images.len(), 1, "{sampler:?}");
            assert!(!smooth.images[0].nearest, "{sampler:?} is smooth");
            assert_eq!(smooth.images[0].rgba8, drawn, "{sampler:?}");
            assert_eq!(smooth.materials[0].base_color_texture, Some(0));
        }
    }

    #[test]
    fn an_image_sampled_both_ways_is_imported_twice() {
        // Image 0 is sampled with NEAREST only, image 1 smoothly only (LINEAR, and no sampler),
        // image 2 both ways (NEAREST, and a sampler that names no magnification filter).
        let bytes = glb(
            &[
                png(16, 16, art),
                png(8, 4, |x, y| [x as u8, y as u8, 7, 255]),
                png(4, 8, |x, y| [200, x as u8 * 60, y as u8 * 30, 128]),
            ],
            json!([{"magFilter": 9728, "minFilter": 9986}, {"magFilter": 9729}, {}]),
            json!([
                {"source": 0, "sampler": 0},
                {"source": 1, "sampler": 1},
                {"source": 1},
                {"source": 2, "sampler": 0},
                {"source": 2, "sampler": 2}
            ]),
            json!([
                {"name": "pixel", "pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}},
                {"name": "smooth", "pbrMetallicRoughness": {"baseColorTexture": {"index": 1}},
                    "normalTexture": {"index": 2}},
                {"name": "pixel copy", "pbrMetallicRoughness": {
                    "baseColorTexture": {"index": 3}}},
                {"name": "mixed",
                    "pbrMetallicRoughness": {
                        "baseColorTexture": {"index": 4},
                        "metallicRoughnessTexture": {"index": 3}},
                    "normalTexture": {"index": 3},
                    "occlusionTexture": {"index": 4},
                    "emissiveTexture": {"index": 0}}
            ]),
        );
        let asset = import_glb_bytes(&bytes).expect("the model imports");
        // The file's three images keep their indices; image 2's marked copy follows them.
        let marks: Vec<bool> = asset.images.iter().map(|i| i.nearest).collect();
        assert_eq!(marks, [true, false, false, true]);
        let (smooth, pixel) = (&asset.images[2], &asset.images[3]);
        assert_eq!((pixel.width, pixel.height), (4, 8));
        assert_eq!(pixel.rgba8, smooth.rgba8, "the copy holds the same pixels");
        assert_eq!((pixel.srgb, &pixel.name), (smooth.srgb, &smooth.name));
        // Each slot takes the image its own texture's sampler asks for.
        let slots = |name: &str| {
            let m = asset
                .materials
                .iter()
                .find(|m| m.name == name)
                .expect("the material");
            [
                m.base_color_texture,
                m.metallic_roughness_texture,
                m.normal_texture,
                m.occlusion_texture,
                m.emissive_texture,
            ]
        };
        assert_eq!(slots("pixel"), [Some(0), None, None, None, None]);
        assert_eq!(slots("smooth"), [Some(1), None, Some(1), None, None]);
        assert_eq!(slots("pixel copy"), [Some(3), None, None, None, None]);
        assert_eq!(
            slots("mixed"),
            [Some(2), Some(3), Some(3), Some(2), Some(0)]
        );
    }

    #[test]
    fn blockcraft_ships_its_pixel_art_as_drawn() {
        // samples/blockcraft writes its 16 x 16 art as drawn (textures.py), and its files' one
        // sampler magnifies with NEAREST (gen_world.py): every image arrives marked, at its size.
        let blockcraft = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/blockcraft");
        let blocks = import_gltf(&blockcraft.join("blocks.gltf")).expect("blocks.gltf imports");
        let cloud = import_gltf(&blockcraft.join("clouds/cloud0.gltf")).expect("a cloud imports");
        assert_eq!((blocks.images.len(), blocks.materials.len()), (24, 24));
        assert_eq!(cloud.images.len(), 1);
        for (i, image) in blocks.images.iter().chain(&cloud.images).enumerate() {
            assert_eq!((image.width, image.height), (16, 16), "image {i}");
            assert!(image.nearest, "image {i} is marked");
            assert_eq!(image.rgba8.len(), 16 * 16 * 4, "image {i}");
        }
        for (i, m) in blocks.materials.iter().enumerate() {
            assert_eq!(m.base_color_texture, Some(i), "{}", m.name);
        }
    }
}
