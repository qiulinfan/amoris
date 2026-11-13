//! KTX2 textures (docs/spec/texture-formats.md 8). Two checks, each skipping without a GPU:
//!
//! - The CPU's block decoder (`amoris_assets::ktx2::decode`, the fallback) gives the texels the
//!   GPU samples: BC1, BC3, BC4, BC5 and BC7 levels made by Intel's encoder, every mip of a
//!   128 x 128 chain, uploaded as stored and read back with `textureLoad`. Skipped (with a line
//!   saying so) on a device without `texture-compression-bc`.
//! - A model whose quads carry KTX2 base colors (BC7 with a BC7 normal map, both fitting a layer;
//!   BC1 fitting a layer; BC7 too small to fit) draws the same frames on a device that samples BC7
//!   as stored and on one that decodes every image on the CPU (`AMORIS_GPU_MINIMAL`'s
//!   `texture-compression`), and close up the same frames as the decoded images handed in as
//!   plain RGBA8, as a PNG is. The frame's stats count the layers held as BC7.

#[path = "../examples/ktx2/encode.rs"]
#[allow(dead_code)]
mod encode;

use amoris_assets::frame::{
    EnvironmentView, InstanceUpdate, LightKindView, LightView, Look, Pose, RenderFrame,
};
use amoris_assets::ktx2;
use amoris_assets::mesh::{BlockFormat, ImageData, MaterialData, ModelAsset, NodeData};
use amoris_assets::primitives::primitive;
use amoris_render::gpu::Minimal;
use amoris_render::materials::{Placement, TEX_SIZE, placement};
use amoris_render::{Antialiasing, BackendChoice, CameraState, Gpu, Gtao, Renderer};
use glam::{Quat, Vec3};

/// A 128 x 128 test image: the pattern's squares and ring, alpha falling from left to right with a
/// cut-out strip (BC1's transparent texels, BC3's and BC7's alpha).
fn alpha_pattern(side: u32) -> Vec<u8> {
    let mut px = encode::pattern(side, side, 3);
    for y in 0..side {
        for x in 0..side {
            let a = &mut px[((y * side + x) * 4 + 3) as usize];
            *a = if x < side / 8 {
                0
            } else {
                255 - (x * 160 / side) as u8
            };
        }
    }
    px
}

fn wgpu_format(format: BlockFormat) -> wgpu::TextureFormat {
    match format {
        BlockFormat::Rgba8 => wgpu::TextureFormat::Rgba8Unorm,
        BlockFormat::Bc1 => wgpu::TextureFormat::Bc1RgbaUnorm,
        BlockFormat::Bc3 => wgpu::TextureFormat::Bc3RgbaUnorm,
        BlockFormat::Bc4 => wgpu::TextureFormat::Bc4RUnorm,
        BlockFormat::Bc5 => wgpu::TextureFormat::Bc5RgUnorm,
        BlockFormat::Bc7 => wgpu::TextureFormat::Bc7RgbaUnorm,
    }
}

/// Every mip of `levels` (a `side`-square chain) as the GPU samples it, RGBA8.
fn gpu_texels(gpu: &Gpu, side: u32, levels: &amoris_assets::mesh::TextureLevels) -> Vec<Vec<u8>> {
    let (device, queue) = (&gpu.device, &gpu.queue);
    let format = wgpu_format(levels.format);
    let mips = levels.levels.len() as u32;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ktx2 test"),
        size: wgpu::Extent3d {
            width: side,
            height: side,
            depth_or_array_layers: 1,
        },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let (bw, bh) = format.block_dimensions();
    let block = format.block_copy_size(None).expect("a color format");
    for (m, data) in levels.levels.iter().enumerate() {
        let size = (side >> m).max(1);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: m as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size.div_ceil(bw) * block),
                rows_per_image: Some(size.div_ceil(bh)),
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            }
            .physical_size(format),
        );
    }
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ktx2 load"),
        source: wgpu::ShaderSource::Wgsl(include_str!("ktx2_load.wgsl").into()),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("ktx2 load"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (0..mips)
        .map(|m| {
            let size = (side >> m).max(1);
            let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("level"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            queue.write_buffer(&uniform, 0, bytemuck::cast_slice(&[m, 0, 0, 0]));
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ktx2 load"),
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: uniform.as_entire_binding(),
                    },
                ],
            });
            let target = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("ktx2 texels"),
                size: wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let row = (size * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ktx2 readback"),
                size: u64::from(row * size),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut enc = device.create_command_encoder(&Default::default());
            {
                let tv = target.create_view(&Default::default());
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("ktx2 load"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &tv,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &bind, &[]);
                pass.draw(0..3, 0..1);
            }
            enc.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &target,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row),
                        rows_per_image: Some(size),
                    },
                },
                wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: 1,
                },
            );
            queue.submit([enc.finish()]);
            let slice = readback.slice(..);
            slice.map_async(wgpu::MapMode::Read, |r| r.expect("the readback maps"));
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("the device waits");
            let mapped = slice.get_mapped_range().expect("mapped");
            let mut out = Vec::with_capacity((size * size * 4) as usize);
            for y in 0..size {
                let at = (y * row) as usize;
                out.extend_from_slice(&mapped[at..at + (size * 4) as usize]);
            }
            out
        })
        .collect()
}

#[test]
fn the_cpu_decodes_each_bc_format_as_the_gpu_samples_it() {
    let Ok(gpu) = Gpu::headless_with(BackendChoice::from_env(), Minimal::from_env()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    if !gpu.caps.texture_compression_bc {
        eprintln!("no texture-compression-bc: the GPU decoding comparison skipped");
        return;
    }
    let side = 128;
    let image = alpha_pattern(side);
    // The largest difference in 8-bit steps each format may show: BC7's decoding is exact by
    // the format's definition; BC1's and BC3's color and BC4's and BC5's values are interpolated
    // between endpoints, which a GPU may do at a higher precision than the decoder's 8-bit
    // integer thirds and sevenths, truncated. Measured (2026-11-12): BC1 and BC3 4 on the RTX 5060
    // (D3D12, Vulkan) and 2 on the Radeon 780M; BC4 2 on both; BC5 3 on the 5060, 2 on the 780M.
    let formats = [
        (BlockFormat::Bc1, 4u8),
        (BlockFormat::Bc3, 4),
        (BlockFormat::Bc4, 2),
        (BlockFormat::Bc5, 3),
        (BlockFormat::Bc7, 0),
    ];
    for (format, tolerance) in formats {
        let levels = encode::encode(format, side, side, &image, true);
        let file = ktx2::write(side, side, false, &levels, true);
        let read = ktx2::read(&file).expect("the file reads");
        assert_eq!(
            read.levels,
            levels,
            "{}: the file holds the blocks",
            format.name()
        );
        let gpu_levels = gpu_texels(&gpu, side, &levels);
        // The GPU reads BC4 as (r, 0, 0, 1) and BC5 as (r, g, 0, 1); the decoder gives BC4 as
        // grey, as the importer reads a one-channel PNG.
        let channels: &[usize] = match format {
            BlockFormat::Bc4 => &[0],
            BlockFormat::Bc5 => &[0, 1],
            _ => &[0, 1, 2, 3],
        };
        let mut worst = 0u8;
        let mut differing = 0usize;
        for (m, gpu_level) in gpu_levels.iter().enumerate() {
            let size = (side >> m).max(1);
            let cpu = ktx2::decode(format, size, size, &levels.levels[m]).expect("it decodes");
            for (p, q) in cpu.chunks(4).zip(gpu_level.chunks(4)) {
                let d = channels
                    .iter()
                    .map(|&c| p[c].abs_diff(q[c]))
                    .max()
                    .unwrap_or(0);
                worst = worst.max(d);
                differing += usize::from(d > 0);
            }
        }
        eprintln!(
            "{}: {} levels; {differing} texels differ, by at most {worst}",
            format.name(),
            gpu_levels.len()
        );
        assert!(
            worst <= tolerance,
            "{}: the decoder differs from the GPU by {worst} 8-bit steps ({differing} texels; \
             at most {tolerance} allowed)",
            format.name()
        );
    }
}

const W: u32 = 256;
const H: u32 = 256;
const MODEL: &str = "test/ktx2.glb";
/// The quads, their x positions: each 4 m square.
const QUADS: [(&str, f32); 4] = [
    ("bc7", -6.0),
    ("bc1", -2.0),
    ("small", 2.0),
    ("normal", 6.0),
];

/// A KTX2 image as the importer hands it over: the file written and read back.
fn ktx2_image(name: &str, format: BlockFormat, side: u32, rgba: &[u8], srgb: bool) -> ImageData {
    let file = ktx2::write(
        side,
        side,
        srgb,
        &encode::encode(format, side, side, rgba, true),
        true,
    );
    ktx2::image_data(name.into(), &file, srgb).expect("the file reads")
}

/// The test's images: `bc7`'s base color and normal map (BC7, 1024: they fit a layer), `bc1`'s
/// base color (BC1, 1024: fits, decoded), `small`'s (BC7, 256: resized), and `normal`, the same
/// BC7 normal map under a white base color.
fn images() -> Vec<ImageData> {
    let big = TEX_SIZE;
    vec![
        ktx2_image(
            "bc7",
            BlockFormat::Bc7,
            big,
            &encode::pattern(big, big, 0),
            true,
        ),
        ktx2_image(
            "bc7 normal",
            BlockFormat::Bc7,
            big,
            &encode::normals(big, big),
            false,
        ),
        ktx2_image(
            "bc1",
            BlockFormat::Bc1,
            big,
            &encode::pattern(big, big, 1),
            true,
        ),
        ktx2_image(
            "small",
            BlockFormat::Bc7,
            256,
            &encode::pattern(256, 256, 2),
            true,
        ),
    ]
}

/// A model of the four quads (the plane primitive), materials over `images`.
fn model(images: Vec<ImageData>) -> ModelAsset {
    let mut asset = ModelAsset {
        images,
        ..ModelAsset::default()
    };
    let materials = [
        (Some(0), Some(1)),
        (Some(2), None),
        (Some(3), None),
        (None, Some(1)),
    ];
    for (i, ((name, _), (base, normal))) in QUADS.iter().zip(materials).enumerate() {
        let mut mesh = primitive("plane").expect("the plane primitive");
        mesh.name = (*name).into();
        mesh.material = Some(i);
        asset.meshes.push(mesh);
        asset.materials.push(MaterialData {
            name: (*name).into(),
            roughness: 0.6,
            base_color_texture: base,
            normal_texture: normal,
            ..MaterialData::default()
        });
        asset.nodes.push(NodeData {
            name: (*name).into(),
            mesh: i,
            transform: glam::Mat4::IDENTITY.to_cols_array(),
            skin: None,
        });
    }
    asset
}

/// The four quads facing the camera (+z), lit from the side.
fn scene() -> RenderFrame {
    let instances = QUADS
        .iter()
        .enumerate()
        .map(|(i, (name, x))| InstanceUpdate {
            id: i as u64 + 1,
            pose: Some(Pose {
                position: [*x, 0.0, 0.0],
                rotation: Quat::from_rotation_x(std::f32::consts::FRAC_PI_2).to_array(),
                scale: [4.0, 1.0, 4.0],
            }),
            look: Some(Look {
                mesh: format!("{MODEL}#{name}"),
                material: String::new(),
                color: [1.0; 4],
                metallic: 0.0,
                roughness: 0.6,
                transmission: None,
                ior: None,
                emissive: [0.0; 3],
                cast_shadows: false,
                visible: true,
            }),
            anim: None,
        })
        .collect();
    RenderFrame {
        tick: 1,
        dt_s: 1.0 / 60.0,
        reset: true,
        instances,
        lights: Some(vec![LightView {
            id: 0,
            kind: LightKindView::Directional,
            position: [0.0; 3],
            direction: Vec3::new(0.5, -0.3, -1.0).normalize().to_array(),
            color: [1.0; 3],
            intensity: 3.0,
            range: 0.0,
            inner_deg: 0.0,
            outer_deg: 0.0,
            shadows: false,
        }]),
        cameras: Some(vec![]),
        environment: Some(EnvironmentView {
            sky: 1,
            sky_color: [0.2, 0.4, 0.9],
            ambient: 1.0,
            baked_gi: String::new(),
            neural_gi: String::new(),
            gi_intensity: 1.0,
            fog_density: 0.0,
            fog_color: [0.6, 0.7, 0.8],
            exposure_ev: 0.0,
            bloom: 0.0,
            color_lut: String::new(),
            color_lut_intensity: 1.0,
        }),
        ..RenderFrame::default()
    }
}

/// The views: close up over each quad (an orthographic 1 m: a texel a pixel, the layer's first
/// level), then all four from afar (24 m: about a sixteenth, the file's smaller levels).
fn views() -> Vec<(String, Vec3, f32)> {
    let mut v: Vec<(String, Vec3, f32)> = QUADS
        .iter()
        .map(|(name, x)| (format!("{name} close"), Vec3::new(*x + 0.3, 0.2, 0.0), 1.0))
        .collect();
    v.push(("far".into(), Vec3::ZERO, 24.0));
    v
}

/// `asset` drawn from each view, and the frame's count of BC7 layers.
fn frames(gpu: &Gpu, asset: &ModelAsset) -> (Vec<Vec<u8>>, u32) {
    let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
    r.set_antialiasing(Antialiasing::Off);
    r.set_gtao(Gtao::Off);
    r.add_model(MODEL, asset);
    r.apply(scene(), 0.0);
    let shots = views()
        .iter()
        .map(|(_, centre, height)| {
            let mut camera = CameraState::look_at(*centre + Vec3::Z * 10.0, *centre);
            camera.ortho_height = Some(*height);
            r.set_camera_override(Some(camera));
            r.capture_rgba(0.0).2
        })
        .collect();
    (shots, r.last.texture_blocks)
}

/// The share of pixels of which a color channel differs at all, and the largest difference.
fn differing(a: &[u8], b: &[u8]) -> (f64, u8) {
    assert_eq!(a.len(), b.len());
    let (mut n, mut max) = (0u32, 0u8);
    for (p, q) in a.chunks(4).zip(b.chunks(4)) {
        let d = (0..3).map(|k| p[k].abs_diff(q[k])).max().unwrap_or(0);
        max = max.max(d);
        n += u32::from(d > 0);
    }
    (f64::from(n) / f64::from(W * H), max)
}

#[test]
fn a_ktx2_textured_model_draws_alike_with_and_without_bc() {
    let choice = BackendChoice::from_env();
    let minimal = Minimal::from_env();
    let Ok(full) = Gpu::headless_with(choice, minimal) else {
        eprintln!("no GPU: skipped");
        return;
    };
    let decoded = Gpu::headless_with(
        choice,
        Minimal {
            texture_compression: true,
            ..minimal
        },
    )
    .expect("another device");
    assert!(!decoded.caps.texture_compression_bc);
    let bc = full.caps.texture_compression_bc;
    if !bc {
        eprintln!(
            "no texture-compression-bc: both devices decode on the CPU; BC7 layers not checked"
        );
    }
    let images = images();
    // Where each image goes (materials.rs `placement`).
    let places: Vec<Placement> = images.iter().map(|i| placement(i, bc)).collect();
    let fits = if bc {
        Placement::Blocks(0)
    } else {
        Placement::Levels(0)
    };
    assert_eq!(
        places,
        [fits, fits, Placement::Levels(0), Placement::Resize],
        "bc: {bc}"
    );
    let asset = model(images.clone());
    let (with, blocks) = frames(&full, &asset);
    let (without, none) = frames(&decoded, &asset);
    // The base color and the normal map of `bc7`, each a BC7 layer of its own kind.
    assert_eq!(blocks, if bc { 2 } else { 0 });
    assert_eq!(none, 0);
    // The same images handed in decoded, as a PNG is: level 0 resized (here not at all) and the
    // mips drawn from it on the GPU.
    let plain = model(
        images
            .into_iter()
            .map(|i| ImageData { levels: None, ..i })
            .collect(),
    );
    let (reference, _) = frames(&decoded, &plain);
    for (v, (view, _, _)) in views().iter().enumerate() {
        let pixels = with[v].chunks(4).collect::<Vec<_>>();
        assert!(
            pixels.iter().any(|p| *p != pixels[0]),
            "{view}: the frame is flat"
        );
        let (share, max) = differing(&with[v], &without[v]);
        eprintln!(
            "{view}: BC7 as stored against decoded: {:.3}% of the pixels differ, by at most {max}",
            share * 100.0
        );
        assert!(
            max <= 1,
            "{view}: the BC7 layers draw {:.3}% of the pixels other than the decoded ones, by up \
             to {max}",
            share * 100.0
        );
        if view.ends_with("close") {
            let (share, max) = differing(&with[v], &reference[v]);
            eprintln!(
                "{view}: against the decoded image as a PNG: {:.3}% differ, by at most {max}",
                share * 100.0
            );
            assert!(
                max <= 1,
                "{view}: the layer's first level differs from the decoded image's on {:.3}% of \
                 the pixels, by up to {max}",
                share * 100.0
            );
        }
    }
}
