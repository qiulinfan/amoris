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
    let gltf::Gltf { document, blob } = gltf::Gltf::open(path).map_err(|e| failed(path, e))?;
    let base = path.parent().unwrap_or_else(|| Path::new("./"));
    let buffers = gltf::import_buffers(&document, Some(base), blob).map_err(|e| failed(path, e))?;
    convert(&document, &buffers, Some(base)).map_err(|e| failed(path, e))
}

/// A PNG or JPEG file's pixels as RGBA8 (sRGB), with its width and height: a game UI image
/// (docs/spec/ui.md 2).
pub fn decode_image(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let img = image::load_from_memory(bytes)
        .map_err(|e| e.to_string())?
        .to_rgba8();
    Ok((img.width(), img.height(), img.into_raw()))
}

/// Imports a `.glb` held in memory (the browser fetches it); external files are not resolved.
pub fn import_glb_bytes(bytes: &[u8]) -> Result<ModelAsset, Problem> {
    let memory = Path::new("<memory>");
    let gltf::Gltf { document, blob } =
        gltf::Gltf::from_slice(bytes).map_err(|e| failed(memory, e))?;
    let buffers = gltf::import_buffers(&document, None, blob).map_err(|e| failed(memory, e))?;
    convert(&document, &buffers, None).map_err(|e| failed(memory, e))
}

/// The image a texture names in `KHR_texture_basisu` (a KTX2 file), if any.
fn basisu_source(t: &gltf::Texture<'_>) -> Option<usize> {
    t.extension_value("KHR_texture_basisu")?["source"]
        .as_u64()
        .map(|i| i as usize)
}

/// The images a texture names: its `KHR_texture_basisu` source, then its own `source`.
fn named_images(t: &gltf::Texture<'_>) -> impl Iterator<Item = usize> {
    basisu_source(t)
        .into_iter()
        .chain(t.source().map(|s| s.index()))
}

/// Whether an image is a KTX2 file, by its media type, its file name or its first bytes.
fn is_ktx2_image(image: &gltf::Image<'_>, buffers: &[gltf::buffer::Data]) -> bool {
    match image.source() {
        gltf::image::Source::View { view, mime_type } => {
            mime_type == "image/ktx2"
                || buffers.get(view.buffer().index()).is_some_and(|b| {
                    b.0.get(view.offset()..view.offset() + view.length())
                        .is_some_and(crate::ktx2::is_ktx2)
                })
        }
        gltf::image::Source::Uri { uri, mime_type } => {
            mime_type == Some("image/ktx2")
                || uri.starts_with("data:image/ktx2")
                || uri.to_ascii_lowercase().ends_with(".ktx2")
        }
    }
}

/// A KTX2 image's bytes: a buffer view, a base64 data URI, or a file beside the glTF file.
fn ktx2_bytes(
    image: &gltf::Image<'_>,
    buffers: &[gltf::buffer::Data],
    base: Option<&Path>,
) -> Result<Vec<u8>, String> {
    match image.source() {
        gltf::image::Source::View { view, .. } => buffers
            .get(view.buffer().index())
            .and_then(|b| b.0.get(view.offset()..view.offset() + view.length()))
            .map(<[u8]>::to_vec)
            .ok_or_else(|| "its buffer view lies outside its buffer".into()),
        gltf::image::Source::Uri { uri, .. } => {
            if let Some(rest) = uri.strip_prefix("data:") {
                let (_, data) = rest
                    .split_once(";base64,")
                    .ok_or("a data URI that is not base64")?;
                return unbase64(data).ok_or_else(|| "a data URI that does not decode".into());
            }
            let base = base.ok_or("an external file, which an import from memory does not read")?;
            let file = base.join(percent_decode(uri));
            std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))
        }
    }
}

/// A URI's `%XX` escapes decoded (glTF's URIs are percent-encoded).
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    let hex = |c: u8| (c as char).to_digit(16);
    while i < b.len() {
        if b[i] == b'%'
            && let (Some(h), Some(l)) = (
                b.get(i + 1).copied().and_then(hex),
                b.get(i + 2).copied().and_then(hex),
            )
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Standard or URL-safe base64, padding optional.
fn unbase64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=') {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        };
        acc = ((acc << 6) | u32::from(v)) & 0xFFFF;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// The file's images as imported (`None`: not decoded), and the image each texture takes
/// (docs/spec/texture-formats.md 3). A texture with a `KHR_texture_basisu` source takes that KTX2
/// image when it reads, else its own `source` (the extension's fallback), else none; a KTX2 image
/// that does not read fails the import unless every texture naming it has a fallback that reads.
/// An image that only fallbacks of KTX2 images that read name is not decoded. Every other image
/// decodes as before: PNG and JPEG through the `image` crate, KTX2 by [`crate::ktx2`].
#[allow(clippy::type_complexity)]
fn load_images(
    doc: &gltf::Document,
    buffers: &[gltf::buffer::Data],
    base: Option<&Path>,
) -> Result<(Vec<Option<ImageData>>, Vec<Option<usize>>), String> {
    let count = doc.images().count();
    let ktx2: Vec<bool> = doc.images().map(|i| is_ktx2_image(&i, buffers)).collect();
    // The KTX2 images textures name, each read once.
    let mut wanted = vec![false; count];
    for t in doc.textures() {
        for i in named_images(&t).filter(|&i| i < count && ktx2[i]) {
            wanted[i] = true;
        }
    }
    let mut read: Vec<Option<Result<ImageData, String>>> = vec![None; count];
    for image in doc.images() {
        let i = image.index();
        if wanted[i] {
            read[i] = Some(
                ktx2_bytes(&image, buffers, base)
                    .and_then(|bytes| crate::ktx2::image_data(format!("image{i}"), &bytes, false)),
            );
        }
    }
    let reads = |i: usize| i < count && (!ktx2[i] || matches!(read[i], Some(Ok(_))));
    let why = |t: &gltf::Texture<'_>, i: usize| {
        let reason = match read.get(i) {
            Some(Some(Err(e))) => e.clone(),
            _ => "no such image".into(),
        };
        format!(
            "texture {}'s KTX2 image {i} does not read: {reason}",
            t.index()
        )
    };
    let mut chosen = Vec::new();
    for t in doc.textures() {
        let fallback = t.source().map(|s| s.index());
        let pick = match (basisu_source(&t), fallback) {
            (Some(k), _) if reads(k) && ktx2[k] => Some(k),
            (Some(_), Some(f)) if reads(f) => Some(f),
            (Some(k), _) => return Err(why(&t, k)),
            (None, Some(f)) if reads(f) => Some(f),
            (None, Some(f)) => return Err(why(&t, f)),
            (None, None) => None,
        };
        chosen.push(pick);
    }
    // An image a texture names but none takes is a fallback left unused: not decoded.
    let mut used = vec![false; count];
    let mut named = vec![false; count];
    for (t, pick) in doc.textures().zip(&chosen) {
        if let Some(i) = *pick {
            used[i] = true;
        }
        for i in named_images(&t).filter(|&i| i < count) {
            named[i] = true;
        }
    }
    let mut images = Vec::with_capacity(count);
    for image in doc.images() {
        let i = image.index();
        images.push(if ktx2[i] {
            match read[i].take() {
                Some(Ok(data)) if used[i] => Some(data),
                _ => None,
            }
        } else if named[i] && !used[i] {
            None
        } else {
            let data = gltf::image::Data::from_source(image.source(), base, buffers)
                .map_err(|e| format!("image {i}: {e}"))?;
            Some(to_rgba8(&data, false, i))
        });
    }
    Ok((images, chosen))
}

fn convert(
    doc: &gltf::Document,
    buffers: &[gltf::buffer::Data],
    base: Option<&Path>,
) -> Result<ModelAsset, String> {
    let mut asset = ModelAsset::default();
    let (loaded, chosen) = load_images(doc, buffers, base)?;
    let images = loaded.len();
    let source = |t: &gltf::Texture<'_>| chosen[t.index()];
    // Which images are color data: base color and emissive are sRGB; the rest are linear.
    let mut srgb = vec![false; images];
    for m in doc.materials() {
        let pbr = m.pbr_metallic_roughness();
        if let Some(i) = pbr.base_color_texture().and_then(|t| source(&t.texture())) {
            srgb[i] = true;
        }
        if let Some(i) = m.emissive_texture().and_then(|t| source(&t.texture())) {
            srgb[i] = true;
        }
    }
    // Which images a texture magnifies with NEAREST (pixel art), and which a texture samples
    // smoothly: a filter belongs to a texture's sampler, and the renderer applies it per image.
    let mut nearest = vec![false; images];
    let mut smooth = vec![false; images];
    for t in doc.textures() {
        let by = if magnifies_nearest(&t) {
            &mut nearest
        } else {
            &mut smooth
        };
        if let Some(i) = source(&t) {
            by[i] = true;
        }
    }
    for (i, img) in loaded.into_iter().enumerate() {
        let mut image = img.unwrap_or_else(|| ImageData {
            name: format!("image{i}"),
            ..ImageData::default()
        });
        image.srgb = srgb[i];
        image.nearest = nearest[i] && !smooth[i];
        asset.images.push(image);
    }
    // An image sampled both ways is imported twice, the marked copy after the file's images: the
    // file's indices keep their meaning, and each copy gets a texture layer of its own.
    let mut marked: Vec<usize> = (0..images).collect();
    for i in 0..images {
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
        let i = source(&t)?;
        Some(if magnifies_nearest(&t) { marked[i] } else { i })
    };
    for m in doc.materials() {
        let pbr = m.pbr_metallic_roughness();
        let tex = |t: Option<gltf::texture::Info<'_>>| t.and_then(|t| image(t.texture()));
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
            normal_texture: m.normal_texture().and_then(|t| image(t.texture())),
            emissive_texture: tex(m.emissive_texture()),
            occlusion_texture: m.occlusion_texture().and_then(|t| image(t.texture())),
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
    Ok(asset)
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
        levels: None,
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
        let mime = |i: &Vec<u8>| {
            if crate::ktx2::is_ktx2(i) {
                "image/ktx2"
            } else {
                "image/png"
            }
        };
        let mut doc = json!({
            "asset": {"version": "2.0"},
            "buffers": [{"byteLength": bin.len()}],
            "bufferViews": views,
            "images": images
                .iter()
                .enumerate()
                .map(|(i, image)| json!({"bufferView": i, "mimeType": mime(image)}))
                .collect::<Vec<_>>(),
            "samplers": samplers,
            "textures": textures,
            "materials": materials,
        });
        if images.iter().any(|i| crate::ktx2::is_ktx2(i)) {
            doc["extensionsUsed"] = json!(["KHR_texture_basisu"]);
            doc["extensionsRequired"] = json!(["KHR_texture_basisu"]);
        }
        let mut text = doc.to_string().into_bytes();
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

    /// An RGBA8 KTX2 file of `width` x `height` pixels, each `color(x, y)`, with its full chain,
    /// Zstd-supercompressed.
    fn ktx2_file(width: u32, height: u32, color: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
        let px: Vec<u8> = (0..height)
            .flat_map(|y| (0..width).flat_map(move |x| (0..4).map(move |c| (x, y, c))))
            .map(|(x, y, c)| color(x, y)[c])
            .collect();
        let levels = crate::mesh::TextureLevels {
            format: crate::mesh::BlockFormat::Rgba8,
            levels: crate::ktx2::box_chain(width, height, &px),
        };
        crate::ktx2::write(width, height, true, &levels, true)
    }

    /// `file` with its `vkFormat` undefined: what a Basis Universal file says.
    fn as_basis(mut file: Vec<u8>) -> Vec<u8> {
        file[12..16].copy_from_slice(&0u32.to_le_bytes());
        file
    }

    #[test]
    fn a_basisu_texture_takes_its_ktx2_image() {
        let fallback = png(8, 8, |_, _| [9, 9, 9, 255]);
        let color = ktx2_file(8, 8, art);
        let normal = ktx2_file(4, 4, |_, _| [128, 128, 255, 255]);
        let bytes = glb(
            &[fallback, color, normal],
            json!([]),
            json!([
                {"source": 0, "extensions": {"KHR_texture_basisu": {"source": 1}}},
                {"extensions": {"KHR_texture_basisu": {"source": 2}}}
            ]),
            json!([{"name": "panel",
                "pbrMetallicRoughness": {"baseColorTexture": {"index": 0}},
                "normalTexture": {"index": 1}}]),
        );
        let asset = import_glb_bytes(&bytes).expect("the model imports");
        let m = &asset.materials[0];
        assert_eq!((m.base_color_texture, m.normal_texture), (Some(1), Some(2)));
        let (fallback, color, normal) = (&asset.images[0], &asset.images[1], &asset.images[2]);
        assert_eq!(
            (fallback.width, fallback.rgba8.len()),
            (0, 0),
            "the unused PNG is not decoded"
        );
        assert!(
            color.srgb && !normal.srgb,
            "glTF's use decides the color space"
        );
        let drawn: Vec<u8> = (0..8)
            .flat_map(|y| (0..8).flat_map(move |x| art(x, y)))
            .collect();
        assert_eq!(color.rgba8, drawn, "level 0, decoded");
        let levels = color.levels.as_ref().expect("the file's levels");
        assert_eq!(levels.format, crate::mesh::BlockFormat::Rgba8);
        assert_eq!(levels.levels.len(), 4, "8, 4, 2, 1");
        assert_eq!(levels.levels[0], drawn);
        assert_eq!(&normal.rgba8[..4], &[128, 128, 255, 255]);
        // A plain image/ktx2 source reads the same way.
        let plain = glb(
            &[ktx2_file(8, 8, art)],
            json!([]),
            json!([{"source": 0}]),
            json!([{"pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}}]),
        );
        let asset = import_glb_bytes(&plain).expect("a plain KTX2 source imports");
        assert_eq!(asset.images[0].rgba8, drawn);
        assert!(asset.images[0].levels.is_some());
    }

    #[test]
    fn a_ktx2_image_that_does_not_read_falls_back_or_fails_the_import() {
        let fallback = png(4, 4, |x, y| [x as u8 * 50, y as u8 * 50, 3, 255]);
        let basis = as_basis(ktx2_file(4, 4, art));
        let material = json!([{"pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}}]);
        let with = glb(
            &[fallback.clone(), basis.clone()],
            json!([]),
            json!([{"source": 0, "extensions": {"KHR_texture_basisu": {"source": 1}}}]),
            material.clone(),
        );
        let asset = import_glb_bytes(&with).expect("the fallback imports");
        assert_eq!(asset.materials[0].base_color_texture, Some(0));
        assert_eq!(
            (asset.images[0].width, asset.images[0].levels.is_none()),
            (4, true)
        );
        assert_eq!(asset.images[1].width, 0, "the Basis image is left out");
        let without = glb(
            &[basis],
            json!([]),
            json!([{"extensions": {"KHR_texture_basisu": {"source": 0}}}]),
            material,
        );
        let refused = import_glb_bytes(&without).expect_err("no fallback");
        assert!(refused.message.contains("Basis"), "{}", refused.message);
    }

    #[test]
    fn a_gltf_file_reads_a_ktx2_file_beside_it() {
        let dir = std::env::temp_dir().join(format!("amoris-ktx2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a b.ktx2"), ktx2_file(4, 4, art)).unwrap();
        let doc = json!({
            "asset": {"version": "2.0"},
            "extensionsUsed": ["KHR_texture_basisu"],
            "extensionsRequired": ["KHR_texture_basisu"],
            "images": [{"uri": "a%20b.ktx2"}],
            "textures": [{"extensions": {"KHR_texture_basisu": {"source": 0}}}],
            "materials": [{"pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}}],
        });
        std::fs::write(dir.join("m.gltf"), doc.to_string()).unwrap();
        let asset = import_gltf(&dir.join("m.gltf"));
        let _ = std::fs::remove_dir_all(&dir);
        let asset = asset.expect("the model imports");
        assert_eq!(asset.materials[0].base_color_texture, Some(0));
        let drawn: Vec<u8> = (0..4)
            .flat_map(|y| (0..4).flat_map(move |x| art(x, y)))
            .collect();
        assert_eq!(asset.images[0].rgba8, drawn);
    }
}
