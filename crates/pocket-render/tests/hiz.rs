//! The depth pyramid and the late occlusion test against brute force (docs/spec/occlusion.md 3, 4,
//! 9). Both build the pyramid with `Occlusion::encode_pyramid` from a multisampled depth target
//! that `hiz_probe.wgsl` fills with one depth per sample, then read it back.
//!
//! - `pyramid_is_the_minimum_over_every_sample`: at odd sizes (where a level's last texel takes the
//!   remainder of the level below) and with an independent random depth per sample, every texel of
//!   every level must equal the CPU's minimum over every sample of the pixels it covers (texel `t`
//!   of level `k` covers pixels `[t << (k + 1), (t + 1) << (k + 1))`, its level's last texel up to
//!   the edge).
//! - `occlusion_test_never_hides_a_box_in_front_of_the_depth`: over walls at four depths with
//!   sparse holes (some in one sample only), cull.wgsl's own `occluded` runs on 20,000 random
//!   boxes (sizes from 2 cm to 3 m, any orientation, some reaching behind the camera or off the
//!   screen). A box it hides must be farther at its nearest corner than every sample of every
//!   pixel its projection touches; and it must hide a fair share of them.
//!
//! Skips without a GPU.

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Quat, Vec3, Vec4};
use pocket_render::occlusion::{Occlusion, OcclusionMode, levels};
use pocket_render::profiler::GpuProfiler;
use pocket_render::{BackendChoice, CameraState, Gpu};
use wgpu::util::DeviceExt;

/// The renderer's depth target's samples (`post::SAMPLES`).
const SAMPLES: u32 = 4;
const PER_PIXEL: usize = SAMPLES as usize;

/// `Probe` in hiz_probe.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ProbeUniform {
    view_proj: [[f32; 4]; 4],
    size: [f32; 2],
    levels: u32,
    count: u32,
    pattern: u32,
    seed: u32,
    near: f32,
    _pad: u32,
}

/// `ProbeBox` in hiz_probe.wgsl: a centre and three half axes.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ProbeBox {
    c: [f32; 4],
    a: [[f32; 4]; 3],
}

struct Probe<'a> {
    gpu: &'a Gpu,
    fill: wgpu::RenderPipeline,
    dump: wgpu::ComputePipeline,
    test: wgpu::ComputePipeline,
}

/// A deterministic generator (SplitMix64).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `[a, b)`.
    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

impl<'a> Probe<'a> {
    fn new(gpu: &'a Gpu) -> Probe<'a> {
        let device = &gpu.device;
        let source = format!(
            "{}\n{}",
            pocket_render::shader_source("cull"),
            include_str!("hiz_probe.wgsl")
        );
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hiz probe"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let fill = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hiz probe fill"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("probe_fill_vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("probe_fill_fs"),
                compilation_options: Default::default(),
                targets: &[],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: SAMPLES,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        });
        let compute = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        Probe {
            gpu,
            dump: compute("probe_dump"),
            test: compute("probe_occluded"),
            fill,
        }
    }

    fn uniform(&self, u: &ProbeUniform) -> wgpu::Buffer {
        self.gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("hiz probe"),
                contents: bytemuck::bytes_of(u),
                usage: wgpu::BufferUsages::UNIFORM,
            })
    }

    fn group(
        &self,
        layout: &wgpu::BindGroupLayout,
        entries: &[(u32, wgpu::BindingResource<'_>)],
    ) -> wgpu::BindGroup {
        let entries: Vec<wgpu::BindGroupEntry<'_>> = entries
            .iter()
            .map(|(binding, resource)| wgpu::BindGroupEntry {
                binding: *binding,
                resource: resource.clone(),
            })
            .collect();
        self.gpu
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("hiz probe"),
                layout,
                entries: &entries,
            })
    }

    /// A `w` x `h` multisampled depth target filled with pattern `u.pattern`.
    fn depth(&self, w: u32, h: u32, u: &ProbeUniform) -> wgpu::TextureView {
        let device = &self.gpu.device;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hiz probe depth"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: SAMPLES,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let uniform = self.uniform(u);
        let group = self.group(
            &self.fill.get_bind_group_layout(0),
            &[(10, uniform.as_entire_binding())],
        );
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hiz probe fill"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.fill);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.gpu.queue.submit([enc.finish()]);
        view
    }

    /// Every sample of a `w` x `h` depth target, `SAMPLES` per pixel, row by row.
    fn samples(&self, depth: &wgpu::TextureView, w: u32, h: u32) -> Vec<f32> {
        let device = &self.gpu.device;
        let size = u64::from(w * h * SAMPLES) * 4;
        let out = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hiz probe samples"),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let group = self.group(
            &self.dump.get_bind_group_layout(0),
            &[
                (13, wgpu::BindingResource::TextureView(depth)),
                (14, out.as_entire_binding()),
            ],
        );
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.dump);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
        }
        let bytes = self.read(enc, &out, size);
        bytemuck::pod_collect_to_vec(&bytes)
    }

    /// Copies `src` into a mappable buffer at the end of `enc`, submits and reads it.
    fn read(&self, mut enc: wgpu::CommandEncoder, src: &wgpu::Buffer, size: u64) -> Vec<u8> {
        let device = &self.gpu.device;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hiz probe readback"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        enc.copy_buffer_to_buffer(src, 0, &buf, 0, size);
        self.gpu.queue.submit([enc.finish()]);
        map(device, &buf)
    }

    /// The pyramid of `depth` (`w` x `h`), built by the renderer's own passes.
    fn pyramid(&self, depth: &wgpu::TextureView, w: u32, h: u32) -> Occlusion {
        let device = &self.gpu.device;
        let mut occlusion = Occlusion::new(device, OcclusionMode::On);
        let mut profiler = GpuProfiler::new(device, &self.gpu.queue, false);
        let mut enc = device.create_command_encoder(&Default::default());
        occlusion.encode_pyramid(device, &mut enc, &mut profiler, depth, (w, h));
        self.gpu.queue.submit([enc.finish()]);
        occlusion
    }

    /// Level `k` of a pyramid (`lw` x `lh` texels), row by row.
    fn level(&self, pyramid: &wgpu::Texture, k: u32, lw: u32, lh: u32) -> Vec<f32> {
        let device = &self.gpu.device;
        let row = (lw * 4).div_ceil(256) * 256;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hiz probe level"),
            size: u64::from(row * lh),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: pyramid,
                mip_level: k,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(lh),
                },
            },
            wgpu::Extent3d {
                width: lw,
                height: lh,
                depth_or_array_layers: 1,
            },
        );
        self.gpu.queue.submit([enc.finish()]);
        let bytes = map(device, &buf);
        (0..lh)
            .flat_map(|y| {
                let r = &bytes[(y * row) as usize..(y * row + lw * 4) as usize];
                bytemuck::pod_collect_to_vec::<u8, f32>(r)
            })
            .collect()
    }

    /// cull.wgsl's `occluded` for each box against `pyramid`.
    fn occluded(&self, pyramid: &wgpu::Texture, u: &ProbeUniform, boxes: &[ProbeBox]) -> Vec<bool> {
        let device = &self.gpu.device;
        let uniform = self.uniform(u);
        let input = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("hiz probe boxes"),
            contents: bytemuck::cast_slice(boxes),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let size = boxes.len() as u64 * 4;
        let out = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hiz probe answers"),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let all = pyramid.create_view(&Default::default());
        let group = self.group(
            &self.test.get_bind_group_layout(0),
            &[
                (8, wgpu::BindingResource::TextureView(&all)),
                (10, uniform.as_entire_binding()),
                (11, input.as_entire_binding()),
                (12, out.as_entire_binding()),
            ],
        );
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.test);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups((boxes.len() as u32).div_ceil(64), 1, 1);
        }
        let bytes = self.read(enc, &out, size);
        bytemuck::pod_collect_to_vec::<u8, u32>(&bytes)
            .into_iter()
            .map(|a| a != 0)
            .collect()
    }
}

fn map(device: &wgpu::Device, buf: &wgpu::Buffer) -> Vec<u8> {
    let slice = buf.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    rx.recv()
        .expect("a mapping answer")
        .expect("the buffer maps");
    let bytes = slice.get_mapped_range().expect("mapped").to_vec();
    buf.unmap();
    bytes
}

/// The sizes of a `w` x `h` target's pyramid levels.
fn level_sizes(w: u32, h: u32) -> Vec<(u32, u32)> {
    let (w0, h0) = ((w / 2).max(1), (h / 2).max(1));
    (0..levels(w, h))
        .map(|k| ((w0 >> k).max(1), (h0 >> k).max(1)))
        .collect()
}

/// The farthest (minimum) of each pixel's samples.
fn pixel_min(samples: &[f32]) -> Vec<f32> {
    samples
        .as_chunks::<PER_PIXEL>()
        .0
        .iter()
        .map(|s| s.iter().copied().fold(1.0, f32::min))
        .collect()
}

/// The tests run one at a time: creating two devices at once can fail to load the Vulkan driver.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn gpu() -> Option<Gpu> {
    match Gpu::headless(BackendChoice::from_env()) {
        Ok(gpu) => Some(gpu),
        Err(e) => {
            eprintln!("no GPU ({e:?}): skipped");
            None
        }
    }
}

#[test]
fn pyramid_is_the_minimum_over_every_sample() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Some(gpu) = gpu() else {
        return;
    };
    let probe = Probe::new(&gpu);
    for (i, &(w, h)) in [(481, 271), (1281, 721), (7, 3), (1, 1)].iter().enumerate() {
        let u = ProbeUniform {
            pattern: 0,
            seed: 17 + i as u32,
            ..Zeroable::zeroed()
        };
        let depth = probe.depth(w, h, &u);
        let samples = probe.samples(&depth, w, h);
        // The samples of a pixel differ (else a pyramid reading one sample would pass).
        let mixed = samples
            .as_chunks::<PER_PIXEL>()
            .0
            .iter()
            .filter(|s| s.iter().any(|&d| d != s[0]))
            .count();
        assert!(
            mixed * 10 > (w * h) as usize * 9,
            "{w}x{h}: samples per pixel alike"
        );
        let occlusion = probe.pyramid(&depth, w, h);
        let pyramid = occlusion.pyramid().expect("a pyramid");
        let pixels = pixel_min(&samples);
        let sizes = level_sizes(w, h);
        assert_eq!(pyramid.mip_level_count() as usize, sizes.len());
        for (k, &(lw, lh)) in sizes.iter().enumerate() {
            let mut want = vec![1.0f32; (lw * lh) as usize];
            for y in 0..h {
                for x in 0..w {
                    let tx = (x >> (k + 1)).min(lw - 1);
                    let ty = (y >> (k + 1)).min(lh - 1);
                    let t = (ty * lw + tx) as usize;
                    want[t] = want[t].min(pixels[(y * w + x) as usize]);
                }
            }
            let got = probe.level(pyramid, k as u32, lw, lh);
            let wrong: Vec<_> = (0..want.len())
                .filter(|&t| got[t] != want[t])
                .map(|t| (t as u32 % lw, t as u32 / lw, got[t], want[t]))
                .collect();
            assert!(
                wrong.is_empty(),
                "{w}x{h}, level {k} ({lw}x{lh}): {} texels differ from the minimum over their \
                 samples, e.g. (x, y, pyramid, minimum) {:?}",
                wrong.len(),
                &wrong[..wrong.len().min(4)]
            );
        }
        eprintln!("{w}x{h}: {} levels exact", sizes.len());
    }
}

#[test]
fn occlusion_test_never_hides_a_box_in_front_of_the_depth() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Some(gpu) = gpu() else {
        return;
    };
    let probe = Probe::new(&gpu);
    let (w, h) = (801u32, 451u32);
    let cam = CameraState::look_at(Vec3::ZERO, Vec3::NEG_Z);
    let aspect = w as f32 / h as f32;
    let vp = cam.proj(aspect) * cam.view();
    let u = ProbeUniform {
        view_proj: vp.to_cols_array_2d(),
        size: [w as f32, h as f32],
        levels: levels(w, h),
        pattern: 1,
        seed: 5,
        near: cam.near,
        ..Zeroable::zeroed()
    };
    let depth = probe.depth(w, h, &u);
    let pixels = pixel_min(&probe.samples(&depth, w, h));
    let occlusion = probe.pyramid(&depth, w, h);
    let pyramid = occlusion.pyramid().expect("a pyramid");

    // Boxes centred anywhere a little beyond the view, 3 to 35 m away.
    let mut rng = Rng(0x5eed);
    let tan = (cam.fov_y * 0.5).tan();
    let mut boxes = Vec::new();
    for _ in 0..20_000 {
        let z = rng.range(3.0, 35.0);
        let (sx, sy) = (rng.range(-1.2, 1.2), rng.range(-1.2, 1.2));
        let c = Vec3::new(sx * z * tan * aspect, sy * z * tan, -z);
        let half = Vec3::new(
            0.02 * 150f32.powf(rng.range(0.0, 1.0)),
            0.02 * 150f32.powf(rng.range(0.0, 1.0)),
            0.02 * 150f32.powf(rng.range(0.0, 1.0)),
        );
        let q = Vec4::new(
            rng.range(-1.0, 1.0),
            rng.range(-1.0, 1.0),
            rng.range(-1.0, 1.0),
            rng.range(-1.0, 1.0),
        );
        let rot = Quat::from_vec4(q.normalize_or(Vec4::W));
        boxes.push(ProbeBox {
            c: c.extend(1.0).to_array(),
            a: [
                (rot * Vec3::X * half.x).extend(0.0).to_array(),
                (rot * Vec3::Y * half.y).extend(0.0).to_array(),
                (rot * Vec3::Z * half.z).extend(0.0).to_array(),
            ],
        });
    }
    let u = ProbeUniform {
        count: boxes.len() as u32,
        ..u
    };
    let hidden = probe.occluded(pyramid, &u, &boxes);

    let mut on_screen = 0;
    let mut hidden_count = 0;
    // Boxes farther at their nearest corner than every sample their rectangle touches: what a
    // test reading every pixel could hide.
    let mut hideable = 0;
    let mut wrong = Vec::new();
    for (i, (b, &hid)) in boxes.iter().zip(&hidden).enumerate() {
        match reference(vp, w, h, &pixels, b) {
            Reference::Unseen => {}
            Reference::NeverHidden => {
                on_screen += 1;
                if hid {
                    wrong.push((
                        i,
                        "reaches behind the camera or past the near plane",
                        0.0,
                        0.0,
                    ));
                }
            }
            Reference::Box { near, farthest } => {
                on_screen += 1;
                if farthest > near * (1.0 + 1e-4) {
                    hideable += 1;
                }
                if hid {
                    hidden_count += 1;
                    if farthest < near * (1.0 - 1e-4) {
                        wrong.push((i, "in front of a sample it covers", near, farthest));
                    }
                }
            }
        }
    }
    eprintln!("{on_screen} boxes on screen, {hideable} hideable, {hidden_count} hidden");
    assert!(
        wrong.is_empty(),
        "{} boxes hidden wrongly, e.g. (box, why, nearest corner depth, farthest sample) {:?}",
        wrong.len(),
        &wrong[..wrong.len().min(4)]
    );
    // Not a vacuous pass: the walls could hide many boxes, and the test hides a fair share of
    // those (it reads 2x2 texels of a level coarser than the box, so it hides fewer).
    assert!(
        hideable * 5 > on_screen,
        "{hideable} of {on_screen} hideable"
    );
    assert!(
        hidden_count * 10 > hideable * 3,
        "{hidden_count} of {hideable} hideable boxes hidden"
    );
}

enum Reference {
    /// Its projection misses the target.
    Unseen,
    /// It reaches behind the camera or past the near plane.
    NeverHidden,
    /// Its nearest corner's depth and the farthest sample over the pixels its projection touches.
    Box { near: f32, farthest: f32 },
}

/// What brute force says about a box: every pixel its corners' screen rectangle touches.
fn reference(vp: Mat4, w: u32, h: u32, pixels: &[f32], b: &ProbeBox) -> Reference {
    let c = Vec4::from(b.c).truncate();
    let a = b.a.map(|v| Vec4::from(v).truncate());
    let mut lo = glam::Vec2::splat(f32::INFINITY);
    let mut hi = glam::Vec2::splat(f32::NEG_INFINITY);
    let mut near = 0.0f32;
    for i in 0..8 {
        let s = |bit: u32| if i >> bit & 1 == 1 { 1.0 } else { -1.0 };
        let p = vp * (c + a[0] * s(0) + a[1] * s(1) + a[2] * s(2)).extend(1.0);
        if p.w <= 1e-6 {
            return Reference::NeverHidden;
        }
        let n = p.truncate() / p.w;
        lo = lo.min(n.truncate());
        hi = hi.max(n.truncate());
        near = near.max(n.z);
    }
    if near >= 1.0 {
        return Reference::NeverHidden;
    }
    let (wf, hf) = (w as f32, h as f32);
    let x0 = ((lo.x * 0.5 + 0.5) * wf).floor();
    let x1 = ((hi.x * 0.5 + 0.5) * wf).floor();
    let y0 = ((0.5 - hi.y * 0.5) * hf).floor();
    let y1 = ((0.5 - lo.y * 0.5) * hf).floor();
    if x1 < 0.0 || y1 < 0.0 || x0 > wf - 1.0 || y0 > hf - 1.0 {
        return Reference::Unseen;
    }
    let (x0, x1) = (x0.max(0.0) as u32, x1.min(wf - 1.0) as u32);
    let (y0, y1) = (y0.max(0.0) as u32, y1.min(hf - 1.0) as u32);
    let mut farthest = 1.0f32;
    for y in y0..=y1 {
        for x in x0..=x1 {
            farthest = farthest.min(pixels[(y * w + x) as usize]);
        }
    }
    Reference::Box { near, farthest }
}
