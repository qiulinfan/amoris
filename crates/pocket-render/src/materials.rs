//! Materials and their textures. A material is a row of a storage buffer that the forward pass
//! indexes per instance; instances whose looks are equal share a row. Textures live as layers of
//! two texture arrays (sRGB color data, linear data), each layer resized on the GPU to one size with
//! a full mip chain: every material is reachable from one bind group, so batches are per mesh only
//! and work on WebGPU, which has no bindless textures.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use pocket_assets::frame::Look;
use pocket_assets::mesh::{AlphaMode, ImageData, MaterialData};

use crate::blit::Blitter;

pub const NO_TEXTURE: u32 = u32::MAX;
/// The size of every texture layer.
pub const TEX_SIZE: u32 = 1024;

/// One material row (matches `Material` in common.wgsl).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct MaterialGpu {
    pub base_color: [f32; 4],
    pub emissive: [f32; 3],
    pub metallic: f32,
    pub roughness: f32,
    pub alpha_cutoff: f32,
    pub flags: u32,
    pub _pad: u32,
    pub base_color_tex: u32,
    pub normal_tex: u32,
    pub metal_rough_tex: u32,
    pub emissive_tex: u32,
}

impl Default for MaterialGpu {
    fn default() -> Self {
        MaterialGpu {
            base_color: [1.0; 4],
            emissive: [0.0; 3],
            metallic: 0.0,
            roughness: 0.5,
            alpha_cutoff: 0.5,
            flags: 0,
            _pad: 0,
            base_color_tex: NO_TEXTURE,
            normal_tex: NO_TEXTURE,
            metal_rough_tex: NO_TEXTURE,
            emissive_tex: NO_TEXTURE,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    material: String,
    bits: [u32; 9],
}

fn key(look: &Look) -> Key {
    Key {
        material: look.material.clone(),
        bits: [
            look.color[0].to_bits(),
            look.color[1].to_bits(),
            look.color[2].to_bits(),
            look.color[3].to_bits(),
            look.metallic.to_bits(),
            look.roughness.to_bits(),
            look.emissive[0].to_bits(),
            look.emissive[1].to_bits(),
            look.emissive[2].to_bits(),
        ],
    }
}

/// A texture array that grows by doubling its layers.
pub struct TextureArray {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub format: wgpu::TextureFormat,
    pub layers: u32,
    pub used: u32,
    mips: u32,
}

impl TextureArray {
    fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        layers: u32,
        label: &str,
    ) -> TextureArray {
        let mips = TEX_SIZE.ilog2() + 1;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: TEX_SIZE,
                height: TEX_SIZE,
                depth_or_array_layers: layers,
            },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        TextureArray {
            texture,
            view,
            format,
            layers,
            used: 0,
            mips,
        }
    }

    /// Room for one more layer, growing (and copying) when full; returns whether it was replaced.
    fn reserve(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, label: &str) -> bool {
        if self.used < self.layers {
            return false;
        }
        let mut grown = TextureArray::new(device, self.format, self.layers * 2, label);
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("texture array growth"),
        });
        for mip in 0..self.mips {
            let size = (TEX_SIZE >> mip).max(1);
            enc.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: mip,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &grown.texture,
                    mip_level: mip,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: self.used,
                },
            );
        }
        queue.submit([enc.finish()]);
        grown.used = self.used;
        *self = grown;
        true
    }
}

pub struct MaterialPool {
    pub buffer: wgpu::Buffer,
    rows: Vec<MaterialGpu>,
    by_key: HashMap<Key, u32>,
    /// Asset materials by path (`models/boat.glb#Sail`), with their texture layers resolved.
    assets: HashMap<String, MaterialGpu>,
    pub srgb: TextureArray,
    pub linear: TextureArray,
    dirty: bool,
    pub generation: u64,
}

impl MaterialPool {
    pub fn new(device: &wgpu::Device) -> MaterialPool {
        MaterialPool {
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("materials"),
                size: 256 * std::mem::size_of::<MaterialGpu>() as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            rows: vec![MaterialGpu::default()],
            by_key: HashMap::new(),
            assets: HashMap::new(),
            srgb: TextureArray::new(
                device,
                wgpu::TextureFormat::Rgba8UnormSrgb,
                2,
                "textures (sRGB)",
            ),
            linear: TextureArray::new(
                device,
                wgpu::TextureFormat::Rgba8Unorm,
                2,
                "textures (linear)",
            ),
            dirty: true,
            generation: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The pipeline variant a row needs: 1 alpha-masked, 2 double-sided.
    pub fn variant(&self, id: u32) -> u32 {
        self.rows.get(id as usize).map_or(0, |r| r.flags & 3)
    }

    /// Whether an asset material is known.
    pub fn has_asset(&self, path: &str) -> bool {
        self.assets.contains_key(path)
    }

    /// The row for an instance's look (`None` while its asset material is not loaded).
    pub fn resolve(&mut self, look: &Look) -> Option<u32> {
        let k = key(look);
        if let Some(&id) = self.by_key.get(&k) {
            return Some(id);
        }
        let mut row = if look.material.is_empty() {
            MaterialGpu::default()
        } else {
            *self.assets.get(&look.material)?
        };
        let base = row.base_color;
        row.base_color = [
            base[0] * look.color[0],
            base[1] * look.color[1],
            base[2] * look.color[2],
            base[3] * look.color[3],
        ];
        if look.material.is_empty() {
            row.metallic = look.metallic;
            row.roughness = look.roughness;
            row.emissive = look.emissive;
        } else {
            row.emissive = [
                row.emissive[0] + look.emissive[0],
                row.emissive[1] + look.emissive[1],
                row.emissive[2] + look.emissive[2],
            ];
        }
        let id = self.rows.len() as u32;
        self.rows.push(row);
        self.by_key.insert(k, id);
        self.dirty = true;
        Some(id)
    }

    /// Uploads an asset's images and registers its materials as `prefix#name` and `prefix#index`.
    pub fn add_asset(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        blitter: &mut Blitter,
        prefix: &str,
        materials: &[MaterialData],
        images: &[ImageData],
    ) {
        // Each image goes to the array its use needs; an image used both ways goes to both.
        let mut srgb_layer: HashMap<usize, u32> = HashMap::new();
        let mut linear_layer: HashMap<usize, u32> = HashMap::new();
        let mut layer = |pool: &mut MaterialPool, img: usize, srgb: bool| -> u32 {
            let map = if srgb {
                &mut srgb_layer
            } else {
                &mut linear_layer
            };
            if let Some(&l) = map.get(&img) {
                return l;
            }
            let Some(data) = images.get(img) else {
                return NO_TEXTURE;
            };
            let arr = if srgb {
                &mut pool.srgb
            } else {
                &mut pool.linear
            };
            if arr.reserve(
                device,
                queue,
                if srgb {
                    "textures (sRGB)"
                } else {
                    "textures (linear)"
                },
            ) {
                pool.generation += 1;
            }
            let l = arr.used;
            arr.used += 1;
            blitter.upload_layer(device, queue, data, &arr.texture, arr.format, l, arr.mips);
            map.insert(img, l);
            l
        };
        for (i, m) in materials.iter().enumerate() {
            let mut row = MaterialGpu {
                base_color: m.base_color,
                emissive: m.emissive,
                metallic: m.metallic,
                roughness: m.roughness,
                alpha_cutoff: m.alpha_cutoff,
                flags: match m.alpha_mode {
                    AlphaMode::Mask => 1,
                    _ => 0,
                } | if m.double_sided { 2 } else { 0 },
                ..MaterialGpu::default()
            };
            if let Some(t) = m.base_color_texture {
                row.base_color_tex = layer(self, t, true);
            }
            if let Some(t) = m.emissive_texture {
                row.emissive_tex = layer(self, t, true);
            }
            if let Some(t) = m.normal_texture {
                row.normal_tex = layer(self, t, false);
            }
            if let Some(t) = m.metallic_roughness_texture {
                row.metal_rough_tex = layer(self, t, false);
            }
            self.assets.insert(format!("{prefix}#{i}"), row);
            if !m.name.is_empty() {
                self.assets.insert(format!("{prefix}#{}", m.name), row);
            }
        }
    }

    /// Writes the rows if they changed; returns whether the buffer was replaced.
    pub fn flush(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        if !self.dirty {
            return false;
        }
        self.dirty = false;
        let bytes = (self.rows.len() * std::mem::size_of::<MaterialGpu>()) as u64;
        let mut replaced = false;
        if bytes > self.buffer.size() {
            self.buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("materials"),
                size: bytes.next_power_of_two(),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.generation += 1;
            replaced = true;
        }
        queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&self.rows));
        replaced
    }
}
