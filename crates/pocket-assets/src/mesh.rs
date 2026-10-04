//! CPU mesh data (charter 4.4): what an importer or a primitive generator produces and the renderer
//! uploads. One vertex layout for every mesh, so every mesh can live in one GPU buffer and be drawn
//! by the same indirect draws.

use serde::{Deserialize, Serialize};

/// One vertex: position, normal, texture coordinate, tangent (`w` is the bitangent's sign).
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    bytemuck::Pod,
    bytemuck::Zeroable,
)]
#[repr(C)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub tangent: [f32; 4],
}

/// A bounding sphere and box in the mesh's own space.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub center: [f32; 3],
    pub radius: f32,
}

impl Bounds {
    pub fn of(vertices: &[Vertex]) -> Bounds {
        if vertices.is_empty() {
            return Bounds::default();
        }
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for v in vertices {
            for i in 0..3 {
                min[i] = min[i].min(v.position[i]);
                max[i] = max[i].max(v.position[i]);
            }
        }
        let center = [
            (min[0] + max[0]) * 0.5,
            (min[1] + max[1]) * 0.5,
            (min[2] + max[2]) * 0.5,
        ];
        let radius = vertices
            .iter()
            .map(|v| {
                let d = [
                    v.position[0] - center[0],
                    v.position[1] - center[1],
                    v.position[2] - center[2],
                ];
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
            })
            .fold(0.0f32, f32::max);
        Bounds {
            min,
            max,
            center,
            radius,
        }
    }
}

/// A triangle mesh with one material.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MeshData {
    pub name: String,
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub bounds: Bounds,
    /// The index of its material in the asset, if any.
    pub material: Option<usize>,
    /// Coarser versions for distance (level 1 onward), each with its own indices into `vertices`
    /// and the screen-space error it was simplified to.
    pub lods: Vec<Lod>,
    /// Joint indices and weights per vertex, for a skinned mesh.
    #[serde(default)]
    pub skin: Option<SkinWeights>,
}

/// Per-vertex skinning: up to four joints (indices into the skin's joint list) and their weights.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SkinWeights {
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
}

/// A coarser index list over the same vertices.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Lod {
    pub indices: Vec<u32>,
    /// Relative geometric error of this level (0 exact), from the simplifier.
    pub error: f32,
}

impl MeshData {
    pub fn new(name: &str, vertices: Vec<Vertex>, indices: Vec<u32>) -> MeshData {
        let bounds = Bounds::of(&vertices);
        MeshData {
            name: name.to_owned(),
            vertices,
            indices,
            bounds,
            material: None,
            lods: Vec::new(),
            skin: None,
        }
    }

    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }

    /// Fills tangents from positions, normals and texture coordinates (per-triangle accumulation,
    /// Gram-Schmidt against the normal); meshes without usable coordinates get any perpendicular.
    pub fn compute_tangents(&mut self) {
        let n = self.vertices.len();
        let mut tan = vec![[0.0f32; 3]; n];
        let mut bit = vec![[0.0f32; 3]; n];
        for t in self.indices.chunks_exact(3) {
            let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
            let (pa, pb, pc) = (
                self.vertices[a].position,
                self.vertices[b].position,
                self.vertices[c].position,
            );
            let (ua, ub, uc) = (
                self.vertices[a].uv,
                self.vertices[b].uv,
                self.vertices[c].uv,
            );
            let e1 = sub(pb, pa);
            let e2 = sub(pc, pa);
            let (du1, dv1) = (ub[0] - ua[0], ub[1] - ua[1]);
            let (du2, dv2) = (uc[0] - ua[0], uc[1] - ua[1]);
            let det = du1 * dv2 - du2 * dv1;
            if det.abs() < 1e-12 {
                continue;
            }
            let r = 1.0 / det;
            let s = scale(sub(scale(e1, dv2), scale(e2, dv1)), r);
            let tt = scale(sub(scale(e2, du1), scale(e1, du2)), r);
            for &i in &[a, b, c] {
                tan[i] = add(tan[i], s);
                bit[i] = add(bit[i], tt);
            }
        }
        for (i, v) in self.vertices.iter_mut().enumerate() {
            let nrm = v.normal;
            let mut t = sub(tan[i], scale(nrm, dot(nrm, tan[i])));
            let len = dot(t, t).sqrt();
            if len < 1e-8 {
                t = any_perpendicular(nrm);
            } else {
                t = scale(t, 1.0 / len);
            }
            let w = if dot(cross(nrm, t), bit[i]) < 0.0 {
                -1.0
            } else {
                1.0
            };
            v.tangent = [t[0], t[1], t[2], w];
        }
    }
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn any_perpendicular(n: [f32; 3]) -> [f32; 3] {
    let a = if n[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let t = cross(n, a);
    let l = dot(t, t).sqrt().max(1e-12);
    scale(t, 1.0 / l)
}

/// Texture data decoded to RGBA8.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ImageData {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub rgba8: Vec<u8>,
    /// Color data (sRGB-encoded) rather than linear data (normals, roughness).
    pub srgb: bool,
}

/// A PBR metallic-roughness material (glTF 2.0's core model).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaterialData {
    pub name: String,
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: [f32; 3],
    /// Indices into the asset's images.
    pub base_color_texture: Option<usize>,
    pub metallic_roughness_texture: Option<usize>,
    pub normal_texture: Option<usize>,
    pub emissive_texture: Option<usize>,
    pub occlusion_texture: Option<usize>,
    pub alpha_mode: AlphaMode,
    pub alpha_cutoff: f32,
    pub double_sided: bool,
}

impl Default for MaterialData {
    fn default() -> Self {
        MaterialData {
            name: String::new(),
            base_color: [1.0; 4],
            metallic: 0.0,
            roughness: 0.5,
            emissive: [0.0; 3],
            base_color_texture: None,
            metallic_roughness_texture: None,
            normal_texture: None,
            emissive_texture: None,
            occlusion_texture: None,
            alpha_mode: AlphaMode::Opaque,
            alpha_cutoff: 0.5,
            double_sided: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlphaMode {
    #[default]
    Opaque,
    Mask,
    Blend,
}

/// One node of an imported scene: a mesh drawn at a transform relative to the asset's root.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeData {
    pub name: String,
    pub mesh: usize,
    /// Column-major 4x4, relative to the asset root.
    pub transform: [f32; 16],
    /// The skin deforming this mesh (an index into `ModelAsset::skins`).
    #[serde(default)]
    pub skin: Option<usize>,
}

/// A node of the file's hierarchy in its rest pose (bones are nodes).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkeletonNode {
    pub name: String,
    pub parent: Option<usize>,
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

/// A skin: its joints (skeleton node indices) and their inverse bind matrices (column-major).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkinAsset {
    pub joints: Vec<usize>,
    pub inverse_bind: Vec<[f32; 16]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelPath {
    Translation,
    Rotation,
    Scale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Interpolation {
    Step,
    Linear,
    /// Cubic spline: `values` holds (in-tangent, value, out-tangent) per key.
    Cubic,
}

/// Keys of one property of one node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub node: usize,
    pub path: ChannelPath,
    pub interpolation: Interpolation,
    pub times: Vec<f32>,
    /// xyz (w unused) for translation and scale, xyzw for rotation.
    pub values: Vec<[f32; 4]>,
}

/// An animation clip.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnimationClip {
    pub name: String,
    pub duration: f32,
    pub channels: Vec<Channel>,
}

/// An imported model: meshes, materials, images and the default scene's drawn nodes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelAsset {
    pub meshes: Vec<MeshData>,
    pub materials: Vec<MaterialData>,
    pub images: Vec<ImageData>,
    pub nodes: Vec<NodeData>,
    /// Every node of the file, in the file's order (skins and channels index it).
    #[serde(default)]
    pub skeleton: Vec<SkeletonNode>,
    #[serde(default)]
    pub skins: Vec<SkinAsset>,
    #[serde(default)]
    pub animations: Vec<AnimationClip>,
}

impl ModelAsset {
    pub fn triangles(&self) -> usize {
        self.nodes
            .iter()
            .map(|n| self.meshes.get(n.mesh).map_or(0, MeshData::triangles))
            .sum()
    }
}
