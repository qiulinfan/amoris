//! Offline static diffuse GI baking. Rays use the same primitive/glTF geometry as the renderer.
//!
//! A probe stores *incoming radiance*: a primary ray misses into the environment, or hits a
//! surface whose emission, explicitly shadowed punctual/direct illumination, and further diffuse
//! bounces are accumulated. The receiving surface's direct lights are therefore not baked twice.
//! SH coefficients are radiance integrals (4π/N), before the Lambert band convolution / π.
//!
//! This is a diffuse transport baseline, not a GGX path tracer. Metallic surfaces contribute only
//! their diffuse lobe; specular transport, transmission, atmosphere and animated geometry are
//! rejected or explicitly outside this bake. Color skies, opaque/alpha-mask meshes, normal maps,
//! base-color/emissive/metallic textures, and static glTF node transforms are supported.

#![allow(clippy::disallowed_methods)]

use std::collections::BTreeMap;
use std::f32::consts::{PI, TAU};
use std::path::{Component, Path};
use std::sync::Arc;
use std::time::Instant;

use glam::{Mat3, Mat4, Quat, Vec2, Vec3, Vec4};
use pocket_assets::gi::{BAKED_GI_FORMAT, BakedGi, BakedProbe, MAX_DISTANCE_MOMENTS, MAX_PROBES};
use pocket_assets::mesh::{AlphaMode, ImageData, MaterialData, MeshData, ModelAsset};
use pocket_assets::visual::{Environment, Light, LightKind, Model, SkyKind};
use serde::Serialize;
use serde_json::Value;

const EPSILON: f32 = 1e-4;
const DISTANCE_SAMPLES: u32 = 8;

#[derive(Clone, Debug, Serialize)]
pub struct BakeOptions {
    pub origin: [f32; 3],
    pub spacing: [f32; 3],
    pub dimensions: [u32; 3],
    pub rays_per_probe: u32,
    /// Number of surface interactions after the ray leaves the probe (1 includes direct bounce).
    pub bounces: u32,
    pub seed: u64,
    pub distance_resolution: u32,
    /// Visibility distance cap only; radiance paths are not truncated at this distance.
    pub max_distance: f32,
}

impl Default for BakeOptions {
    fn default() -> Self {
        Self {
            origin: [-4.0, 0.5, -4.0],
            spacing: [1.0; 3],
            dimensions: [9, 5, 9],
            rays_per_probe: 2048,
            bounces: 3,
            seed: 1,
            distance_resolution: 8,
            max_distance: 100.0,
        }
    }
}

impl BakeOptions {
    pub fn validate(&self) -> Result<usize, String> {
        if !self.origin.iter().all(|v| v.is_finite() && v.abs() <= 1e6)
            || !self
                .spacing
                .iter()
                .all(|v| v.is_finite() && *v > 0.0 && *v <= 1e6)
        {
            return Err(
                "origin must be finite within ±1e6; spacing must be finite in (0, 1e6]".into(),
            );
        }
        if self.dimensions.iter().any(|&v| v == 0 || v > 1024) {
            return Err("each grid dimension must be in 1..=1024".into());
        }
        let probes = self
            .dimensions
            .iter()
            .try_fold(1u64, |p, &n| p.checked_mul(n as u64))
            .ok_or("grid dimensions overflow")?;
        if probes > MAX_PROBES as u64 {
            return Err(format!("grid exceeds {MAX_PROBES} probes"));
        }
        for axis in 0..3 {
            let end = self.origin[axis] + self.spacing[axis] * (self.dimensions[axis] - 1) as f32;
            if !end.is_finite() || end.abs() > 1e6 {
                return Err("grid extent must remain within ±1e6".into());
            }
        }
        if !(1..=1_048_576).contains(&self.rays_per_probe) || !(1..=16).contains(&self.bounces) {
            return Err("rays must be in 1..=1048576 and bounces in 1..=16".into());
        }
        if !(1..=32).contains(&self.distance_resolution)
            || !self.max_distance.is_finite()
            || !(EPSILON..=1e6).contains(&self.max_distance)
        {
            return Err(
                "distance resolution must be in 1..=32; max distance in [0.0001, 1e6]".into(),
            );
        }
        let texels = u64::from(self.distance_resolution).pow(2);
        if probes * texels > MAX_DISTANCE_MOMENTS as u64 {
            return Err(format!(
                "bake exceeds {MAX_DISTANCE_MOMENTS} distance moments"
            ));
        }
        if probes
            * (u64::from(self.rays_per_probe) * u64::from(self.bounces)
                + texels * u64::from(DISTANCE_SAMPLES))
            > 1_000_000_000
        {
            return Err("bake exceeds one billion path/visibility rays; split the volume".into());
        }
        Ok(probes as usize)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct BakeStats {
    pub load_seconds: f64,
    pub bake_seconds: f64,
    pub probes: usize,
    pub rays: u64,
    pub secondary_rays: u64,
    pub shadow_rays: u64,
    pub distance_rays: u64,
    pub classification_rays: u64,
    pub radiance_rays: u64,
    pub invalid_probe_indices: Vec<usize>,
    pub triangles: usize,
    pub scene_signature: String,
}

#[derive(Clone)]
struct Surface {
    material: MaterialData,
    images: Arc<Vec<ImageData>>,
    casts_shadows: bool,
}

#[derive(Clone, Copy)]
struct Triangle {
    p: [Vec3; 3],
    n: [Vec3; 3],
    uv: [Vec2; 3],
    tangent: [Vec4; 3],
    surface: usize,
}

#[derive(Clone, Copy)]
struct Node {
    min: Vec3,
    max: Vec3,
    start: usize,
    count: usize,
    children: Option<[usize; 2]>,
}

#[derive(Clone)]
struct SceneLight {
    light: Light,
    position: Vec3,
    direction: Vec3,
}

/// CPU triangle scene shared by the baker and future reference tracing/training tools.
/// BVH median splits are deterministic. All scene coordinates and material colors are linear.
pub struct RayScene {
    triangles: Vec<Triangle>,
    surfaces: Vec<Surface>,
    nodes: Vec<Node>,
    lights: Vec<SceneLight>,
    environment: Vec3,
    environment_settings: Environment,
    signature: String,
}

/// Static world geometry for acceleration-structure upload or reference-data tools.
#[derive(Clone, Copy, Debug)]
pub struct WorldTriangle {
    pub positions: [[f32; 3]; 3],
    pub normals: [[f32; 3]; 3],
    pub uv: [[f32; 2]; 3],
    pub tangents: [[f32; 4]; 3],
    pub material: usize,
}

/// Material plus shared decoded images, without any GPU resource ownership.
#[derive(Clone, Debug)]
pub struct WorldMaterial {
    pub material: MaterialData,
    pub images: Arc<Vec<ImageData>>,
    pub casts_shadows: bool,
}

/// Explicit light definition in world space for GPU/reference tracers.
#[derive(Clone, Debug)]
pub struct WorldLight {
    pub light: Light,
    pub position: [f32; 3],
    pub direction: [f32; 3],
}

/// A nearest hit, with its material sampled at the barycentric UV.
#[derive(Clone, Copy, Debug)]
pub struct SurfaceHit {
    pub front_face: bool,
    pub distance: f32,
    pub position: Vec3,
    pub normal: Vec3,
    pub geometric_normal: Vec3,
    pub albedo: Vec3,
    pub emissive: Vec3,
    pub metallic: f32,
}

#[derive(Default)]
struct RayCounts {
    primary: u64,
    classification: u64,
    secondary: u64,
    shadow: u64,
    distance: u64,
}

impl RayScene {
    #[cfg(all(not(target_arch = "wasm32"), feature = "import"))]
    pub fn from_project(root: &Path) -> Result<Self, String> {
        Self::load_project(root, false, "scene.json")
    }

    /// Import a static surface scene for full path tracing. Baking retains its stricter contract.
    #[cfg(all(not(target_arch = "wasm32"), feature = "import"))]
    pub fn for_path_tracing(root: &Path) -> Result<Self, String> {
        Self::load_project(root, true, "scene.json")
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "import"))]
    fn load_project(root: &Path, path_tracing: bool, scene_file: &str) -> Result<Self, String> {
        if Path::new(scene_file)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err("scene file must be a project-relative path without ..".into());
        }
        let path = root.join(scene_file);
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let scene: Value =
            serde_json::from_slice(&bytes).map_err(|e| format!("scene.json: {e}"))?;
        if scene["format"] != "pocket-scene" || scene["version"] != 1 {
            return Err("expected pocket-scene version 1".into());
        }
        let entities = scene["entities"]
            .as_array()
            .ok_or("scene.entities must be an array")?;
        let mut result = Self {
            triangles: Vec::new(),
            surfaces: Vec::new(),
            nodes: Vec::new(),
            lights: Vec::new(),
            environment: Vec3::ZERO,
            environment_settings: Environment::default(),
            signature: String::new(),
        };
        let mut assets = BTreeMap::<String, Arc<ModelAsset>>::new();
        let mut image_cache = BTreeMap::<String, Arc<Vec<ImageData>>>::new();
        let mut hash = Signature::new();
        hash.feed(&bytes);
        let mut environment_found = false;
        let mut sun_found = false;
        for (index, entity) in entities.iter().enumerate() {
            if entity.get("prefab").is_some() || entity.get("parent").is_some() {
                return Err(format!(
                    "entity {index}: static baker needs expanded, unparented entities"
                ));
            }
            let components = entity["components"]
                .as_object()
                .ok_or("entity.components must be an object")?;
            let transform = components.get("Transform");
            let position = vector(transform.and_then(|t| t.get("position")), [0.0; 3])?;
            let rotation = vector(
                transform.and_then(|t| t.get("rotation")),
                [0.0, 0.0, 0.0, 1.0],
            )?;
            let rotation = Quat::from_array(rotation);
            if (rotation.length_squared() - 1.0).abs() > 1e-3 {
                return Err(format!(
                    "entity {index}: rotation must be a unit quaternion"
                ));
            }
            for unsupported in ["Animator", "Splat", "Ocean", "Sea", "ParticleEmitter"] {
                if components.contains_key(unsupported) {
                    return Err(format!(
                        "entity {index}: {unsupported} is not supported by the static triangle baker"
                    ));
                }
            }
            if let Some(value) = components.get("Environment") {
                if !environment_found {
                    let env: Environment = component(value)?;
                    validate_numbers(&[env.ambient], "environment ambient", 0.0, 2.0)?;
                    validate_numbers(&env.sky_color, "environment sky color", 0.0, 1e6)?;
                    if !path_tracing && env.sky != SkyKind::Color {
                        return Err("static GI bake supports sky: color; atmosphere needs a matching sky-radiance export".into());
                    }
                    result.environment =
                        Vec3::from(env.sky_color.map(|n| n as f32)) * env.ambient as f32;
                    result.environment_settings = env;
                    environment_found = true;
                }
            }
            if let Some(value) = components.get("Light") {
                let light: Light = component(value)?;
                validate_numbers(&light.color, "light color", 0.0, 1e6)?;
                validate_numbers(&[light.intensity], "light intensity", 0.0, 1e6)?;
                validate_numbers(&[light.range], "light range", EPSILON as f64, 1e6)?;
                if light.inner_deg < 0.0
                    || light.outer_deg < light.inner_deg
                    || light.outer_deg > 90.0
                    || !light.inner_deg.is_finite()
                    || !light.outer_deg.is_finite()
                {
                    return Err("spot angles must be finite with 0 <= inner <= outer <= 90".into());
                }
                // The renderer lights with the first directional entity only.
                if path_tracing || light.kind != LightKind::Directional || !sun_found {
                    sun_found |= light.kind == LightKind::Directional;
                    result.lights.push(SceneLight {
                        light,
                        position: Vec3::from(position),
                        direction: rotation * -Vec3::Z,
                    });
                }
            }
            let Some(value) = components.get("Model") else {
                continue;
            };
            let model: Model = component(value)?;
            if !model.visible {
                continue;
            }
            validate_numbers(&model.color, "model color", 0.0, 1e6)?;
            validate_numbers(&model.emissive, "model emissive", 0.0, 1e6)?;
            validate_numbers(
                &[model.metallic, model.roughness],
                "model metallic/roughness",
                0.0,
                1.0,
            )?;
            if !model
                .scale
                .iter()
                .all(|s| s.is_finite() && *s >= EPSILON as f64 && *s <= 1e6)
            {
                return Err(format!(
                    "entity {index}: scale must be finite in [0.0001, 1e6]; mirrored transforms need a frozen geometry export"
                ));
            }
            let pose = (
                Vec3::from(position),
                rotation,
                Vec3::from(model.scale.map(|s| s as f32)),
            );
            if let Some(mesh) = pocket_assets::primitives::primitive(&model.mesh) {
                let surface = resolve_surface(
                    root,
                    &model,
                    None,
                    &mut assets,
                    &mut image_cache,
                    path_tracing,
                )?;
                result.add_mesh(&mesh, matrix(pose), surface)?;
                continue;
            }
            let (path, selector) = split(&model.mesh);
            let asset = load_asset(root, path, &mut assets)?;
            if let Some(selector) = selector {
                let mi = asset
                    .meshes
                    .iter()
                    .position(|m| m.name == selector)
                    .or_else(|| selector.parse::<usize>().ok())
                    .filter(|&i| i < asset.meshes.len())
                    .ok_or_else(|| format!("{}: unknown mesh selector #{selector}", path))?;
                let mesh = &asset.meshes[mi];
                let surface = resolve_surface(
                    root,
                    &model,
                    Some((path, mesh, &asset)),
                    &mut assets,
                    &mut image_cache,
                    path_tracing,
                )?;
                result.add_mesh(mesh, matrix(pose), surface)?;
            } else {
                for node in &asset.nodes {
                    if node.skin.is_some() {
                        return Err(format!("{path}: skinned nodes need a frozen-pose export"));
                    }
                    let mesh = asset
                        .meshes
                        .get(node.mesh)
                        .ok_or("glTF node mesh index out of bounds")?;
                    let (local_scale, local_rotation, local_position) =
                        Mat4::from_cols_array(&node.transform).to_scale_rotation_translation();
                    // Match renderer Scene::compose exactly (TRS composition rather than introducing shear).
                    let combined = (
                        pose.0 + pose.1 * (pose.2 * local_position),
                        (pose.1 * local_rotation).normalize(),
                        pose.2 * local_scale,
                    );
                    let surface = resolve_surface(
                        root,
                        &model,
                        Some((path, mesh, &asset)),
                        &mut assets,
                        &mut image_cache,
                        path_tracing,
                    )?;
                    result.add_mesh(mesh, matrix(combined), surface)?;
                }
            }
        }
        if !environment_found && !path_tracing {
            return Err("scene needs an explicit Environment with sky: color (default atmosphere is not baked)".into());
        }
        for (path, asset) in assets {
            hash.feed(path.as_bytes());
            // Decoded images and mesh/node/material values include external .gltf/.bin/image content.
            hash.feed(&serde_json::to_vec(&*asset).map_err(|e| e.to_string())?);
        }
        let signature_kind = if path_tracing { "surface" } else { "diffuse" };
        result.signature = format!("fnv1a64-static-{signature_kind}-v1:{:016x}", hash.0);
        if !result.triangles.is_empty() {
            result.build_node(0, result.triangles.len());
        }
        Ok(result)
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "import"))]
    pub fn for_path_tracing_file(root: &Path, scene_file: &str) -> Result<Self, String> {
        Self::load_project(root, true, scene_file)
    }

    pub fn triangle_count(&self) -> usize {
        self.triangles.len()
    }
    pub fn scene_signature(&self) -> &str {
        &self.signature
    }

    /// Constant incoming diffuse environment radiance (sky_color multiplied by ambient).
    pub fn world_sky_color(&self) -> Vec3 {
        self.environment
    }

    pub fn world_environment(&self) -> &Environment {
        &self.environment_settings
    }

    pub fn environment_radiance(&self) -> [f32; 3] {
        self.environment.to_array()
    }

    pub fn explicit_light_count(&self) -> usize {
        self.lights.len()
    }

    pub fn world_lights(&self) -> impl Iterator<Item = WorldLight> + '_ {
        self.lights.iter().map(|source| WorldLight {
            light: source.light.clone(),
            position: source.position.to_array(),
            direction: source.direction.to_array(),
        })
    }

    pub fn world_triangles(&self) -> impl Iterator<Item = WorldTriangle> + '_ {
        self.triangles.iter().map(|triangle| WorldTriangle {
            positions: triangle.p.map(|v| v.to_array()),
            normals: triangle.n.map(|v| v.to_array()),
            uv: triangle.uv.map(|v| v.to_array()),
            tangents: triangle.tangent.map(|v| v.to_array()),
            material: triangle.surface,
        })
    }

    pub fn world_materials(&self) -> impl Iterator<Item = WorldMaterial> + '_ {
        self.surfaces.iter().map(|surface| WorldMaterial {
            material: surface.material.clone(),
            images: surface.images.clone(),
            casts_shadows: surface.casts_shadows,
        })
    }

    /// Closest material-visible triangle; alpha-mask cutouts and one-sided faces are respected.
    pub fn intersect(
        &self,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
    ) -> Option<SurfaceHit> {
        self.intersection(origin, direction, max_distance, false)
    }

    /// Reference radiance query for tools. `direction` must be unit length; deterministic seed.
    pub fn trace_radiance(
        &self,
        origin: Vec3,
        direction: Vec3,
        bounces: u32,
        seed: u64,
    ) -> Result<Vec3, String> {
        if !origin.is_finite()
            || !direction.is_finite()
            || (direction.length_squared() - 1.0).abs() > 1e-3
            || !(1..=16).contains(&bounces)
        {
            return Err("trace needs finite origin, unit direction and 1..=16 bounces".into());
        }
        let mut rng = Rng(seed);
        Ok(self.trace(
            origin,
            direction,
            bounces,
            &mut rng,
            &mut RayCounts::default(),
        ))
    }

    fn add_mesh(
        &mut self,
        mesh: &MeshData,
        transform: Mat4,
        surface: Surface,
    ) -> Result<(), String> {
        if mesh.skin.is_some() {
            return Err(format!(
                "{}: skinned meshes need a frozen-pose export",
                mesh.name
            ));
        }
        if !transform.is_finite() || transform.determinant().abs() < 1e-12 {
            return Err(format!("{}: singular or invalid transform", mesh.name));
        }
        if mesh.indices.len() % 3 != 0 {
            return Err("mesh indices must form triangles".into());
        }
        let sid = self.surfaces.len();
        let normal_matrix = Mat3::from_mat4(transform.inverse().transpose());
        let tangent_matrix = Mat3::from_mat4(transform);
        if transform.determinant() < 0.0 {
            return Err("mirrored glTF transforms need a frozen geometry export".into());
        }
        for indices in mesh.indices.chunks_exact(3) {
            let mut triangle = Triangle {
                p: [Vec3::ZERO; 3],
                n: [Vec3::ZERO; 3],
                uv: [Vec2::ZERO; 3],
                tangent: [Vec4::ZERO; 3],
                surface: sid,
            };
            for k in 0..3 {
                let vertex = mesh
                    .vertices
                    .get(indices[k] as usize)
                    .ok_or("mesh index out of bounds")?;
                triangle.p[k] = transform.transform_point3(Vec3::from(vertex.position));
                triangle.n[k] = (normal_matrix * Vec3::from(vertex.normal)).normalize_or_zero();
                triangle.uv[k] = Vec2::from(vertex.uv);
                let tangent = (tangent_matrix
                    * Vec3::new(vertex.tangent[0], vertex.tangent[1], vertex.tangent[2]))
                .normalize_or_zero();
                triangle.tangent[k] = tangent.extend(vertex.tangent[3]);
                if !triangle.p[k].is_finite()
                    || !triangle.n[k].is_finite()
                    || !triangle.uv[k].is_finite()
                {
                    return Err("mesh has nonfinite vertex data".into());
                }
            }
            if (triangle.p[1] - triangle.p[0])
                .cross(triangle.p[2] - triangle.p[0])
                .length_squared()
                > 1e-16
            {
                self.triangles.push(triangle);
            }
        }
        self.surfaces.push(surface);
        Ok(())
    }

    fn build_node(&mut self, start: usize, count: usize) -> usize {
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        let mut centroid_min = min;
        let mut centroid_max = max;
        for triangle in &self.triangles[start..start + count] {
            for p in triangle.p {
                min = min.min(p);
                max = max.max(p);
            }
            let centroid = (triangle.p[0] + triangle.p[1] + triangle.p[2]) / 3.0;
            centroid_min = centroid_min.min(centroid);
            centroid_max = centroid_max.max(centroid);
        }
        let index = self.nodes.len();
        self.nodes.push(Node {
            min,
            max,
            start,
            count,
            children: None,
        });
        if count > 8 {
            let extent = centroid_max - centroid_min;
            let axis = if extent.x >= extent.y && extent.x >= extent.z {
                0
            } else if extent.y >= extent.z {
                1
            } else {
                2
            };
            self.triangles[start..start + count].sort_by(|a, b| {
                let a = a.p.iter().map(|p| p[axis]).sum::<f32>();
                let b = b.p.iter().map(|p| p[axis]).sum::<f32>();
                a.total_cmp(&b)
            });
            let half = count / 2;
            let a = self.build_node(start, half);
            let b = self.build_node(start + half, count - half);
            self.nodes[index].children = Some([a, b]);
        }
        index
    }

    fn intersection(
        &self,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
        shadows_only: bool,
    ) -> Option<SurfaceHit> {
        self.intersection_mode(origin, direction, max_distance, shadows_only, false)
    }

    fn intersection_mode(
        &self,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
        shadows_only: bool,
        force_two_sided: bool,
    ) -> Option<SurfaceHit> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut stack = vec![0usize];
        let mut closest = max_distance;
        let mut result = None;
        while let Some(index) = stack.pop() {
            let node = self.nodes[index];
            if !bounds_hit(origin, direction, node.min, node.max, closest) {
                continue;
            }
            if let Some([a, b]) = node.children {
                stack.push(b);
                stack.push(a);
                continue;
            }
            for triangle in &self.triangles[node.start..node.start + node.count] {
                let surface = &self.surfaces[triangle.surface];
                if shadows_only && !surface.casts_shadows {
                    continue;
                }
                let edge1 = triangle.p[1] - triangle.p[0];
                let edge2 = triangle.p[2] - triangle.p[0];
                let cross = direction.cross(edge2);
                let det = edge1.dot(cross);
                if det.abs() < 1e-10
                    || (!force_two_sided && !surface.material.double_sided && det < 0.0)
                {
                    continue;
                }
                let inv = det.recip();
                let delta = origin - triangle.p[0];
                let u = delta.dot(cross) * inv;
                if !(0.0..=1.0).contains(&u) {
                    continue;
                }
                let q = delta.cross(edge1);
                let v = direction.dot(q) * inv;
                if v < 0.0 || u + v > 1.0 {
                    continue;
                }
                let distance = edge2.dot(q) * inv;
                if distance <= EPSILON || distance >= closest {
                    continue;
                }
                let weight = Vec3::new(1.0 - u - v, u, v);
                let uv = triangle.uv[0] * weight.x
                    + triangle.uv[1] * weight.y
                    + triangle.uv[2] * weight.z;
                let material = &surface.material;
                let base = Vec4::from_array(material.base_color)
                    * sample_texture(surface, material.base_color_texture, uv, true);
                if material.alpha_mode == AlphaMode::Mask && base.w < material.alpha_cutoff {
                    continue;
                }
                let mut normal = (triangle.n[0] * weight.x
                    + triangle.n[1] * weight.y
                    + triangle.n[2] * weight.z)
                    .normalize_or_zero();
                let mut geometric_normal = edge1.cross(edge2).normalize();
                if normal == Vec3::ZERO {
                    normal = geometric_normal;
                }
                if material.normal_texture.is_some() {
                    let tangent = triangle.tangent[0] * weight.x
                        + triangle.tangent[1] * weight.y
                        + triangle.tangent[2] * weight.z;
                    let t = (tangent.truncate() - normal * normal.dot(tangent.truncate()))
                        .normalize_or_zero();
                    let bitangent = normal.cross(t) * tangent.w.signum();
                    let n = sample_texture(surface, material.normal_texture, uv, false).truncate()
                        * 2.0
                        - Vec3::ONE;
                    normal = (t * n.x + bitangent * n.y + normal * n.z).normalize_or(normal);
                }
                if det < 0.0 {
                    normal = -normal;
                    geometric_normal = -geometric_normal;
                }
                let metallic = material.metallic
                    * sample_texture(surface, material.metallic_roughness_texture, uv, false).z;
                let emissive = Vec3::from(material.emissive)
                    * sample_texture(surface, material.emissive_texture, uv, true).truncate();
                closest = distance;
                result = Some(SurfaceHit {
                    front_face: det > 0.0,
                    distance,
                    position: origin + direction * distance,
                    normal,
                    geometric_normal,
                    albedo: base.truncate(),
                    emissive,
                    metallic,
                });
            }
        }
        result
    }

    /// A probe deeply inside a closed solid sees back faces in almost every direction. These
    /// probes are disabled instead of integrating through back-face culling into the outside sky.
    /// This conservative static classification is not a general watertight-mesh containment test.
    fn inside_solid(&self, origin: Vec3, counts: &mut RayCounts) -> bool {
        let mut hits = 0;
        let mut back_faces = 0;
        for i in 0..32 {
            let z = 1.0 - 2.0 * (i as f32 + 0.5) / 32.0;
            let r = (1.0 - z * z).sqrt();
            let a = i as f32 * 2.3999631;
            let direction = Vec3::new(r * a.cos(), z, r * a.sin());
            counts.classification += 1;
            if let Some(hit) = self.intersection_mode(origin, direction, f32::INFINITY, false, true)
            {
                hits += 1;
                back_faces += u32::from(!hit.front_face);
            }
        }
        hits >= 24 && back_faces * 10 >= hits * 9
    }

    fn trace(
        &self,
        mut origin: Vec3,
        mut direction: Vec3,
        bounces: u32,
        rng: &mut Rng,
        counts: &mut RayCounts,
    ) -> Vec3 {
        let mut radiance = Vec3::ZERO;
        let mut throughput = Vec3::ONE;
        for bounce in 0..bounces {
            let Some(hit) = self.intersection(origin, direction, f32::INFINITY, false) else {
                radiance += throughput * self.environment;
                break;
            };
            radiance += throughput * (hit.emissive + self.direct(&hit, -direction, counts));
            if bounce + 1 == bounces {
                break;
            }
            let next = cosine_hemisphere(hit.normal, rng);
            // Cosine importance sampling cancels n·l / π. Match the renderer's diffuse Fresnel.
            throughput *= diffuse_weight(hit.albedo, hit.metallic, -direction, next);
            if throughput.max_element() < 1e-8 {
                break;
            }
            origin = hit.position + hit.geometric_normal * EPSILON * 4.0;
            direction = next;
            counts.secondary += 1;
        }
        radiance
    }

    fn direct(&self, hit: &SurfaceHit, view: Vec3, counts: &mut RayCounts) -> Vec3 {
        let mut radiance = Vec3::ZERO;
        for source in &self.lights {
            let light = &source.light;
            let (direction, distance, mut attenuation) = if light.kind == LightKind::Directional {
                (-source.direction, f32::INFINITY, 1.0)
            } else {
                let delta = source.position - hit.position;
                let d2 = delta.length_squared();
                if d2 <= 1e-10 {
                    continue;
                }
                let distance = d2.sqrt();
                let x = d2 / (light.range as f32).powi(2);
                let window = (1.0 - x * x).clamp(0.0, 1.0);
                (delta / distance, distance, window * window / d2.max(0.01))
            };
            if light.kind == LightKind::Spot {
                let cosine = (-direction).dot(source.direction);
                let outer = (light.outer_deg as f32).to_radians().cos();
                let inner = (light.inner_deg as f32).to_radians().cos();
                let weight = if inner <= outer + 1e-6 {
                    if cosine >= inner { 1.0 } else { 0.0 }
                } else {
                    ((cosine - outer) / (inner - outer)).clamp(0.0, 1.0)
                };
                attenuation *= weight * weight * (3.0 - 2.0 * weight);
            }
            let cosine = hit.normal.dot(direction).max(0.0);
            if cosine <= 0.0 || attenuation <= 0.0 || light.intensity <= 0.0 {
                continue;
            }
            if light.shadows {
                counts.shadow += 1;
                if self
                    .intersection(
                        hit.position + hit.geometric_normal * EPSILON * 4.0,
                        direction,
                        distance - EPSILON * 8.0,
                        true,
                    )
                    .is_some()
                {
                    continue;
                }
            }
            radiance += diffuse_weight(hit.albedo, hit.metallic, view, direction)
                * Vec3::from(light.color.map(|c| c as f32))
                * (light.intensity as f32 * attenuation * cosine / PI);
        }
        radiance
    }
}

/// Loads the static scene then bakes its regular probe grid. No GPU or window is required.
#[cfg(all(not(target_arch = "wasm32"), feature = "import"))]
pub fn bake_project(root: &Path, options: &BakeOptions) -> Result<(BakedGi, BakeStats), String> {
    options.validate()?;
    let start = Instant::now();
    let scene = RayScene::from_project(root)?;
    let load_seconds = start.elapsed().as_secs_f64();
    let (asset, mut stats) = bake_scene(&scene, options)?;
    stats.load_seconds = load_seconds;
    Ok((asset, stats))
}

pub fn bake_scene(scene: &RayScene, options: &BakeOptions) -> Result<(BakedGi, BakeStats), String> {
    let count = options.validate()?;
    let start = Instant::now();
    let mut probes = Vec::with_capacity(count);
    let mut counts = RayCounts::default();
    let mut invalid_probe_indices = Vec::new();
    for z in 0..options.dimensions[2] {
        for y in 0..options.dimensions[1] {
            for x in 0..options.dimensions[0] {
                let index = probes.len() as u64;
                let position = Vec3::from(options.origin)
                    + Vec3::from(options.spacing) * Vec3::new(x as f32, y as f32, z as f32);
                if scene.inside_solid(position, &mut counts) {
                    invalid_probe_indices.push(index as usize);
                    probes.push(BakedProbe {
                        radiance_sh: [[0.0; 3]; 9],
                        distance_moments: vec![
                            [0.0; 2];
                            options.distance_resolution.pow(2) as usize
                        ],
                    });
                    continue;
                }
                let mut rng = Rng(options.seed ^ index.wrapping_mul(0x9e3779b97f4a7c15));
                let rotation = random_rotation(&mut rng);
                let mut coefficients = [Vec3::ZERO; 9];
                for sample in 0..options.rays_per_probe {
                    let vertical =
                        1.0 - 2.0 * (sample as f32 + 0.5) / options.rays_per_probe as f32;
                    let horizontal = (1.0 - vertical * vertical).max(0.0).sqrt();
                    let angle = sample as f32 * 2.3999631;
                    let direction = rotation
                        * Vec3::new(horizontal * angle.cos(), vertical, horizontal * angle.sin());
                    counts.primary += 1;
                    let radiance =
                        scene.trace(position, direction, options.bounces, &mut rng, &mut counts);
                    let basis = sh_basis(direction);
                    for k in 0..9 {
                        coefficients[k] += radiance * basis[k];
                    }
                }
                let scale = 4.0 * PI / options.rays_per_probe as f32;
                let radiance_sh = coefficients.map(|coefficient| (coefficient * scale).to_array());
                let distance_moments =
                    bake_distance(scene, position, options, &mut rng, &mut counts);
                if radiance_sh.iter().flatten().any(|n| !n.is_finite()) {
                    return Err(format!(
                        "probe {index}: nonfinite radiance; reduce source intensity/color"
                    ));
                }
                probes.push(BakedProbe {
                    radiance_sh,
                    distance_moments,
                });
            }
        }
    }
    let mut settings_hash = Signature::new();
    settings_hash.feed(&serde_json::to_vec(options).map_err(|e| e.to_string())?);
    let scene_signature = format!("{}:{:016x}", scene.signature, settings_hash.0);
    let asset = BakedGi {
        format: BAKED_GI_FORMAT.into(),
        origin: options.origin,
        spacing: options.spacing,
        dimensions: options.dimensions,
        probes,
        distance_resolution: options.distance_resolution,
        max_distance: options.max_distance,
        rays_per_probe: options.rays_per_probe,
        bounces: options.bounces,
        scene_signature: scene_signature.clone(),
    };
    asset.validate()?;
    let primary_rays = counts.primary;
    let stats = BakeStats {
        load_seconds: 0.0,
        bake_seconds: start.elapsed().as_secs_f64(),
        probes: count,
        rays: primary_rays
            + counts.secondary
            + counts.shadow
            + counts.distance
            + counts.classification,
        secondary_rays: counts.secondary,
        shadow_rays: counts.shadow,
        distance_rays: counts.distance,
        classification_rays: counts.classification,
        radiance_rays: counts.primary,
        invalid_probe_indices,
        triangles: scene.triangles.len(),
        scene_signature,
    };
    Ok((asset, stats))
}

fn bake_distance(
    scene: &RayScene,
    position: Vec3,
    options: &BakeOptions,
    rng: &mut Rng,
    counts: &mut RayCounts,
) -> Vec<[f32; 2]> {
    let resolution = options.distance_resolution;
    let mut moments = Vec::with_capacity((resolution * resolution) as usize);
    for y in 0..resolution {
        for x in 0..resolution {
            let mut mean = 0.0;
            let mut square = 0.0;
            for sample in 0..DISTANCE_SAMPLES {
                // Stratify within each octahedron texel; no empty visibility bins at low ray counts.
                let uv = Vec2::new(
                    (x as f32 + (sample % 4) as f32 / 4.0 + rng.next() / 4.0) / resolution as f32,
                    (y as f32 + (sample / 4) as f32 / 2.0 + rng.next() / 2.0) / resolution as f32,
                );
                let direction = oct_decode(uv);
                let distance = scene
                    .intersection(position, direction, options.max_distance, false)
                    .map_or(options.max_distance, |hit| hit.distance);
                mean += distance;
                square += distance * distance;
                counts.distance += 1;
            }
            moments.push([
                mean / DISTANCE_SAMPLES as f32,
                square / DISTANCE_SAMPLES as f32,
            ]);
        }
    }
    moments
}

/// Real orthonormal SH basis: [1, y, z, x, xy, yz, 3z²−1, xz, x²−y²].
pub fn sh_basis(n: Vec3) -> [f32; 9] {
    [
        0.2820948,
        0.48860252 * n.y,
        0.48860252 * n.z,
        0.48860252 * n.x,
        1.0925485 * n.x * n.y,
        1.0925485 * n.y * n.z,
        0.31539157 * (3.0 * n.z * n.z - 1.0),
        1.0925485 * n.x * n.z,
        0.54627424 * (n.x * n.x - n.y * n.y),
    ]
}

fn oct_decode(uv: Vec2) -> Vec3 {
    let uv = uv * 2.0 - Vec2::ONE;
    let mut direction = Vec3::new(uv.x, uv.y, 1.0 - uv.x.abs() - uv.y.abs());
    if direction.z < 0.0 {
        direction.x = (1.0 - uv.y.abs()) * if uv.x >= 0.0 { 1.0 } else { -1.0 };
        direction.y = (1.0 - uv.x.abs()) * if uv.y >= 0.0 { 1.0 } else { -1.0 };
    }
    direction.normalize()
}

fn diffuse_weight(albedo: Vec3, metallic: f32, view: Vec3, light: Vec3) -> Vec3 {
    let half = (view + light).normalize_or_zero();
    let f0 = Vec3::splat(0.04).lerp(albedo, metallic);
    let fresnel = f0 + (Vec3::ONE - f0) * (1.0 - view.dot(half).max(0.0)).powi(5);
    (Vec3::ONE - fresnel) * albedo * (1.0 - metallic)
}

fn cosine_hemisphere(normal: Vec3, rng: &mut Rng) -> Vec3 {
    let radius = rng.next().sqrt();
    let angle = TAU * rng.next();
    let up = if normal.z.abs() < 0.999 {
        Vec3::Z
    } else {
        Vec3::X
    };
    let tangent = up.cross(normal).normalize();
    let bitangent = normal.cross(tangent);
    (tangent * (radius * angle.cos())
        + bitangent * (radius * angle.sin())
        + normal * (1.0 - radius * radius).max(0.0).sqrt())
    .normalize()
}

fn random_rotation(rng: &mut Rng) -> Quat {
    let u = rng.next();
    let a = TAU * rng.next();
    let b = TAU * rng.next();
    Quat::from_xyzw(
        (1.0 - u).sqrt() * a.sin(),
        (1.0 - u).sqrt() * a.cos(),
        u.sqrt() * b.sin(),
        u.sqrt() * b.cos(),
    )
}

fn bounds_hit(origin: Vec3, direction: Vec3, min: Vec3, max: Vec3, limit: f32) -> bool {
    let mut near: f32 = 0.0;
    let mut far = limit;
    for axis in 0..3 {
        if direction[axis].abs() < 1e-12 {
            if origin[axis] < min[axis] || origin[axis] > max[axis] {
                return false;
            }
        } else {
            let a = (min[axis] - origin[axis]) / direction[axis];
            let b = (max[axis] - origin[axis]) / direction[axis];
            near = near.max(a.min(b));
            far = far.min(a.max(b));
            if near > far {
                return false;
            }
        }
    }
    far > EPSILON
}

fn sample_texture(surface: &Surface, texture: Option<usize>, uv: Vec2, srgb: bool) -> Vec4 {
    let Some(image) = texture.and_then(|i| surface.images.get(i)) else {
        return Vec4::ONE;
    };
    // Match renderer's repeat/bilinear sampling. CPU operates at mip 0 for offline detail.
    let x = uv.x.rem_euclid(1.0) * image.width as f32 - 0.5;
    let y = uv.y.rem_euclid(1.0) * image.height as f32 - 0.5;
    let tx = x - x.floor();
    let ty = y - y.floor();
    let pixel = |x: i32, y: i32| {
        let x = x.rem_euclid(image.width as i32) as usize;
        let y = y.rem_euclid(image.height as i32) as usize;
        let i = (y * image.width as usize + x) * 4;
        let mut value = Vec4::new(
            image.rgba8[i] as f32,
            image.rgba8[i + 1] as f32,
            image.rgba8[i + 2] as f32,
            image.rgba8[i + 3] as f32,
        ) / 255.0;
        if srgb {
            for k in 0..3 {
                value[k] = if value[k] <= 0.04045 {
                    value[k] / 12.92
                } else {
                    ((value[k] + 0.055) / 1.055).powf(2.4)
                };
            }
        }
        value
    };
    pixel(x.floor() as i32, y.floor() as i32)
        .lerp(pixel(x.floor() as i32 + 1, y.floor() as i32), tx)
        .lerp(
            pixel(x.floor() as i32, y.floor() as i32 + 1)
                .lerp(pixel(x.floor() as i32 + 1, y.floor() as i32 + 1), tx),
            ty,
        )
}

#[cfg(all(not(target_arch = "wasm32"), feature = "import"))]
fn load_asset(
    root: &Path,
    path: &str,
    cache: &mut BTreeMap<String, Arc<ModelAsset>>,
) -> Result<Arc<ModelAsset>, String> {
    if let Some(asset) = cache.get(path) {
        return Ok(asset.clone());
    }
    if path.is_empty()
        || Path::new(path)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("asset path must be a project-relative path without ..".into());
    }
    let asset =
        Arc::new(pocket_assets::import::import_gltf(&root.join(path)).map_err(|e| e.message)?);
    cache.insert(path.to_owned(), asset.clone());
    Ok(asset)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "import"))]
fn resolve_surface(
    root: &Path,
    model: &Model,
    source: Option<(&str, &MeshData, &ModelAsset)>,
    cache: &mut BTreeMap<String, Arc<ModelAsset>>,
    image_cache: &mut BTreeMap<String, Arc<Vec<ImageData>>>,
    path_tracing: bool,
) -> Result<Surface, String> {
    let mut material = MaterialData::default();
    let mut images = Arc::new(Vec::new());
    let imported = if !model.material.is_empty() {
        let (path, selector) = split(&model.material);
        let selector = selector.ok_or("material path requires #name or #index")?;
        let asset = load_asset(root, path, cache)?;
        let mi = asset
            .materials
            .iter()
            .position(|m| m.name == selector)
            .or_else(|| selector.parse::<usize>().ok())
            .filter(|&i| i < asset.materials.len())
            .ok_or_else(|| format!("{path}: unknown material #{selector}"))?;
        material = asset.materials[mi].clone();
        images = image_cache
            .entry(path.to_owned())
            .or_insert_with(|| Arc::new(asset.images.clone()))
            .clone();
        true
    } else if let Some((path, mesh, asset)) = source
        && let Some(mi) = mesh.material
    {
        material = asset
            .materials
            .get(mi)
            .ok_or("mesh material index out of bounds")?
            .clone();
        images = image_cache
            .entry(path.to_owned())
            .or_insert_with(|| Arc::new(asset.images.clone()))
            .clone();
        true
    } else {
        false
    };
    if !path_tracing
        && (material.alpha_mode == AlphaMode::Blend
            || material.transmission > 0.0
            || model.transmission.is_some_and(|t| t > 0.0))
    {
        return Err(format!(
            "material {}: alpha blending/transmission is not supported by diffuse baking",
            material.name
        ));
    }
    validate_numbers(
        &material.base_color.map(f64::from),
        "material base color",
        0.0,
        1e6,
    )?;
    validate_numbers(
        &material.emissive.map(f64::from),
        "material emissive",
        0.0,
        1e6,
    )?;
    validate_numbers(
        &[material.metallic as f64, material.roughness as f64],
        "material metallic/roughness",
        0.0,
        1.0,
    )?;
    for texture in [
        material.base_color_texture,
        material.metallic_roughness_texture,
        material.normal_texture,
        material.emissive_texture,
        material.occlusion_texture,
        material.transmission_texture,
    ]
    .into_iter()
    .flatten()
    {
        let image = images
            .get(texture)
            .ok_or("material texture index out of bounds")?;
        let bytes = u64::from(image.width) * u64::from(image.height) * 4;
        if image.width == 0
            || image.height == 0
            || image.width > 65536
            || image.height > 65536
            || bytes != image.rgba8.len() as u64
        {
            return Err("material texture has invalid dimensions/pixel length".into());
        }
    }
    for k in 0..4 {
        material.base_color[k] *= model.color[k] as f32;
    }
    if imported {
        for k in 0..3 {
            material.emissive[k] += model.emissive[k] as f32;
        }
    } else {
        material.metallic = model.metallic as f32;
        material.roughness = model.roughness as f32;
        material.emissive = model.emissive.map(|e| e as f32);
    }
    if let Some(transmission) = model.transmission {
        validate_numbers(&[transmission], "model transmission", 0.0, 1.0)?;
        material.transmission = transmission as f32;
    }
    if let Some(ior) = model.ior {
        validate_numbers(&[ior], "model ior", 1.0, 3.0)?;
        material.ior = ior as f32;
    }
    validate_numbers(
        &[material.transmission as f64],
        "material transmission",
        0.0,
        1.0,
    )?;
    validate_numbers(&[material.ior as f64], "material ior", 1.0, 3.0)?;
    Ok(Surface {
        material,
        images,
        casts_shadows: model.cast_shadows,
    })
}

fn split(path: &str) -> (&str, Option<&str>) {
    path.split_once('#')
        .map_or((path, None), |(p, s)| (p, Some(s)))
}
fn matrix(pose: (Vec3, Quat, Vec3)) -> Mat4 {
    Mat4::from_scale_rotation_translation(pose.2, pose.1, pose.0)
}
fn vector<const N: usize>(value: Option<&Value>, default: [f32; N]) -> Result<[f32; N], String> {
    let Some(value) = value else {
        return Ok(default);
    };
    let array = value
        .as_array()
        .filter(|a| a.len() == N)
        .ok_or("invalid transform vector length")?;
    let mut output = default;
    for (out, value) in output.iter_mut().zip(array) {
        let number = value
            .as_f64()
            .filter(|n| n.is_finite() && n.abs() <= 1e6)
            .ok_or("transform must have finite numbers within ±1e6")?;
        *out = number as f32;
    }
    Ok(output)
}
fn component<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T, String> {
    serde_json::from_value(value.clone()).map_err(|e| e.to_string())
}
fn validate_numbers(values: &[f64], name: &str, min: f64, max: f64) -> Result<(), String> {
    if !values
        .iter()
        .all(|v| v.is_finite() && *v >= min && *v <= max)
    {
        return Err(format!("{name}: values must be finite in [{min}, {max}]"));
    }
    Ok(())
}

/// SplitMix64, entirely local to an offline bake (no engine simulation state is touched).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut n = self.0;
        n = (n ^ (n >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        n = (n ^ (n >> 27)).wrapping_mul(0x94d049bb133111eb);
        n ^= n >> 31;
        (n >> 40) as f32 / 16777216.0
    }
}
struct Signature(u64);
impl Signature {
    fn new() -> Self {
        Self(0xcbf29ce484222325)
    }
    fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty(environment: Vec3) -> RayScene {
        RayScene {
            triangles: Vec::new(),
            surfaces: Vec::new(),
            nodes: Vec::new(),
            lights: Vec::new(),
            environment,
            environment_settings: Environment {
                sky: SkyKind::Color,
                sky_color: environment.to_array().map(f64::from),
                ..Environment::default()
            },
            signature: "test".into(),
        }
    }
    fn surface(albedo: Vec3, emissive: Vec3, double_sided: bool) -> Surface {
        Surface {
            material: MaterialData {
                base_color: albedo.extend(1.0).to_array(),
                emissive: emissive.to_array(),
                double_sided,
                ..MaterialData::default()
            },
            images: Arc::new(Vec::new()),
            casts_shadows: true,
        }
    }
    fn opts() -> BakeOptions {
        BakeOptions {
            origin: [0.0; 3],
            dimensions: [1; 3],
            rays_per_probe: 4096,
            distance_resolution: 4,
            ..BakeOptions::default()
        }
    }

    #[test]
    fn constant_sky_sh_has_correct_radiance_normalization_and_seed_reproducibility() {
        let sky = Vec3::new(0.3, 0.5, 0.8);
        let scene = empty(sky);
        let (a, stats) = bake_scene(&scene, &opts()).unwrap();
        assert_eq!(stats.radiance_rays, u64::from(opts().rays_per_probe));
        let (b, _) = bake_scene(&scene, &opts()).unwrap();
        assert_eq!(
            serde_json::to_vec(&a).unwrap(),
            serde_json::to_vec(&b).unwrap()
        );
        let coefficients = a.probes[0].radiance_sh;
        for direction in [Vec3::X, Vec3::Y, Vec3::Z, -Vec3::Y] {
            let basis = sh_basis(direction);
            let mut irradiance_over_pi = Vec3::ZERO;
            for i in 0..9 {
                let band = if i == 0 {
                    1.0
                } else if i <= 3 {
                    2.0 / 3.0
                } else {
                    0.25
                };
                irradiance_over_pi += Vec3::from(coefficients[i]) * basis[i] * band;
            }
            assert!((irradiance_over_pi - sky).abs().max_element() < 5e-4);
        }
        assert!(
            a.probes[0]
                .distance_moments
                .iter()
                .all(|m| *m == [100.0, 10000.0])
        );
    }

    #[test]
    fn bvh_nearest_hit_respects_node_transform_and_front_faces() {
        let mut scene = empty(Vec3::ZERO);
        scene
            .add_mesh(
                &pocket_assets::primitives::cube(),
                Mat4::from_scale_rotation_translation(
                    Vec3::new(2.0, 4.0, 2.0),
                    Quat::IDENTITY,
                    Vec3::new(3.0, 0.0, 0.0),
                ),
                surface(Vec3::ONE, Vec3::ZERO, false),
            )
            .unwrap();
        scene.build_node(0, scene.triangles.len());
        let hit = scene
            .intersect(Vec3::new(3.0, 0.0, 5.0), -Vec3::Z, 100.0)
            .unwrap();
        assert!((hit.distance - 4.0).abs() < 1e-6);
        assert_eq!(hit.geometric_normal, Vec3::Z);
        assert!(
            scene
                .intersect(Vec3::new(0.0, 0.0, 5.0), -Vec3::Z, 100.0)
                .is_none()
        );
        assert!(
            scene
                .intersect(Vec3::new(3.0, 0.0, 0.0), Vec3::Z, 100.0)
                .is_none()
        );
    }

    #[test]
    fn reflected_directional_light_has_shadow_visibility_and_color_bleeding() {
        let mut scene = empty(Vec3::ZERO);
        scene
            .add_mesh(
                &pocket_assets::primitives::plane(),
                Mat4::from_scale(Vec3::splat(10.0)),
                surface(Vec3::new(1.0, 0.1, 0.05), Vec3::ZERO, false),
            )
            .unwrap();
        scene.lights.push(SceneLight {
            light: Light {
                color: [1.0; 3],
                intensity: PI as f64,
                ..Light::default()
            },
            position: Vec3::ZERO,
            direction: -Vec3::Y,
        });
        scene.build_node(0, scene.triangles.len());
        let lit = scene
            .trace_radiance(Vec3::new(0.0, 2.0, 0.0), -Vec3::Y, 1, 1)
            .unwrap();
        assert!(lit.x > 0.9 && lit.y < 0.11 && lit.z < 0.06);
        scene.nodes.clear();
        scene
            .add_mesh(
                &pocket_assets::primitives::cube(),
                Mat4::from_scale_rotation_translation(
                    Vec3::ONE,
                    Quat::IDENTITY,
                    Vec3::new(0.0, 1.0, 0.0),
                ),
                surface(Vec3::ZERO, Vec3::ZERO, true),
            )
            .unwrap();
        scene.build_node(0, scene.triangles.len());
        let shadowed = scene.direct(
            &SurfaceHit {
                front_face: true,
                distance: 0.0,
                position: Vec3::ZERO,
                normal: Vec3::Y,
                geometric_normal: Vec3::Y,
                albedo: Vec3::ONE,
                emissive: Vec3::ZERO,
                metallic: 0.0,
            },
            Vec3::Y,
            &mut RayCounts::default(),
        );
        assert_eq!(shadowed, Vec3::ZERO);
    }

    #[test]
    fn two_bounces_recover_environment_seen_after_diffuse_surface() {
        let mut scene = empty(Vec3::splat(0.5));
        scene
            .add_mesh(
                &pocket_assets::primitives::plane(),
                Mat4::from_scale(Vec3::splat(10.0)),
                surface(Vec3::splat(0.8), Vec3::ZERO, false),
            )
            .unwrap();
        scene.build_node(0, scene.triangles.len());
        let one = scene.trace_radiance(Vec3::Y, -Vec3::Y, 1, 3).unwrap();
        let two = scene.trace_radiance(Vec3::Y, -Vec3::Y, 2, 3).unwrap();
        assert_eq!(one, Vec3::ZERO);
        assert!(two.x > 0.35 && two.x < 0.4, "{two:?}");
    }

    #[test]
    fn sky_primary_has_no_punctual_direct_component_and_emission_is_recovered() {
        let mut scene = empty(Vec3::ZERO);
        scene.lights.push(SceneLight {
            light: Light::default(),
            position: Vec3::ZERO,
            direction: -Vec3::Y,
        });
        assert_eq!(
            scene.trace_radiance(Vec3::ZERO, Vec3::Y, 2, 1).unwrap(),
            Vec3::ZERO
        );
        scene
            .add_mesh(
                &pocket_assets::primitives::plane(),
                Mat4::IDENTITY,
                surface(Vec3::ZERO, Vec3::new(2.0, 0.5, 0.0), true),
            )
            .unwrap();
        scene.build_node(0, scene.triangles.len());
        assert_eq!(
            scene.trace_radiance(Vec3::Y, -Vec3::Y, 1, 1).unwrap(),
            Vec3::new(2.0, 0.5, 0.0)
        );
    }

    #[test]
    fn texture_sampling_decodes_srgb_before_bilinear_and_repeats() {
        let mut surface = surface(Vec3::ONE, Vec3::ZERO, false);
        surface.images = Arc::new(vec![ImageData {
            width: 2,
            height: 1,
            rgba8: vec![0, 0, 0, 255, 255, 255, 255, 255],
            ..ImageData::default()
        }]);
        let mid = sample_texture(&surface, Some(0), Vec2::new(0.5, 0.5), true);
        assert!((mid.x - 0.5).abs() < 1e-6);
        assert_eq!(
            mid,
            sample_texture(&surface, Some(0), Vec2::new(1.5, -0.5), true)
        );
    }

    #[test]
    fn probes_inside_closed_solid_are_reported_and_disabled() {
        let mut scene = empty(Vec3::ONE);
        scene
            .add_mesh(
                &pocket_assets::primitives::cube(),
                Mat4::IDENTITY,
                surface(Vec3::ONE, Vec3::ZERO, false),
            )
            .unwrap();
        scene.build_node(0, scene.triangles.len());
        let (asset, stats) = bake_scene(&scene, &opts()).unwrap();
        assert_eq!(stats.invalid_probe_indices, vec![0]);
        assert_eq!(stats.radiance_rays, 0);
        assert_eq!(asset.probes[0].radiance_sh, [[0.0; 3]; 9]);
        assert!(
            asset.probes[0]
                .distance_moments
                .iter()
                .all(|m| *m == [0.0; 2])
        );
        let mut outside = opts();
        outside.origin = [0.0, 2.0, 0.0];
        let (_, stats) = bake_scene(&scene, &outside).unwrap();
        assert!(stats.invalid_probe_indices.is_empty());
    }

    #[test]
    fn point_falloff_and_spot_cone_match_forward_light_conventions() {
        let mut scene = empty(Vec3::ZERO);
        let hit = SurfaceHit {
            front_face: true,
            distance: 0.0,
            position: Vec3::ZERO,
            normal: Vec3::Y,
            geometric_normal: Vec3::Y,
            albedo: Vec3::ONE,
            emissive: Vec3::ZERO,
            metallic: 0.0,
        };
        scene.lights.push(SceneLight {
            light: Light {
                kind: LightKind::Point,
                color: [1.0; 3],
                intensity: PI as f64,
                range: 100.0,
                shadows: false,
                ..Light::default()
            },
            position: Vec3::Y,
            direction: -Vec3::Y,
        });
        let near = scene.direct(&hit, Vec3::Y, &mut RayCounts::default());
        scene.lights[0].position = Vec3::Y * 2.0;
        let far = scene.direct(&hit, Vec3::Y, &mut RayCounts::default());
        assert!((near.x / far.x - 4.0).abs() < 1e-4);
        scene.lights[0].light.kind = LightKind::Spot;
        let aligned = scene.direct(&hit, Vec3::Y, &mut RayCounts::default());
        assert_eq!(aligned, far);
        scene.lights[0].direction = Vec3::X;
        assert_eq!(
            scene.direct(&hit, Vec3::Y, &mut RayCounts::default()),
            Vec3::ZERO
        );
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "import"))]
    #[test]
    fn project_loader_imports_default_nodes_and_mesh_selector_with_materials() {
        use serde_json::json;
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        struct Temp(std::path::PathBuf);
        impl Drop for Temp {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let temp = Temp(std::env::temp_dir().join(format!(
            "amoris-gi-baker-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )));
        std::fs::create_dir_all(&temp.0).unwrap();
        let positions: [f32; 9] = [-0.5, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 1.0, 0.0];
        let normals: [f32; 9] = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0];
        let mut binary = Vec::new();
        for value in positions.into_iter().chain(normals) {
            binary.extend_from_slice(&value.to_le_bytes());
        }
        std::fs::write(temp.0.join("triangle.bin"), binary).unwrap();
        let gltf = json!({"asset":{"version":"2.0"}, "scene":0,
            "scenes":[{"nodes":[0,1]}], "nodes":[
                {"mesh":0,"translation":[2,0,0]}, {"mesh":0,"translation":[5,0,0]}],
            "buffers":[{"uri":"triangle.bin","byteLength":72}],
            "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":36},
                {"buffer":0,"byteOffset":36,"byteLength":36}],
            "accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3",
                "min":[-0.5,0,0],"max":[0.5,1,0]},
                {"bufferView":1,"componentType":5126,"count":3,"type":"VEC3"}],
            "materials":[{"name":"Paint","pbrMetallicRoughness":{
                "baseColorFactor":[0.2,0.4,0.6,1],"metallicFactor":0},"emissiveFactor":[0.1,0,0]}],
            "meshes":[{"name":"Triangle","primitives":[{"attributes":{"POSITION":0,"NORMAL":1},"material":0}]}]});
        std::fs::write(
            temp.0.join("triangle.gltf"),
            serde_json::to_vec(&gltf).unwrap(),
        )
        .unwrap();
        let mut project = json!({"format":"pocket-scene","version":1,"entities":[
            {"components":{"Environment":{"sky":"color","sky_color":[0,0,0]}}},
            {"components":{"Transform":{"position":[1,2,3]},
                "Model":{"mesh":"triangle.gltf","scale":[2,1,1],"color":[0.5,1,1,1],"emissive":[0.02,0,0]}}}]});
        std::fs::write(
            temp.0.join("scene.json"),
            serde_json::to_vec(&project).unwrap(),
        )
        .unwrap();
        let default = RayScene::from_project(&temp.0).unwrap();
        assert_eq!(default.triangle_count(), 2);
        let hit = default
            .intersect(Vec3::new(5.0, 2.2, 5.0), -Vec3::Z, 10.0)
            .unwrap();
        assert!((hit.distance - 2.0).abs() < 1e-6);
        assert!((hit.albedo - Vec3::new(0.1, 0.4, 0.6)).abs().max_element() < 1e-6);
        assert!((hit.emissive.x - 0.12).abs() < 1e-6);
        assert!(
            default
                .intersect(Vec3::new(11.0, 2.2, 5.0), -Vec3::Z, 10.0)
                .is_some()
        );
        project["entities"][1]["components"]["Model"]["mesh"] = json!("triangle.gltf#Triangle");
        std::fs::write(
            temp.0.join("scene.json"),
            serde_json::to_vec(&project).unwrap(),
        )
        .unwrap();
        let selected = RayScene::from_project(&temp.0).unwrap();
        assert_eq!(selected.triangle_count(), 1);
        assert!(
            selected
                .intersect(Vec3::new(1.0, 2.2, 5.0), -Vec3::Z, 10.0)
                .is_some()
        );
        assert_ne!(selected.scene_signature(), default.scene_signature());
        project["entities"][0]["components"]["Environment"]["sky"] = json!("atmosphere");
        std::fs::write(
            temp.0.join("scene.json"),
            serde_json::to_vec(&project).unwrap(),
        )
        .unwrap();
        assert!(
            RayScene::from_project(&temp.0)
                .err()
                .unwrap()
                .contains("sky: color")
        );
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "import"))]
    #[test]
    fn path_tracing_accepts_atmosphere_and_glass_without_weakening_baker() {
        use serde_json::json;
        struct Temp(std::path::PathBuf);
        impl Drop for Temp {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let temp = Temp(
            std::env::temp_dir().join(format!("amoris-pt-loader-test-{}", std::process::id())),
        );
        std::fs::create_dir_all(&temp.0).unwrap();
        let positions: [f32; 9] = [-0.5, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 1.0, 0.0];
        std::fs::write(
            temp.0.join("tri.bin"),
            positions
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let gltf = json!({"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0}],
            "extensionsUsed":["KHR_materials_transmission","KHR_materials_ior"],
            "buffers":[{"uri":"tri.bin","byteLength":36}],"bufferViews":[{"buffer":0,"byteLength":36}],
            "accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[-0.5,0,0],"max":[0.5,1,0]}],
            "materials":[{"pbrMetallicRoughness":{"metallicFactor":0},"extensions":{"KHR_materials_transmission":{"transmissionFactor":0.85},"KHR_materials_ior":{"ior":1.33}}}],
            "meshes":[{"primitives":[{"attributes":{"POSITION":0},"material":0}]}]});
        std::fs::write(
            temp.0.join("glass.gltf"),
            serde_json::to_vec(&gltf).unwrap(),
        )
        .unwrap();
        let mut project = json!({"format":"pocket-scene","version":1,"entities":[
            {"components":{"Environment":{"sky":"atmosphere","ambient":0.7}}},
            {"components":{"Model":{"mesh":"glass.gltf"}}}]});
        std::fs::write(
            temp.0.join("scene.json"),
            serde_json::to_vec(&project).unwrap(),
        )
        .unwrap();
        let loaded = RayScene::for_path_tracing(&temp.0).unwrap();
        let material = loaded.world_materials().next().unwrap().material;
        assert_eq!(material.transmission, 0.85);
        assert_eq!(material.ior, 1.33);
        assert_eq!(loaded.world_environment().sky, SkyKind::Atmosphere);
        assert_eq!(loaded.world_environment().ambient, 0.7);
        assert!(RayScene::from_project(&temp.0).is_err());
        project["entities"][0]["components"]["Environment"]["sky"] = json!("color");
        project["entities"][1]["components"]["Model"]["transmission"] = json!(0.5);
        project["entities"][1]["components"]["Model"]["ior"] = json!(1.7);
        std::fs::write(
            temp.0.join("scene.json"),
            serde_json::to_vec(&project).unwrap(),
        )
        .unwrap();
        let overridden = RayScene::for_path_tracing(&temp.0).unwrap();
        let material = overridden.world_materials().next().unwrap().material;
        assert_eq!(material.transmission, 0.5);
        assert_eq!(material.ior, 1.7);
        assert!(
            RayScene::from_project(&temp.0)
                .err()
                .unwrap()
                .contains("diffuse baking")
        );
        assert!(RayScene::for_path_tracing_file(&temp.0, "../scene.json").is_err());
    }

    #[test]
    fn invalid_or_excessive_grids_fail_without_allocating() {
        let mut options = opts();
        options.origin[0] = f32::NAN;
        assert!(options.validate().is_err());
        options = opts();
        options.spacing[1] = 0.0;
        assert!(options.validate().is_err());
        options = opts();
        options.dimensions = [u32::MAX; 3];
        assert!(options.validate().is_err());
        options = opts();
        options.bounces = 0;
        assert!(options.validate().is_err());
        options = opts();
        options.max_distance = f32::INFINITY;
        assert!(options.validate().is_err());
    }
}
