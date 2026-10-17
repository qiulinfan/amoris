//! Levels of detail and vertex order for the GPU (charter 4.4's Pioneer note of 2026-10-09,
//! docs/spec/lod.md): a mesh's coarser levels are index lists over its own vertices, simplified from
//! the full mesh by meshoptimizer with the geometric error each reached, in the mesh's own units;
//! every list is ordered for the post-transform vertex cache and the vertices for fetch locality,
//! coarsest level first, so a coarse level reads a short prefix of the vertex buffer.
//!
//! The same code runs natively (the importer's worker thread, the renderer's primitives) and in the
//! browser (the viewport imports a fetched `.glb` in WebAssembly): meshoptimizer's C++ builds for
//! `wasm32-unknown-unknown` with the stripped headers its crate ships, and the two C++ allocation
//! functions it references are provided below.

// Mesh processing for the renderer: its floats never reach the world or its hash.
#![allow(clippy::disallowed_methods)]

use crate::mesh::{Bounds, Lod, MeshData, Vertex};

/// The most levels a mesh has, the full mesh included (the culling pass keeps an instance's
/// level in 4 bits and reads at most this many errors; cull.wgsl's `MAX_LODS`).
pub const MAX_LEVELS: usize = 8;

/// How a chain is made.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LodOptions {
    /// Meshes with fewer triangles get no coarser levels (only the cache and fetch orders).
    pub min_triangles: usize,
    /// Each level aims at this share of the previous level's triangles.
    pub ratio: f32,
    /// Levels, the full mesh included (at most [`MAX_LEVELS`]).
    pub max_levels: usize,
    /// A level is kept only if it has at most this share of the previous level's triangles: the
    /// simplifier stops short of its target when the error bound or the topology (borders, attribute
    /// seams) holds it back, and a level that saves little costs a draw batch for nothing.
    pub min_reduction: f32,
    /// The largest geometric error a level may reach, as a share of the bounding sphere's radius.
    pub max_error: f32,
    /// No level is made below this many triangles.
    pub min_level_triangles: usize,
}

impl Default for LodOptions {
    fn default() -> Self {
        LodOptions {
            min_triangles: 64,
            ratio: 0.5,
            max_levels: MAX_LEVELS,
            min_reduction: 0.85,
            max_error: 0.5,
            min_level_triangles: 8,
        }
    }
}

/// Orders `mesh` for the GPU and fills `mesh.lods` with its coarser levels (any previous ones are
/// replaced). Vertices no index refers to are dropped; skin weights follow their vertices.
pub fn build(mesh: &mut MeshData, options: &LodOptions) {
    mesh.lods.clear();
    let n = mesh.vertices.len();
    if n == 0 || mesh.indices.len() < 3 || mesh.indices.iter().any(|&i| i as usize >= n) {
        return;
    }
    mesh.indices.truncate(mesh.indices.len() / 3 * 3);
    let full = meshopt::optimize_vertex_cache(&mesh.indices, n);
    let mut lods: Vec<Lod> = Vec::new();
    let triangles = full.len() / 3;
    if triangles >= options.min_triangles.max(1) {
        let bytes: &[u8] = bytemuck::cast_slice(&mesh.vertices);
        let Ok(adapter) = meshopt::VertexDataAdapter::new(bytes, size_of::<Vertex>(), 0) else {
            return;
        };
        let bound = Bounds::of(&mesh.vertices).radius * options.max_error;
        let mut previous = triangles;
        let mut error = 0.0f32;
        while lods.len() + 1 < options.max_levels.min(MAX_LEVELS) {
            let target = (previous as f32 * options.ratio) as usize;
            if target < options.min_level_triangles.max(1) {
                break;
            }
            let mut reached = 0.0f32;
            // Always from the full mesh: the error is then the level's own, not a sum over a chain.
            let indices = meshopt::simplify(
                &full,
                &adapter,
                target * 3,
                bound,
                meshopt::SimplifyOptions::ErrorAbsolute,
                Some(&mut reached),
            );
            let count = indices.len() / 3;
            if count == 0 || count as f32 > previous as f32 * options.min_reduction {
                break;
            }
            // Independent simplifications need not have increasing errors; selection assumes they
            // do, so a level is never credited with less error than a finer one.
            error = error.max(reached);
            lods.push(Lod {
                indices: meshopt::optimize_vertex_cache(&indices, n),
                error,
            });
            previous = count;
        }
    }
    mesh.indices = full;
    mesh.lods = lods;
    reorder_vertices(mesh);
}

/// Renumbers the vertices in the order the index lists first use them, coarsest level first, and
/// drops the unused ones (meshoptimizer's `optimizeVertexFetchRemap`, in Rust so the table keeps
/// one entry per old vertex).
fn reorder_vertices(mesh: &mut MeshData) {
    let n = mesh.vertices.len();
    let mut remap = vec![u32::MAX; n];
    let mut next = 0u32;
    let lists = mesh
        .lods
        .iter()
        .rev()
        .map(|l| &l.indices)
        .chain(std::iter::once(&mesh.indices));
    for list in lists {
        for &i in list {
            let r = &mut remap[i as usize];
            if *r == u32::MAX {
                *r = next;
                next += 1;
            }
        }
    }
    let keep = next as usize;
    let mut vertices = vec![Vertex::default(); keep];
    for (old, &new) in remap.iter().enumerate() {
        if new != u32::MAX {
            vertices[new as usize] = mesh.vertices[old];
        }
    }
    if let Some(skin) = &mut mesh.skin
        && skin.joints.len() == n
        && skin.weights.len() == n
    {
        let mut joints = vec![[0u16; 4]; keep];
        let mut weights = vec![[0.0f32; 4]; keep];
        for (old, &new) in remap.iter().enumerate() {
            if new != u32::MAX {
                joints[new as usize] = skin.joints[old];
                weights[new as usize] = skin.weights[old];
            }
        }
        skin.joints = joints;
        skin.weights = weights;
    }
    for i in mesh.indices.iter_mut() {
        *i = remap[*i as usize];
    }
    for l in &mut mesh.lods {
        for i in l.indices.iter_mut() {
            *i = remap[*i as usize];
        }
    }
    mesh.vertices = vertices;
    mesh.bounds = Bounds::of(&mesh.vertices);
}

/// The C++ allocation functions meshoptimizer's default allocator names (`operator new(size_t)`
/// and `operator delete(void*)`). A native build gets them from the C++ runtime; the WebAssembly
/// build links no C++ runtime, so they come from Rust's allocator here, each block carrying its
/// size in a 16-byte header (the alignment `operator new` promises).
#[cfg(target_arch = "wasm32")]
mod cxx_alloc {
    use std::alloc::{Layout, alloc, dealloc, handle_alloc_error};

    const HEADER: usize = 16;

    fn layout(size: usize) -> Layout {
        Layout::from_size_align(size.saturating_add(HEADER), HEADER)
            .unwrap_or_else(|_| handle_alloc_error(Layout::new::<[u8; HEADER]>()))
    }

    /// `operator new(unsigned long)`.
    ///
    /// # Safety
    /// Called by C++ code only, which frees the block with `_ZdlPv`.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn _Znwm(size: usize) -> *mut u8 {
        let layout = layout(size);
        // SAFETY: the layout has a nonzero size (the header); the header is written within it.
        unsafe {
            let block = alloc(layout);
            if block.is_null() {
                handle_alloc_error(layout);
            }
            block.cast::<usize>().write(size);
            block.add(HEADER)
        }
    }

    /// `operator delete(void*)`.
    ///
    /// # Safety
    /// `ptr` is null or came from `_Znwm` and was not freed before.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn _ZdlPv(ptr: *mut u8) {
        if ptr.is_null() {
            return;
        }
        // SAFETY: `_Znwm` returned `ptr` HEADER bytes into a block whose first word is its size.
        unsafe {
            let block = ptr.sub(HEADER);
            let size = block.cast::<usize>().read();
            dealloc(block, layout(size));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::{primitive, sphere};

    /// A bumpy sphere: enough triangles for a full chain, with a surface the simplifier has to
    /// approximate (a smooth sphere would also do, but its errors are all tiny).
    fn rock(subdivisions: u32) -> MeshData {
        let mut m = sphere(16 * subdivisions, 8 * subdivisions);
        for v in &mut m.vertices {
            let p = v.position;
            let bump = 1.0 + 0.08 * (p[0] * 23.0).sin() * (p[1] * 17.0).cos() * (p[2] * 19.0).sin();
            v.position = [p[0] * bump, p[1] * bump, p[2] * bump];
        }
        m.bounds = Bounds::of(&m.vertices);
        m
    }

    fn triangles(indices: &[u32]) -> usize {
        indices.len() / 3
    }

    #[test]
    fn a_chain_halves_with_growing_errors_over_the_same_vertices() {
        let mut m = rock(8);
        let before = m.triangles();
        build(&mut m, &LodOptions::default());
        assert_eq!(m.triangles(), before, "the full mesh keeps every triangle");
        assert!(m.lods.len() >= 4, "{} levels", m.lods.len());
        assert!(m.lods.len() < MAX_LEVELS);
        let mut prev_tris = before;
        let mut prev_err = 0.0;
        for (k, l) in m.lods.iter().enumerate() {
            let t = triangles(&l.indices);
            assert!(
                t as f32 <= prev_tris as f32 * 0.85,
                "level {} has {t} triangles after {prev_tris}",
                k + 1
            );
            assert!(
                t * 4 >= prev_tris,
                "level {} overshoots: {t} after {prev_tris}",
                k + 1
            );
            assert!(
                l.error >= prev_err && l.error > 0.0,
                "{} after {prev_err}",
                l.error
            );
            assert!(l.error <= m.bounds.radius * 0.5);
            assert!(l.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
            prev_tris = t;
            prev_err = l.error;
        }
    }

    #[test]
    fn coarse_levels_read_a_prefix_of_the_vertices() {
        let mut m = rock(6);
        build(&mut m, &LodOptions::default());
        let last = m.lods.last().expect("levels");
        let used = last.indices.iter().copied().max().unwrap_or(0) as usize + 1;
        let distinct: std::collections::BTreeSet<u32> = last.indices.iter().copied().collect();
        assert_eq!(
            used,
            distinct.len(),
            "the coarsest level uses vertices 0..{used} only"
        );
        // Every vertex is used by the full mesh, and nothing out of range.
        let all: std::collections::BTreeSet<u32> = m.indices.iter().copied().collect();
        assert_eq!(all.len(), m.vertices.len());
    }

    #[test]
    fn reordering_keeps_every_triangle_and_its_skin() {
        let mut m = rock(4);
        // A skin that encodes the vertex's original position, so a lost pairing shows.
        let n = m.vertices.len();
        let weights: Vec<[f32; 4]> = m
            .vertices
            .iter()
            .map(|v| [v.position[0], v.position[1], v.position[2], 1.0])
            .collect();
        m.skin = Some(crate::mesh::SkinWeights {
            joints: vec![[0; 4]; n],
            weights,
        });
        let original: std::collections::BTreeSet<[u32; 9]> = m
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| tri_key(&m.vertices, t))
            .collect();
        build(&mut m, &LodOptions::default());
        let after: std::collections::BTreeSet<[u32; 9]> = m
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| tri_key(&m.vertices, t))
            .collect();
        assert_eq!(original, after, "the same triangles, rotated or reordered");
        let skin = m.skin.as_ref().expect("skin");
        for (v, w) in m.vertices.iter().zip(&skin.weights) {
            assert_eq!(v.position, [w[0], w[1], w[2]]);
        }
    }

    /// A triangle as its corners' positions, rotated so the smallest corner comes first.
    fn tri_key(vs: &[Vertex], t: &[u32; 3]) -> [u32; 9] {
        let c: Vec<[u32; 3]> = t
            .iter()
            .map(|&i| vs[i as usize].position.map(f32::to_bits))
            .collect();
        let k = (0..3).min_by_key(|&k| c[k]).unwrap_or(0);
        let mut out = [0u32; 9];
        for j in 0..3 {
            out[j * 3..j * 3 + 3].copy_from_slice(&c[(k + j) % 3]);
        }
        out
    }

    #[test]
    fn small_meshes_get_no_levels_but_keep_their_triangles() {
        let mut cube = primitive("cube").expect("cube");
        let before = cube.triangles();
        build(&mut cube, &LodOptions::default());
        assert!(cube.lods.is_empty());
        assert_eq!(cube.triangles(), before);
        let mut sphere = primitive("sphere").expect("sphere");
        build(&mut sphere, &LodOptions::default());
        assert!(!sphere.lods.is_empty(), "the primitive sphere simplifies");
    }
}
