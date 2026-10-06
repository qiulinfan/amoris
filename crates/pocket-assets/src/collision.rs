//! Geometry-only, static collision extraction. Images are never loaded or decoded.
//!
//! The walk camera uses opaque triangle primitives from the default glTF scene (or first scene).
//! Alpha-mask foliage, alpha-blend glass, and skinned nodes are omitted. This is intentionally a
//! derived navigation collider, not authored gameplay physics or an animated collision mesh.

#![allow(clippy::disallowed_methods)]

use std::path::Path;

use glam::{Mat4, Vec3};
use pocket_contract::{Problem, detail};
use serde_json::json;

/// Static triangles with their glTF node transforms already applied. A caller can clone this
/// geometry for repeated scene instances, then apply each instance's world transform.
#[derive(Clone, Debug, Default)]
pub struct RawCollisionMesh {
    pub vertices: Vec<[f32; 3]>,
    pub indices: Vec<[u32; 3]>,
}

impl RawCollisionMesh {
    /// Applies an affine instance transform, preserving front-face winding under reflection.
    pub fn apply_transform(&mut self, transform: Mat4) -> Result<(), Problem> {
        if !transform.is_finite()
            || transform.x_axis.w.abs() > 1e-6
            || transform.y_axis.w.abs() > 1e-6
            || transform.z_axis.w.abs() > 1e-6
            || (transform.w_axis.w - 1.0).abs() > 1e-6
        {
            return Err(failed(
                Path::new("<instance>"),
                "collision transform must be finite and affine",
            ));
        }
        let determinant = transform.determinant();
        if !determinant.is_finite() || determinant.abs() <= 1e-12 {
            return Err(failed(
                Path::new("<instance>"),
                "collision transform must be nonsingular",
            ));
        }
        let mut transformed = Vec::with_capacity(self.vertices.len());
        for &point in &self.vertices {
            let point = transform.transform_point3(Vec3::from_array(point));
            if !point.is_finite() {
                return Err(failed(
                    Path::new("<instance>"),
                    "transformed collision position is not finite",
                ));
            }
            transformed.push(point.to_array());
        }
        self.vertices = transformed;
        if determinant < 0.0 {
            for triangle in &mut self.indices {
                triangle.swap(1, 2);
            }
        }
        Ok(())
    }
}

fn failed(path: &Path, why: impl std::fmt::Display) -> Problem {
    Problem::new(
        "asset.collision_failed",
        format!(
            "{} cannot provide static collision geometry: {why}.",
            path.display()
        ),
        detail([("path", json!(path.display().to_string()))]),
    )
}

/// A built-in model's geometry, without any imported textures or material work.
pub fn collision_primitive(name: &str) -> Option<RawCollisionMesh> {
    let mesh = crate::primitives::primitive(name)?;
    let vertices: Vec<[f32; 3]> = mesh
        .vertices
        .into_iter()
        .map(|vertex| vertex.position)
        .collect();
    let indices = mesh
        .indices
        .chunks_exact(3)
        .filter_map(|v| {
            let p = Vec3::from_array(vertices[v[0] as usize]);
            let q = Vec3::from_array(vertices[v[1] as usize]);
            let r = Vec3::from_array(vertices[v[2] as usize]);
            ((q - p).cross(r - p).length_squared() > 1e-16).then_some([v[0], v[1], v[2]])
        })
        .collect();
    Some(RawCollisionMesh { vertices, indices })
}

/// Loads position/index buffers only, including their scene-node instances. `selector` accepts
/// a node name (including its descendants), a mesh name, or the importer's primitive name such
/// as `Building.0` / `mesh0.0`. A recognized selector containing only excluded alpha geometry
/// returns an empty mesh; an unknown selector is an error.
pub fn load_collision_gltf(
    path: &Path,
    selector: Option<&str>,
) -> Result<RawCollisionMesh, Problem> {
    let gltf = gltf::Gltf::open(path).map_err(|error| failed(path, error))?;
    // This API explicitly skips gltf::import_images, unlike import_gltf/gltf::import.
    let buffers = gltf::import_buffers(&gltf.document, path.parent(), gltf.blob)
        .map_err(|error| failed(path, error))?;
    let scene = gltf
        .document
        .default_scene()
        .or_else(|| gltf.document.scenes().next())
        .ok_or_else(|| failed(path, "the glTF has no scene"))?;
    let mut geometry = RawCollisionMesh::default();
    let mut matched = selector.is_none();
    for node in scene.nodes() {
        walk(
            node,
            Mat4::IDENTITY,
            false,
            selector,
            &buffers,
            &mut geometry,
            &mut matched,
            path,
        )?;
    }
    if !matched {
        return Err(failed(
            path,
            format!("unknown model selector {:?}", selector.unwrap()),
        ));
    }
    Ok(geometry)
}

#[allow(clippy::too_many_arguments)]
fn walk(
    node: gltf::Node<'_>,
    parent: Mat4,
    ancestor_selected: bool,
    selector: Option<&str>,
    buffers: &[gltf::buffer::Data],
    geometry: &mut RawCollisionMesh,
    matched: &mut bool,
    path: &Path,
) -> Result<(), Problem> {
    let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
    if !world.is_finite() {
        return Err(failed(path, "nonfinite glTF node transform"));
    }
    let determinant = world.determinant();
    let node_selected = ancestor_selected || selector.is_some_and(|name| node.name() == Some(name));
    *matched |= node_selected;
    if let Some(mesh) = node.mesh() {
        let mesh_selected = selector.is_some_and(|name| mesh.name() == Some(name));
        *matched |= mesh_selected;
        if node.skin().is_none() && determinant.abs() > 1e-12 {
            for (index, primitive) in mesh.primitives().enumerate() {
                let primitive_name = match (mesh.name(), mesh.primitives().len()) {
                    (Some(name), 1) => name.to_owned(),
                    (Some(name), _) => format!("{name}.{index}"),
                    (None, _) => format!("mesh{}.{index}", mesh.index()),
                };
                let primitive_selected = selector.is_some_and(|name| name == primitive_name);
                *matched |= primitive_selected;
                if selector.is_some() && !node_selected && !mesh_selected && !primitive_selected {
                    continue;
                }
                if primitive.mode() != gltf::mesh::Mode::Triangles
                    || primitive.material().alpha_mode() != gltf::material::AlphaMode::Opaque
                {
                    continue;
                }
                let reader = primitive
                    .reader(|buffer| buffers.get(buffer.index()).map(|data| data.0.as_slice()));
                let Some(positions) = reader.read_positions() else {
                    continue;
                };
                let positions: Vec<[f32; 3]> = positions.collect();
                if positions.iter().flatten().any(|value| !value.is_finite()) {
                    return Err(failed(path, "nonfinite glTF collision position"));
                }
                let indices: Vec<u32> = reader.read_indices().map_or_else(
                    || (0..positions.len() as u32).collect(),
                    |indices| indices.into_u32().collect(),
                );
                if indices.len() % 3 != 0 || indices.iter().any(|&i| i as usize >= positions.len())
                {
                    return Err(failed(
                        path,
                        "triangle indices do not match collision vertices",
                    ));
                }
                let mut part = RawCollisionMesh {
                    vertices: positions,
                    indices: Vec::with_capacity(indices.len() / 3),
                };
                part.apply_transform(world)?;
                for triangle in indices.chunks_exact(3) {
                    let [a, b, c] = [triangle[0], triangle[1], triangle[2]];
                    let p = Vec3::from_array(part.vertices[a as usize]);
                    let q = Vec3::from_array(part.vertices[b as usize]);
                    let r = Vec3::from_array(part.vertices[c as usize]);
                    if (q - p).cross(r - p).length_squared() <= 1e-16 {
                        continue;
                    }
                    part.indices.push(if determinant < 0.0 {
                        [a, c, b]
                    } else {
                        [a, b, c]
                    });
                }
                if part.indices.is_empty() {
                    continue;
                }
                let base = u32::try_from(geometry.vertices.len())
                    .map_err(|_| failed(path, "collision vertex count exceeds u32"))?;
                if geometry.vertices.len() + part.vertices.len() > u32::MAX as usize {
                    return Err(failed(path, "collision vertex count exceeds u32"));
                }
                geometry.vertices.extend(part.vertices);
                geometry.indices.extend(
                    part.indices
                        .into_iter()
                        .map(|v| [v[0] + base, v[1] + base, v[2] + base]),
                );
            }
        }
    }
    for child in node.children() {
        walk(
            child,
            world,
            node_selected,
            selector,
            buffers,
            geometry,
            matched,
            path,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn fixture() -> std::path::PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "amoris-collision-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let mut binary = Vec::new();
        for value in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
            binary.extend(value.to_le_bytes());
        }
        for value in [0u16, 1, 2, 0, 0, 1] {
            binary.extend(value.to_le_bytes());
        }
        std::fs::write(directory.join("mesh.bin"), binary).unwrap();
        let document = json!({
            "asset": {"version": "2.0"}, "scene": 0,
            "scenes": [{"nodes": [0]}],
            "nodes": [{"name": "parent", "translation": [5, 0, 0], "children": [1]},
                {"name": "wall", "translation": [1, 0, 0], "scale": [-1, 1, 1], "mesh": 0}],
            "buffers": [{"uri": "mesh.bin", "byteLength": 48}],
            "bufferViews": [{"buffer": 0, "byteOffset": 0, "byteLength": 36}, {"buffer": 0, "byteOffset": 36, "byteLength": 12}],
            "accessors": [{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0, 0, 0], "max": [1, 1, 0]},
                {"bufferView": 1, "componentType": 5123, "count": 6, "type": "SCALAR"}],
            "materials": [{}, {"alphaMode": "MASK"}, {"alphaMode": "BLEND"}],
            "meshes": [{"name": "Building", "primitives": [
                {"attributes": {"POSITION": 0}, "indices": 1, "material": 0},
                {"attributes": {"POSITION": 0}, "indices": 1, "material": 1},
                {"attributes": {"POSITION": 0}, "indices": 1, "material": 2}]}],
            "images": [{"uri": "deliberately-missing-image.png"}]
        });
        std::fs::write(directory.join("scene.gltf"), document.to_string()).unwrap();
        directory
    }

    #[test]
    fn geometry_only_applies_nested_nodes_filters_alpha_and_preserves_winding() {
        let directory = fixture();
        let mesh = load_collision_gltf(&directory.join("scene.gltf"), None).unwrap();
        assert_eq!(
            mesh.vertices,
            [[6.0, 0.0, 0.0], [5.0, 0.0, 0.0], [6.0, 1.0, 0.0]]
        );
        assert_eq!(mesh.indices, [[0, 2, 1]]);
        // Missing image URI demonstrates that geometry extraction never reads image data.
        assert_eq!(
            load_collision_gltf(&directory.join("scene.gltf"), Some("Building.0"))
                .unwrap()
                .indices
                .len(),
            1
        );
        assert!(
            load_collision_gltf(&directory.join("scene.gltf"), Some("Building.1"))
                .unwrap()
                .indices
                .is_empty()
        );
        assert_eq!(
            load_collision_gltf(&directory.join("scene.gltf"), Some("parent"))
                .unwrap()
                .indices
                .len(),
            1
        );
        assert!(load_collision_gltf(&directory.join("scene.gltf"), Some("unknown")).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn builtins_clone_and_instance_transform_flip_winding() {
        let cube = collision_primitive("cube").unwrap();
        assert_eq!(cube.indices.len(), 12);
        let mut reflected = cube.clone();
        reflected
            .apply_transform(Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)))
            .unwrap();
        assert_eq!(
            reflected.indices[0],
            [cube.indices[0][0], cube.indices[0][2], cube.indices[0][1]]
        );
        assert!(reflected.apply_transform(Mat4::ZERO).is_err());
        assert!(collision_primitive("not-a-primitive").is_none());
    }
}
