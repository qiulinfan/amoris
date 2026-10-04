//! The sky and its light: the atmosphere baked into a cube when the sun or the sky settings change,
//! a box-filtered mip chain of it, the GGX-prefiltered radiance cube the forward pass samples by
//! roughness, and the irradiance's spherical harmonics.

use bytemuck::{Pod, Zeroable};

use crate::shaders;

const SKY_SIZE: u32 = 128;
const SKY_MIPS: u32 = 6; // 128 .. 4
const ENV_SIZE: u32 = 128;
const ENV_MIPS: u32 = 6;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, PartialEq)]
pub struct SkyParams {
    pub sun: [f32; 4],
    pub color: [f32; 4],
    pub flat_color: [f32; 4],
    pub size: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct IblParams {
    size: [u32; 4],
    rough: [f32; 4],
}

pub struct Sky {
    pub raw: wgpu::Texture,
    pub raw_cube: wgpu::TextureView,
    pub env: wgpu::Texture,
    pub env_cube: wgpu::TextureView,
    pub sh: wgpu::Buffer,
    sh_storage: wgpu::Buffer,
    pub sampler: wgpu::Sampler,
    bake: wgpu::ComputePipeline,
    downsample: wgpu::ComputePipeline,
    prefilter: wgpu::ComputePipeline,
    project: wgpu::ComputePipeline,
    params: wgpu::Buffer,
    last: Option<SkyParams>,
}

fn cube(
    device: &wgpu::Device,
    label: &str,
    size: u32,
    mips: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let t = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 6,
        },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
        view_formats: &[],
    });
    let v = t.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::Cube),
        ..Default::default()
    });
    (t, v)
}

fn storage_view(t: &wgpu::Texture, mip: u32) -> wgpu::TextureView {
    t.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        base_mip_level: mip,
        mip_level_count: Some(1),
        ..Default::default()
    })
}

fn src_view(t: &wgpu::Texture, mips: std::ops::Range<u32>) -> wgpu::TextureView {
    t.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::Cube),
        base_mip_level: mips.start,
        mip_level_count: Some(mips.end - mips.start),
        ..Default::default()
    })
}

impl Sky {
    pub fn new(device: &wgpu::Device) -> Sky {
        let (raw, raw_cube) = cube(device, "sky (raw)", SKY_SIZE, SKY_MIPS);
        let (env, env_cube) = cube(device, "sky (prefiltered)", ENV_SIZE, ENV_MIPS);
        let bake_module = shaders::module(device, "sky_bake");
        let ibl = shaders::module(device, "ibl");
        let compute = |m: &wgpu::ShaderModule, entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: m,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let sh = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sky irradiance (SH)"),
            size: 9 * 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sh_storage = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sky irradiance (SH, compute)"),
            size: 9 * 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        Sky {
            raw,
            raw_cube,
            env,
            env_cube,
            sh,
            sh_storage,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("sky"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            }),
            bake: compute(&bake_module, "bake"),
            downsample: compute(&ibl, "downsample"),
            prefilter: compute(&ibl, "prefilter"),
            project: compute(&ibl, "project_sh"),
            params: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("sky params"),
                size: 256 * 32,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            last: None,
        }
    }

    /// Bakes the sky and its light if `params` changed since the last bake.
    pub fn update(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, params: SkyParams) {
        if self.last == Some(params) {
            return;
        }
        self.last = Some(params);
        // Uniform slots at 256-byte steps: 0 the bake, then one per IBL dispatch.
        let mut slot = 0u64;
        let mut write = |bytes: &[u8]| {
            queue.write_buffer(&self.params, slot * 256, bytes);
            slot += 1;
            (slot - 1) * 256
        };
        let mut p = params;
        p.size = [SKY_SIZE, 0, 0, 0];
        let bake_at = write(bytemuck::bytes_of(&p));
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("sky bake"),
        });
        let uniform = |offset: u64, size: u64| {
            wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &self.params,
                offset,
                size: wgpu::BufferSize::new(size),
            })
        };
        {
            let out = storage_view(&self.raw, 0);
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("sky bake"),
                layout: &self.bake.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform(bake_at, 64),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&out),
                    },
                ],
            });
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.bake);
            pass.set_bind_group(0, &bg, &[]);
            pass.dispatch_workgroups(SKY_SIZE.div_ceil(8), SKY_SIZE.div_ceil(8), 6);
        }
        // Box mips of the raw sky: each from the one above it.
        for mip in 1..SKY_MIPS {
            let size = SKY_SIZE >> mip;
            let at = write(bytemuck::bytes_of(&IblParams {
                size: [size, 1, SKY_MIPS, 0],
                rough: [0.0; 4],
            }));
            let src = src_view(&self.raw, mip - 1..mip);
            let dst = storage_view(&self.raw, mip);
            self.dispatch(
                device,
                &mut enc,
                &self.downsample,
                at,
                &src,
                Some(&dst),
                size,
                false,
            );
        }
        // Prefiltered levels from the whole raw chain.
        let src = src_view(&self.raw, 0..SKY_MIPS);
        for mip in 0..ENV_MIPS {
            let size = ENV_SIZE >> mip;
            let rough = mip as f32 / (ENV_MIPS - 1) as f32;
            let at = write(bytemuck::bytes_of(&IblParams {
                size: [size, mip, SKY_MIPS, 0],
                rough: [rough, 0.0, 0.0, 0.0],
            }));
            let dst = storage_view(&self.env, mip);
            self.dispatch(
                device,
                &mut enc,
                &self.prefilter,
                at,
                &src,
                Some(&dst),
                size,
                false,
            );
        }
        // Irradiance from the 32x32 mip (level 2).
        let at = write(bytemuck::bytes_of(&IblParams {
            size: [SKY_SIZE >> 2, 2, SKY_MIPS, 0],
            rough: [0.0; 4],
        }));
        self.dispatch(device, &mut enc, &self.project, at, &src, None, 1, true);
        enc.copy_buffer_to_buffer(&self.sh_storage, 0, &self.sh, 0, 9 * 16);
        queue.submit([enc.finish()]);
    }

    #[allow(clippy::too_many_arguments)]
    fn dispatch(
        &self,
        device: &wgpu::Device,
        enc: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::ComputePipeline,
        offset: u64,
        src: &wgpu::TextureView,
        dst: Option<&wgpu::TextureView>,
        size: u32,
        sh: bool,
    ) {
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &self.params,
                    offset,
                    size: wgpu::BufferSize::new(32),
                }),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(src),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&self.sampler),
            },
        ];
        if let Some(d) = dst {
            entries.push(wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(d),
            });
        }
        if sh {
            entries.push(wgpu::BindGroupEntry {
                binding: 4,
                resource: self.sh_storage.as_entire_binding(),
            });
        }
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ibl"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &entries,
        });
        let mut pass = enc.begin_compute_pass(&Default::default());
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bg, &[]);
        if sh {
            pass.dispatch_workgroups(1, 1, 1);
        } else {
            pass.dispatch_workgroups(size.div_ceil(8), size.div_ceil(8), 6);
        }
    }
}
