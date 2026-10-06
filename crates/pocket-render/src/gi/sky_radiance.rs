//! A CPU-readable export of the exact renderer sky. Values are incoming linear radiance;
//! the raw atmosphere excludes the visible sun disk, which is handled by directional lighting.

use std::path::Path;
use std::time::Duration;

use glam::{Vec2, Vec3};
use pocket_assets::visual::{LightKind, SkyKind};
use serde::{Deserialize, Serialize};

use crate::Gpu;
use crate::sky::{Sky, SkyParams};

use super::bake::RayScene;

pub const SKY_RADIANCE_FORMAT: &str = "amoris-sky-radiance-v1";
pub const MAX_SKY_JSON_BYTES: usize = 32 * 1024 * 1024;
const MAX_FACE_SIZE: u32 = 256;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkyRadianceSource {
    pub sun: [f32; 4],
    pub color: [f32; 4],
    pub flat_color: [f32; 4],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkyRadianceCube {
    pub format: String,
    pub size: u32,
    /// +X, -X, +Y, -Y, +Z, -Z, each face in row-major order, at mip zero.
    pub texels: Vec<[f32; 3]>,
    /// Applied exactly once during CPU sampling, matching the forward pass's sky intensity.
    pub ambient: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SkyRadianceSource>,
}

impl SkyRadianceCube {
    pub fn validate(&self) -> Result<(), String> {
        if self.format != SKY_RADIANCE_FORMAT {
            return Err(format!("expected {SKY_RADIANCE_FORMAT}"));
        }
        if !(1..=MAX_FACE_SIZE).contains(&self.size) {
            return Err(format!("sky face size must be in 1..={MAX_FACE_SIZE}"));
        }
        if self.texels.len() != 6 * (self.size as usize).pow(2) {
            return Err("sky cube needs exactly six square faces".into());
        }
        if !self.ambient.is_finite() || !(0.0..=2.0).contains(&self.ambient) {
            return Err("sky ambient must be finite in [0, 2]".into());
        }
        if self
            .texels
            .iter()
            .flatten()
            .any(|n| !n.is_finite() || !(0.0..=65504.0).contains(n))
        {
            return Err("sky texels must be finite nonnegative rgba16float radiance".into());
        }
        if let Some(source) = &self.source {
            if source
                .sun
                .iter()
                .chain(source.color.iter())
                .chain(source.flat_color.iter())
                .any(|n| !n.is_finite() || n.abs() > 1e6)
                || source.sun[3] < 0.0
                || source.color.iter().any(|n| *n < 0.0)
                || source.flat_color.iter().any(|n| *n < 0.0)
            {
                return Err("invalid sky source parameters".into());
            }
        }
        Ok(())
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_SKY_JSON_BYTES {
            return Err("sky JSON exceeds 32 MiB".into());
        }
        let cube: Self = serde_json::from_slice(bytes).map_err(|e| format!("sky JSON: {e}"))?;
        cube.validate()?;
        Ok(cube)
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        // The cap applies before allocation even for a file changing while it is read.
        use std::io::Read;
        let mut bytes = Vec::new();
        file.take((MAX_SKY_JSON_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_json(&bytes)
    }

    /// Bilinear filtering in the renderer's cubemap face convention (`sky_bake::face_dir`).
    /// Call validate() before sampling a constructed cube; loaded/exported cubes are validated.
    pub fn sample(&self, direction: Vec3) -> Vec3 {
        if !direction.is_finite() || direction.length_squared() < 1e-20 {
            return Vec3::ZERO;
        }
        let (face, uv) = face_uv(direction);
        let p = (uv * self.size as f32 - Vec2::splat(0.5))
            .clamp(Vec2::ZERO, Vec2::splat((self.size - 1) as f32));
        let x0 = p.x.floor() as u32;
        let y0 = p.y.floor() as u32;
        let x1 = (x0 + 1).min(self.size - 1);
        let y1 = (y0 + 1).min(self.size - 1);
        let at =
            |x, y| Vec3::from_array(self.texels[((face * self.size + y) * self.size + x) as usize]);
        at(x0, y0).lerp(at(x1, y0), p.x - x0 as f32).lerp(
            at(x0, y1).lerp(at(x1, y1), p.x - x0 as f32),
            p.y - y0 as f32,
        ) * self.ambient
    }

    /// Render and read the same raw sky texture as the native/WebGPU forward renderer.
    /// The ambient multiplier is stored separately; the texel data is not scaled here.
    pub fn export_scene(gpu: &Gpu, scene: &RayScene) -> Result<Self, String> {
        let device = &gpu.device;
        let queue = &gpu.queue;
        let env = scene.world_environment();
        let sun = scene
            .world_lights()
            .find(|l| l.light.kind == LightKind::Directional);
        let direction = sun
            .as_ref()
            .map_or(Vec3::new(0.3, 0.8, 0.4).normalize(), |l| {
                -Vec3::from_array(l.direction).normalize_or(Vec3::NEG_Y)
            });
        let color = sun
            .as_ref()
            .map_or([1.0, 0.96, 0.9], |l| l.light.color.map(|v| v as f32));
        let source = SkyRadianceSource {
            sun: [
                direction.x,
                direction.y,
                direction.z,
                sun.as_ref().map_or(6.0, |l| l.light.intensity as f32),
            ],
            color: [
                color[0],
                color[1],
                color[2],
                f32::from(env.sky == SkyKind::Color),
            ],
            flat_color: [
                env.sky_color[0] as f32,
                env.sky_color[1] as f32,
                env.sky_color[2] as f32,
                0.0,
            ],
        };
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut sky = Sky::new(device);
        sky.update(
            device,
            queue,
            SkyParams {
                sun: source.sun,
                color: source.color,
                flat_color: source.flat_color,
                size: [0; 4],
            },
        );
        let size = sky.raw.size().width;
        if sky.raw.size().height != size || sky.raw.size().depth_or_array_layers != 6 {
            return Err("renderer sky is not a six-face square cube".into());
        }
        let view = sky.raw.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            base_mip_level: 0,
            mip_level_count: Some(1),
            ..Default::default()
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sky radiance read"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../shaders/sky_radiance_read.wgsl").into(),
            ),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("sky radiance read"),
            layout: None,
            module: &module,
            entry_point: Some("read_cube"),
            compilation_options: crate::shaders::compute_options(),
            cache: None,
        });
        let byte_size = 6 * u64::from(size).pow(2) * 16;
        let storage = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sky f32 radiance"),
            size: byte_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sky radiance readback"),
            size: byte_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sky radiance read"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: storage.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("sky radiance read"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bindings, &[]);
            pass.dispatch_workgroups(size.div_ceil(8), size.div_ceil(8), 6);
        }
        encoder.copy_buffer_to_buffer(&storage, 0, &readback, 0, byte_size);
        let submission = queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        readback.map_async(wgpu::MapMode::Read, .., move |result| {
            let _ = tx.send(result);
        });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(Duration::from_secs(60)),
            })
            .map_err(|e| format!("sky export GPU wait: {e}"))?;
        if let Some(error) = pollster::block_on(validation.pop()) {
            return Err(format!("sky export GPU validation: {error}"));
        }
        rx.recv_timeout(Duration::from_secs(30))
            .map_err(|e| format!("sky export readback: {e}"))?
            .map_err(|e| format!("sky export map: {e}"))?;
        let mapped = readback.get_mapped_range(..).map_err(|e| e.to_string())?;
        let pixels: &[[f32; 4]] = bytemuck::cast_slice(&mapped);
        let texels = pixels
            .iter()
            .map(|pixel| [pixel[0], pixel[1], pixel[2]])
            .collect();
        drop(mapped);
        readback.unmap();
        let cube = Self {
            format: SKY_RADIANCE_FORMAT.into(),
            size,
            texels,
            ambient: env.ambient as f32,
            source: Some(source),
        };
        cube.validate()?;
        Ok(cube)
    }
}

fn face_uv(d: Vec3) -> (u32, Vec2) {
    let a = d.abs();
    let (face, uv) = if a.x >= a.y && a.x >= a.z {
        if d.x >= 0.0 {
            (0, Vec2::new(-d.z, -d.y) / a.x)
        } else {
            (1, Vec2::new(d.z, -d.y) / a.x)
        }
    } else if a.y >= a.z {
        if d.y >= 0.0 {
            (2, Vec2::new(d.x, d.z) / a.y)
        } else {
            (3, Vec2::new(d.x, -d.z) / a.y)
        }
    } else if d.z >= 0.0 {
        (4, Vec2::new(d.x, -d.y) / a.z)
    } else {
        (5, Vec2::new(-d.x, -d.y) / a.z)
    };
    (face, (uv + Vec2::ONE) * 0.5)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn constant(color: [f32; 3]) -> SkyRadianceCube {
        SkyRadianceCube {
            format: SKY_RADIANCE_FORMAT.into(),
            size: 2,
            texels: vec![color; 24],
            ambient: 1.1,
            source: None,
        }
    }

    #[test]
    fn constant_cube_preserves_radiance_and_scales_ambient_once() {
        let cube = constant([2.0, 3.0, 4.0]);
        cube.validate().unwrap();
        for d in [
            Vec3::X,
            -Vec3::X,
            Vec3::Y,
            -Vec3::Y,
            Vec3::Z,
            -Vec3::Z,
            Vec3::new(-3.0, 2.0, 1.0),
        ] {
            assert!((cube.sample(d) - Vec3::new(2.2, 3.3, 4.4)).length() < 1e-6);
        }
    }

    #[test]
    fn all_faces_match_bake_shader_orientation() {
        let directions = [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z];
        let mut cube = constant([0.0; 3]);
        cube.ambient = 1.0;
        for face in 0..6 {
            cube.texels[face * 4..face * 4 + 4].fill([face as f32, 0.0, 0.0]);
            assert_eq!(cube.sample(directions[face]).x, face as f32);
        }
        for face in 0..6 {
            for uv in [Vec2::new(0.2, 0.7), Vec2::new(0.8, 0.3)] {
                let a = uv * 2.0 - 1.0;
                let d = match face {
                    0 => Vec3::new(1.0, -a.y, -a.x),
                    1 => Vec3::new(-1.0, -a.y, a.x),
                    2 => Vec3::new(a.x, 1.0, a.y),
                    3 => Vec3::new(a.x, -1.0, -a.y),
                    4 => Vec3::new(a.x, -a.y, 1.0),
                    _ => Vec3::new(-a.x, -a.y, -1.0),
                };
                let (actual_face, actual_uv) = face_uv(d);
                assert_eq!(actual_face, face);
                assert!((actual_uv - uv).length() < 1e-6);
            }
        }
    }

    #[test]
    fn bilinear_sampling_interpolates_texel_centers() {
        let mut cube = constant([0.0; 3]);
        cube.ambient = 1.0;
        cube.texels[..4].copy_from_slice(&[[0.0; 3], [2.0; 3], [4.0; 3], [6.0; 3]]);
        assert_eq!(cube.sample(Vec3::X), Vec3::splat(3.0));
        assert_eq!(cube.sample(Vec3::new(1.0, 0.5, 0.5)), Vec3::ZERO);
    }

    #[test]
    fn rejects_bad_shapes_values_formats_and_unknown_fields() {
        let cube = constant([1.0; 3]);
        assert!(SkyRadianceCube::from_json(&serde_json::to_vec(&cube).unwrap()).is_ok());
        let mut bad = cube.clone();
        bad.texels.pop();
        assert!(bad.validate().is_err());
        bad = cube.clone();
        bad.size = 257;
        assert!(bad.validate().is_err());
        bad = cube.clone();
        bad.texels[0][0] = f32::NAN;
        assert!(bad.validate().is_err());
        bad = cube.clone();
        bad.texels[0][0] = -1.0;
        assert!(bad.validate().is_err());
        bad = cube.clone();
        bad.ambient = 3.0;
        assert!(bad.validate().is_err());
        bad = cube.clone();
        bad.format = "other".into();
        assert!(bad.validate().is_err());
        let mut json = serde_json::to_value(cube).unwrap();
        json["unknown"] = serde_json::json!(true);
        assert!(SkyRadianceCube::from_json(&serde_json::to_vec(&json).unwrap()).is_err());
    }
}
