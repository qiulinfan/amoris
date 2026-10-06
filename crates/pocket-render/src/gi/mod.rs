//! Derived global illumination resources. The simulation owns settings, never GPU cache state.

use bytemuck::{Pod, Zeroable};
use pocket_assets::gi::{BakedGi, NeuralGi};

#[cfg(all(not(target_arch = "wasm32"), feature = "import"))]
pub mod bake;
pub mod bsdf;
#[cfg(not(target_arch = "wasm32"))]
pub mod nrc;
#[cfg(all(not(target_arch = "wasm32"), feature = "import"))]
pub mod pt;
pub mod reservoir;
#[cfg(all(not(target_arch = "wasm32"), feature = "import"))]
pub mod rt;
#[cfg(all(not(target_arch = "wasm32"), feature = "import"))]
pub mod sky_radiance;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    origin: [f32; 4],
    spacing: [f32; 4],
    dimensions: [u32; 4],
    settings: [f32; 4],
}

/// Baked or learned diffuse lighting data, sampled by the forward pass.
pub struct ProbeVolume {
    pub params: wgpu::Buffer,
    pub radiance: wgpu::Buffer,
    pub distances: wgpu::Buffer,
    pub generation: u64,
    pub asset: String,
    pub loading: bool,
    pub error: Option<String>,
    pub requested_neural: bool,
    data: Option<BakedGi>,
    network: Option<NeuralGi>,
    intensity: f32,
}

fn buffer(device: &wgpu::Device, label: &str, size: u64, uniform: bool) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::COPY_DST
            | if uniform {
                wgpu::BufferUsages::UNIFORM
            } else {
                wgpu::BufferUsages::STORAGE
            },
        mapped_at_creation: false,
    })
}

impl ProbeVolume {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let mut volume = Self {
            params: buffer(device, "GI volume parameters", 64, true),
            radiance: buffer(device, "GI radiance SH", 144, false),
            distances: buffer(device, "GI visibility moments", 8, false),
            generation: 0,
            asset: String::new(),
            loading: false,
            error: None,
            requested_neural: false,
            data: None,
            network: None,
            intensity: 1.0,
        };
        volume.write_params(queue);
        volume
    }

    pub fn clear(&mut self, queue: &wgpu::Queue) {
        self.error = None;
        self.data = None;
        self.network = None;
        self.write_params(queue);
    }

    pub fn has_data(&self) -> bool {
        self.data.is_some() || self.network.is_some()
    }

    pub fn set_intensity(&mut self, queue: &wgpu::Queue, intensity: f32) {
        let value = if intensity.is_finite() {
            intensity.max(0.0)
        } else {
            0.0
        };
        if self.intensity != value {
            self.intensity = value;
            self.write_params(queue);
        }
    }

    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        data: BakedGi,
    ) -> Result<(), String> {
        data.validate()?;
        let sh: Vec<[f32; 4]> = data
            .probes
            .iter()
            .flat_map(|p| p.radiance_sh.iter().map(|s| [s[0], s[1], s[2], 0.0]))
            .collect();
        let distances: Vec<[f32; 2]> = data
            .probes
            .iter()
            .flat_map(|p| p.distance_moments.iter().copied())
            .collect();
        let sh_bytes = bytemuck::cast_slice(&sh);
        let distance_bytes = bytemuck::cast_slice(&distances);
        let limit = u64::from(device.limits().max_storage_buffer_binding_size);
        if sh_bytes.len() as u64 > limit || distance_bytes.len() as u64 > limit {
            return Err(format!(
                "GI volume exceeds this adapter's storage binding limit ({limit} bytes)"
            ));
        }
        self.radiance = buffer(device, "GI radiance SH", sh_bytes.len() as u64, false);
        self.distances = buffer(
            device,
            "GI visibility moments",
            distance_bytes.len() as u64,
            false,
        );
        queue.write_buffer(&self.radiance, 0, sh_bytes);
        queue.write_buffer(&self.distances, 0, distance_bytes);
        self.data = Some(data);
        self.network = None;
        self.generation += 1;
        self.write_params(queue);
        Ok(())
    }

    pub fn upload_neural(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        data: NeuralGi,
    ) -> Result<(), String> {
        data.validate()?;
        let mut weights = Vec::<f32>::with_capacity(1380);
        for layer in &data.layers {
            for row in &layer.weights {
                weights.extend_from_slice(row);
            }
            weights.extend_from_slice(&layer.bias);
        }
        weights.push(0.0); // array<vec4f> packing
        let bytes = bytemuck::cast_slice(&weights);
        self.radiance = buffer(device, "neural GI weights", bytes.len() as u64, false);
        queue.write_buffer(&self.radiance, 0, bytes);
        self.data = None;
        self.network = Some(data);
        self.generation += 1;
        self.write_params(queue);
        Ok(())
    }

    fn write_params(&mut self, queue: &wgpu::Queue) {
        let params = if let Some(n) = &self.network {
            Params {
                origin: [n.input_min[0], n.input_min[1], n.input_min[2], 0.0],
                spacing: [n.input_extent[0], n.input_extent[1], n.input_extent[2], 0.0],
                dimensions: [32, 6, 3, 0],
                settings: [self.intensity, 0.0, 0.0, 2.0],
            }
        } else if let Some(d) = &self.data {
            Params {
                origin: [d.origin[0], d.origin[1], d.origin[2], 0.0],
                spacing: [d.spacing[0], d.spacing[1], d.spacing[2], 0.0],
                dimensions: [
                    d.dimensions[0],
                    d.dimensions[1],
                    d.dimensions[2],
                    d.distance_resolution,
                ],
                settings: [
                    self.intensity,
                    d.spacing.iter().copied().fold(f32::INFINITY, f32::min) * 0.03,
                    d.max_distance,
                    1.0,
                ],
            }
        } else {
            Params {
                origin: [0.0; 4],
                spacing: [1.0; 4],
                dimensions: [1; 4],
                settings: [0.0; 4],
            }
        };
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&params));
    }
}
