//! Materials and their textures. A material is a row of a storage buffer that the forward pass
//! indexes per instance; instances whose looks are equal share a row. Textures live as layers of
//! two texture arrays (sRGB color data, linear data), each layer resized to one size with a full
//! mip chain (on the GPU by a linear sampler; an image whose glTF sampler magnifies with `NEAREST`
//! by point sampling first, blit.rs): every material is reachable from one bind group, so batches
//! are per mesh only and work on WebGPU, which has no bindless textures. Where the device samples
//! BC, a KTX2 image in BC7 whose levels fit a layer goes as stored into a third or fourth array,
//! BC7 sRGB or linear; a layer index with [`TEX_BLOCKS`] set names one of those
//! (docs/spec/texture-formats.md 4).

use std::collections::HashMap;

use amoris_assets::frame::Look;
use amoris_assets::mesh::{AlphaMode, BlockFormat, ImageData, MaterialData};
use amoris_assets::neural::{Channel, NeuralLayout};
use bytemuck::{Pod, Zeroable};

use crate::blit::Blitter;

pub const NO_TEXTURE: u32 = u32::MAX;
/// Material flag: the channels come from a neural texture (`MaterialGpu::neural`).
pub const FLAG_NEURAL: u32 = 8;
/// Material flag: alpha-blended (glTF BLEND, or a look whose alpha is below 1): drawn by the
/// transparent stage (transparent.rs).
pub const FLAG_BLEND: u32 = 16;
/// Material flag: a terrain layer's (terrain.rs): texture coordinates from the world's x and z,
/// and the mesh's texture coordinates the interval of the layer's share that the forward pass
/// keeps (forward.wgsl `terrain_kept`). Always alpha-masked, with no alpha cutoff, so the layer is
/// drawn by the pipelines that may discard.
pub const FLAG_TERRAIN: u32 = 32;
/// The size of every texture layer.
pub const TEX_SIZE: u32 = 1024;
/// The levels of a layer: TEX_SIZE down to 1 x 1.
pub const TEX_MIPS: u32 = TEX_SIZE.ilog2() + 1;
/// Layer bit: the layer is in the BC7 array of its kind (forward.wgsl `TEX_BLOCKS`).
pub const TEX_BLOCKS: u32 = 0x4000_0000;

/// Where an image's layer comes from (docs/spec/texture-formats.md 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// A BC7 KTX2 image whose levels fit, on a device that samples BC: the file's blocks from
    /// level `j` on, as stored, in a BC7 array.
    Blocks(usize),
    /// Another KTX2 image whose levels fit: its levels from `j` on, decoded to RGBA8 on the CPU,
    /// each written to its mip of the RGBA8 array.
    Levels(usize),
    /// Any other image: level 0 (`rgba8`) resized on the GPU and its mips drawn from it (blit.rs).
    Resize,
}

/// The file level that is a layer's mip 0, when an image's levels fit a layer: square, a power of
/// two at least [`TEX_SIZE`], and every level from that one down to 1 x 1 present. Never for a
/// marked image (it is resized by point sampling).
pub fn fitting_level(image: &ImageData) -> Option<usize> {
    let levels = image.levels.as_ref()?;
    if image.nearest
        || image.width != image.height
        || !image.width.is_power_of_two()
        || image.width < TEX_SIZE
    {
        return None;
    }
    let j = (image.width / TEX_SIZE).ilog2() as usize;
    (levels.levels.len() >= j + TEX_MIPS as usize).then_some(j)
}

/// Where `image`'s layer comes from on a device that samples BC (`bc`) or not.
pub fn placement(image: &ImageData, bc: bool) -> Placement {
    match (fitting_level(image), &image.levels) {
        (Some(j), Some(l)) if bc && l.format == BlockFormat::Bc7 => Placement::Blocks(j),
        (Some(j), _) => Placement::Levels(j),
        _ => Placement::Resize,
    }
}

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
    /// The neural texture's descriptor word in the neural table (flag 8), else [`NO_TEXTURE`].
    pub neural: u32,
    pub base_color_tex: u32,
    pub normal_tex: u32,
    pub metal_rough_tex: u32,
    pub emissive_tex: u32,
    /// KHR_materials_transmission's factor (0: none) and its texture's layer (red channel).
    pub transmission: f32,
    /// KHR_materials_ior.
    pub ior: f32,
    pub transmission_tex: u32,
    /// The scale every texture coordinate of the material takes (KHR_texture_transform).
    pub uv_scale: f32,
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
            neural: NO_TEXTURE,
            base_color_tex: NO_TEXTURE,
            normal_tex: NO_TEXTURE,
            metal_rough_tex: NO_TEXTURE,
            emissive_tex: NO_TEXTURE,
            transmission: 0.0,
            ior: 1.5,
            transmission_tex: NO_TEXTURE,
            uv_scale: 1.0,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    material: String,
    bits: [u32; 9],
    /// The look's transmission and IOR overrides.
    transmission: Option<u32>,
    ior: Option<u32>,
    terrain: bool,
}

fn key(look: &Look) -> Key {
    Key {
        terrain: crate::terrain::is_layer_mesh(&look.mesh),
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
        transmission: look.transmission.map(f32::to_bits),
        ior: look.ior.map(f32::to_bits),
    }
}

/// Whether a row is drawn by the transparent stage ([`MaterialPool::transparent`]).
fn transparent(r: &MaterialGpu) -> bool {
    r.flags & FLAG_NEURAL == 0 && (r.flags & FLAG_BLEND != 0 || r.transmission > 0.0)
}

/// Whether a row can cast a shadow ([`MaterialPool::casts`]): a transparent row casts as
/// alpha-masked, its opacity `alpha * (1 - transmission)` tested against its cutoff, and that
/// opacity is at most the alpha factor times `1 - transmission` (a texture's alpha is at most 1).
fn casts(r: &MaterialGpu) -> bool {
    !transparent(r) || r.base_color[3] * (1.0 - r.transmission) >= r.alpha_cutoff
}

/// An asset material's row, its textures not placed yet.
fn asset_row(m: &MaterialData) -> MaterialGpu {
    let mut row = MaterialGpu {
        base_color: m.base_color,
        emissive: m.emissive,
        metallic: m.metallic,
        roughness: m.roughness,
        alpha_cutoff: m.alpha_cutoff,
        flags: match m.alpha_mode {
            AlphaMode::Mask => 1,
            AlphaMode::Blend => FLAG_BLEND,
            AlphaMode::Opaque => 0,
        } | if m.double_sided { 2 } else { 0 },
        transmission: m.transmission.clamp(0.0, 1.0),
        ior: m.ior,
        uv_scale: m.uv_scale,
        ..MaterialGpu::default()
    };
    if m.alpha_mode == AlphaMode::Opaque {
        // glTF: an opaque material's alpha is ignored (a look's alpha may still blend it).
        row.base_color[3] = 1.0;
    }
    row
}

/// The row for `look` over `row`: the inline material's default, or the asset material it names.
fn look_row(mut row: MaterialGpu, look: &Look) -> MaterialGpu {
    let base = row.base_color;
    row.base_color = [
        base[0] * look.color[0],
        base[1] * look.color[1],
        base[2] * look.color[2],
        base[3] * look.color[3],
    ];
    // A look whose alpha is below 1 blends, unless its material cuts by alpha (the look's alpha
    // then scales what is tested) or is a neural texture's.
    if look.color[3] < 1.0 && row.flags & (1 | FLAG_NEURAL) == 0 {
        row.flags |= FLAG_BLEND;
    }
    if let Some(t) = look.transmission {
        row.transmission = t.clamp(0.0, 1.0);
    }
    if let Some(ior) = look.ior {
        row.ior = ior;
    }
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
    row
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
        let mips = TEX_MIPS;
        // A block-compressed array is written only by copies; the RGBA8 ones are drawn into.
        let drawn = if format.is_compressed() {
            wgpu::TextureUsages::empty()
        } else {
            wgpu::TextureUsages::RENDER_ATTACHMENT
        };
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
                | drawn
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
                // A mip smaller than a block is copied whole: its blocks' size.
                wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: self.used,
                }
                .physical_size(self.format),
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
    /// The BC7 arrays (sRGB, linear), made at the first image that takes one; until then the
    /// bind group holds `placeholder` in their place.
    pub srgb_blocks: Option<TextureArray>,
    pub linear_blocks: Option<TextureArray>,
    placeholder: wgpu::TextureView,
    /// The device samples BC ([`crate::gpu::Capabilities::texture_compression_bc`]).
    bc: bool,
    dirty: bool,
    pub generation: u64,
}

impl MaterialPool {
    /// `bc`: the device samples BC textures.
    pub fn new(device: &wgpu::Device, bc: bool) -> MaterialPool {
        let placeholder = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("textures (no blocks)"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
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
            srgb_blocks: None,
            linear_blocks: None,
            placeholder,
            bc,
            dirty: true,
            generation: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// What the textures group binds for the BC7 sRGB array.
    pub fn srgb_blocks_view(&self) -> &wgpu::TextureView {
        self.srgb_blocks
            .as_ref()
            .map_or(&self.placeholder, |a| &a.view)
    }

    /// What the textures group binds for the BC7 linear array.
    pub fn linear_blocks_view(&self) -> &wgpu::TextureView {
        self.linear_blocks
            .as_ref()
            .map_or(&self.placeholder, |a| &a.view)
    }

    /// The layers held block-compressed, both kinds (`FrameStats::texture_blocks`).
    pub fn block_layers(&self) -> u32 {
        self.srgb_blocks.as_ref().map_or(0, |a| a.used)
            + self.linear_blocks.as_ref().map_or(0, |a| a.used)
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The pipeline variant a row needs: 1 alpha-masked, 2 double-sided, 4 neural texture. A
    /// transparent row is drawn by the transparent stage, so its variant only says how it casts
    /// shadows: alpha-masked, its opacity `alpha * (1 - transmission)` tested against its cutoff
    /// (forward.wgsl `fs_shadow_masked`).
    pub fn variant(&self, id: u32) -> u32 {
        self.rows.get(id as usize).map_or(0, |r| {
            (r.flags & 3)
                | if r.flags & FLAG_NEURAL != 0 {
                    crate::scene::NEURAL_VARIANT
                } else {
                    0
                }
                | u32::from(transparent(r))
        })
    }

    /// Whether a row is drawn by the transparent stage (transparent.rs): it blends, or it
    /// transmits; never a neural texture's (no transparent pipeline decodes one).
    pub fn transparent(&self, id: u32) -> bool {
        self.rows.get(id as usize).is_some_and(transparent)
    }

    /// Whether a row can cast a shadow: every row but a transparent one whose opacity can never
    /// reach its cutoff (clear glass, a look's alpha below the cutoff), which the shadow passes
    /// would draw only to discard every fragment.
    pub fn casts(&self, id: u32) -> bool {
        self.rows.get(id as usize).is_none_or(casts)
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
        let row = look_row(
            if look.material.is_empty() {
                MaterialGpu::default()
            } else {
                *self.assets.get(&look.material)?
            },
            look,
        );
        let mut row = row;
        if k.terrain {
            row.flags |= FLAG_TERRAIN | 1;
            row.alpha_cutoff = 0.0;
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
            let place = placement(data, pool.bc);
            let (arr, label, bit) = match (place, srgb) {
                (Placement::Blocks(_), true) => {
                    let arr = pool.srgb_blocks.get_or_insert_with(|| {
                        TextureArray::new(
                            device,
                            wgpu::TextureFormat::Bc7RgbaUnormSrgb,
                            2,
                            "textures (BC7 sRGB)",
                        )
                    });
                    (arr, "textures (BC7 sRGB)", TEX_BLOCKS)
                }
                (Placement::Blocks(_), false) => {
                    let arr = pool.linear_blocks.get_or_insert_with(|| {
                        TextureArray::new(
                            device,
                            wgpu::TextureFormat::Bc7RgbaUnorm,
                            2,
                            "textures (BC7 linear)",
                        )
                    });
                    (arr, "textures (BC7 linear)", TEX_BLOCKS)
                }
                (_, true) => (&mut pool.srgb, "textures (sRGB)", 0),
                (_, false) => (&mut pool.linear, "textures (linear)", 0),
            };
            // A new BC7 array (its first layer) or a grown one is a new view to bind.
            let made = arr.used == 0 && bit != 0;
            if arr.reserve(device, queue, label) || made {
                pool.generation += 1;
            }
            let l = arr.used;
            arr.used += 1;
            match (place, &data.levels) {
                (Placement::Blocks(j), Some(file)) => {
                    let levels: Vec<&[u8]> = file.levels[j..j + TEX_MIPS as usize]
                        .iter()
                        .map(Vec::as_slice)
                        .collect();
                    crate::blit::write_levels(queue, &arr.texture, l, &levels);
                }
                (Placement::Levels(j), Some(file)) => {
                    let decoded: Result<Vec<Vec<u8>>, String> = (j..j + TEX_MIPS as usize)
                        .map(|k| {
                            if k == 0 {
                                return Ok(data.rgba8.clone());
                            }
                            let side = (data.width >> k).max(1);
                            amoris_assets::ktx2::decode(file.format, side, side, &file.levels[k])
                        })
                        .collect();
                    match decoded {
                        Ok(levels) => {
                            let levels: Vec<&[u8]> = levels.iter().map(Vec::as_slice).collect();
                            crate::blit::write_levels(queue, &arr.texture, l, &levels);
                        }
                        Err(e) => {
                            log::warn!(
                                "{}: its levels do not decode ({e}); resizing level 0",
                                data.name
                            );
                            blitter.upload_layer(
                                device,
                                queue,
                                data,
                                &arr.texture,
                                arr.format,
                                l,
                                arr.mips,
                            );
                        }
                    }
                }
                _ => {
                    blitter.upload_layer(device, queue, data, &arr.texture, arr.format, l, arr.mips)
                }
            }
            map.insert(img, l | bit);
            l | bit
        };
        for (i, m) in materials.iter().enumerate() {
            let mut row = asset_row(m);
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
            if let Some(t) = m.transmission_texture.filter(|_| m.transmission > 0.0) {
                row.transmission_tex = layer(self, t, false);
            }
            self.assets.insert(format!("{prefix}#{i}"), row);
            if !m.name.is_empty() {
                self.assets.insert(format!("{prefix}#{}", m.name), row);
            }
        }
    }

    /// Registers the material a neural texture decodes, under its path (`materials/brick.ntex`):
    /// opaque and single-sided, its factors 1 for the channels it has (the look's color and
    /// emissive apply on top as for any asset material), the glTF defaults for those it lacks.
    pub fn add_neural(&mut self, path: &str, descriptor: u32, layout: &NeuralLayout) {
        let has = |c: Channel| layout.output_of(c).is_some();
        let row = MaterialGpu {
            emissive: if has(Channel::EmissiveR) {
                [1.0; 3]
            } else {
                [0.0; 3]
            },
            metallic: if has(Channel::Metallic) { 1.0 } else { 0.0 },
            roughness: if has(Channel::Roughness) { 1.0 } else { 0.5 },
            flags: FLAG_NEURAL,
            neural: descriptor,
            ..MaterialGpu::default()
        };
        self.assets.insert(path.to_owned(), row);
    }

    /// Registers `path` as the default material (an asset that failed to load still draws).
    pub fn add_fallback(&mut self, path: &str) {
        self.assets.insert(path.to_owned(), MaterialGpu::default());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn look(material: &str, alpha: f32) -> Look {
        Look {
            mesh: "sphere".into(),
            material: material.into(),
            color: [1.0, 1.0, 1.0, alpha],
            metallic: 0.0,
            roughness: 0.5,
            transmission: None,
            ior: None,
            emissive: [0.0; 3],
            cast_shadows: true,
            visible: true,
        }
    }

    fn asset(mode: AlphaMode, alpha: f32, transmission: f32) -> MaterialGpu {
        asset_row(&MaterialData {
            base_color: [1.0, 1.0, 1.0, alpha],
            alpha_mode: mode,
            transmission,
            ..MaterialData::default()
        })
    }

    #[test]
    fn which_rows_are_transparent_and_which_cast() {
        let inline = |alpha| look_row(MaterialGpu::default(), &look("", alpha));
        let over = |row, alpha| look_row(row, &look("m.glb#0", alpha));
        // The inline material: opaque at alpha 1, blended below.
        assert!(!transparent(&inline(1.0)) && casts(&inline(1.0)));
        assert!(transparent(&inline(0.5)) && casts(&inline(0.5)));
        assert!(transparent(&inline(0.35)) && !casts(&inline(0.35)));
        // glTF BLEND: transparent, casting where its alpha may reach the cutoff (0.5).
        let blend = |alpha| over(asset(AlphaMode::Blend, alpha, 0.0), 1.0);
        assert!(transparent(&blend(0.6)) && casts(&blend(0.6)));
        assert!(transparent(&blend(0.45)) && !casts(&blend(0.45)));
        // MASK keeps cutting by the alpha the look scales: never blended, always casting.
        let mask = over(asset(AlphaMode::Mask, 1.0, 0.0), 0.5);
        assert!(!transparent(&mask) && casts(&mask));
        assert_eq!(mask.flags & (1 | FLAG_BLEND), 1);
        // OPAQUE ignores its own alpha; a look's alpha still blends it.
        let opaque = asset(AlphaMode::Opaque, 0.3, 0.0);
        assert_eq!(opaque.base_color[3], 1.0);
        assert!(!transparent(&over(opaque, 1.0)));
        assert!(transparent(&over(opaque, 0.5)));
        // Transmission: imported, or the look's override either way.
        let glass = asset(AlphaMode::Opaque, 1.0, 1.0);
        assert!(transparent(&over(glass, 1.0)) && !casts(&over(glass, 1.0)));
        let cleared = look_row(
            glass,
            &Look {
                transmission: Some(0.0),
                ..look("m.glb#0", 1.0)
            },
        );
        assert!(!transparent(&cleared) && casts(&cleared));
        let tinted = look_row(
            MaterialGpu::default(),
            &Look {
                transmission: Some(0.4),
                ior: Some(1.33),
                ..look("", 1.0)
            },
        );
        assert!(transparent(&tinted) && casts(&tinted));
        assert_eq!((tinted.transmission, tinted.ior), (0.4, 1.33));
        // A neural texture's row never blends or transmits through the stage.
        let neural = MaterialGpu {
            flags: FLAG_NEURAL,
            transmission: 1.0,
            ..MaterialGpu::default()
        };
        let neural = over(neural, 0.5);
        assert!(neural.flags & FLAG_BLEND == 0 && !transparent(&neural) && casts(&neural));
    }

    #[test]
    fn each_image_finds_its_placement() {
        use amoris_assets::mesh::TextureLevels;
        // An image of `side` x `height` with `count` levels of `format` (their bytes do not
        // matter here), or a PNG's (no levels).
        let image = |side: u32, height: u32, levels: Option<(BlockFormat, usize)>| ImageData {
            width: side,
            height,
            levels: levels.map(|(format, count)| TextureLevels {
                format,
                levels: vec![Vec::new(); count],
            }),
            ..ImageData::default()
        };
        let bc7 = BlockFormat::Bc7;
        let full = TEX_MIPS as usize;
        for bc in [true, false] {
            let fits = |j| {
                if bc {
                    Placement::Blocks(j)
                } else {
                    Placement::Levels(j)
                }
            };
            assert_eq!(
                placement(&image(1024, 1024, Some((bc7, full))), bc),
                fits(0)
            );
            // A 2048 image's chain starts at its level 1; a 4096's at its level 2.
            assert_eq!(
                placement(&image(2048, 2048, Some((bc7, full + 1))), bc),
                fits(1)
            );
            assert_eq!(
                placement(&image(4096, 4096, Some((bc7, full + 2))), bc),
                fits(2)
            );
            // Other formats that fit are decoded, level by level.
            for format in [BlockFormat::Rgba8, BlockFormat::Bc1, BlockFormat::Bc5] {
                let i = image(1024, 1024, Some((format, full)));
                assert_eq!(placement(&i, bc), Placement::Levels(0), "{format:?}");
            }
            // What does not fit is resized as a PNG is: a short chain, a smaller, a non-square or
            // a non-power-of-two image, a marked image, and every PNG.
            for i in [
                image(1024, 1024, Some((bc7, full - 1))),
                image(1024, 1024, Some((bc7, 1))),
                image(512, 512, Some((bc7, 10))),
                image(1024, 512, Some((bc7, full))),
                image(1536, 1536, Some((bc7, full))),
                ImageData {
                    nearest: true,
                    ..image(1024, 1024, Some((bc7, full)))
                },
                image(1024, 1024, None),
            ] {
                assert_eq!(
                    placement(&i, bc),
                    Placement::Resize,
                    "{} x {}",
                    i.width,
                    i.height
                );
            }
        }
    }
}
