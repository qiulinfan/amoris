//! Neural texture materials in the forward pass (docs/spec/neural-textures.md 5, 7): a material
//! whose network outputs constants must draw like the inline material with those values, in half
//! and single precision, also as the second of two loaded textures with other channel sets; a
//! texture of another shape is refused; and a scene with no neural material must not change at all
//! when the renderer has one loaded. Skips without a GPU.

use pocket_assets::neural::{
    Channel, GridSpec, LatentGrid, LatentLevel, NeuralLayout, NeuralTexture, Sampling, f32_to_f16,
    full_mip_count, level_dims,
};
use pocket_render::neural::train::srgb_to_linear;
use pocket_render::{BackendChoice, Gpu, Renderer, demo};

/// Base color sRGB, a flat normal, occlusion 1, `roughness`, metallic 0.
fn orm(base: [f32; 3], roughness: f32) -> Vec<(Channel, f32)> {
    vec![
        (Channel::BaseR, base[0]),
        (Channel::BaseG, base[1]),
        (Channel::BaseB, base[2]),
        (Channel::NormalX, 0.5),
        (Channel::NormalY, 0.5),
        (Channel::Occlusion, 1.0),
        (Channel::Roughness, roughness),
        (Channel::Metallic, 0.0),
    ]
}

/// A texture whose every texel is `values`: zero weights, the values as output biases, latents
/// anything.
fn constant(values: &[(Channel, f32)]) -> NeuralTexture {
    let layout = NeuralLayout {
        channels: values.iter().map(|(c, _)| *c).collect(),
        fine: GridSpec {
            features: 8,
            bits: 8,
        },
        coarse: GridSpec {
            features: 8,
            bits: 8,
        },
        fine_shift: 2,
        sampling: Sampling::Bilinear,
        pe_octaves: 2,
        hidden: [32, 32],
    };
    let size = 64;
    let mips = full_mip_count(size, size);
    let levels = level_dims(&layout, size, size, mips)
        .iter()
        .map(|d| {
            let grid = |[w, h]: [u32; 2]| LatentGrid {
                width: w,
                height: h,
                words: (0..2 * w * h)
                    .map(|i| i.wrapping_mul(2_654_435_761))
                    .collect(),
            };
            LatentLevel {
                fine: grid(d.fine),
                coarse: grid(d.coarse),
            }
        })
        .collect();
    let mut weights = vec![0u16; layout.weight_count()];
    let [_, _, (_, b3)] = layout.offsets();
    for (i, (_, v)) in values.iter().enumerate() {
        weights[b3 + i] = f32_to_f16(*v);
    }
    NeuralTexture {
        name: "constant".into(),
        width: size,
        height: size,
        mip_count: mips,
        layout,
        levels,
        weights,
    }
}

fn shoot(
    gpu: &Gpu,
    frame: pocket_assets::RenderFrame,
    neural: Option<(&NeuralTexture, bool)>,
) -> Vec<u8> {
    let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, 320, 180);
    r.add_model(demo::NEURAL_GROUND, &demo::neural_ground_model(40.0, 8.0));
    if let Some((texture, f16)) = neural {
        r.set_neural_half_precision(f16);
        r.add_neural_texture("materials/constant.ntex", texture)
            .expect("the texture loads");
    }
    r.apply(frame, 0.0);
    r.set_camera_override(Some(demo::neural_camera(1)));
    let mut rgba = Vec::new();
    for _ in 0..3 {
        rgba = r.capture_rgba(1.0 / 60.0).2;
    }
    rgba
}

fn worst(a: &[u8], b: &[u8]) -> (u8, usize) {
    let mut worst = 0;
    let mut over = 0;
    for (p, q) in a.chunks(4).zip(b.chunks(4)) {
        let d = (0..3).map(|i| p[i].abs_diff(q[i])).max().unwrap();
        worst = worst.max(d);
        over += usize::from(d > 2);
    }
    (worst, over)
}

#[test]
fn a_constant_neural_material_draws_like_the_inline_one() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("skipped: no GPU");
        return;
    };
    let base = [0.8, 0.35, 0.1];
    let texture = constant(&orm(base, 0.6));
    let linear = base.map(srgb_to_linear);
    let inline = shoot(
        &gpu,
        demo::neural_scene("", [linear[0], linear[1], linear[2], 1.0], 0.6),
        None,
    );
    let precisions: &[bool] = if gpu.caps.shader_f16 {
        &[true, false]
    } else {
        &[false]
    };
    for &f16 in precisions {
        let neural = shoot(
            &gpu,
            demo::neural_scene("materials/constant.ntex", [1.0; 4], 0.6),
            Some((&texture, f16)),
        );
        let (d, over) = worst(&inline, &neural);
        eprintln!("f16 {f16}: worst difference {d}, {over} pixels over 2");
        assert!(
            d <= 3,
            "f16 {f16}: the neural material differs by up to {d}"
        );
    }
    // Another colour must show: the comparison is not between two default materials.
    let green = constant(&orm([0.1, 0.8, 0.1], 0.6));
    let other = shoot(
        &gpu,
        demo::neural_scene("materials/constant.ntex", [1.0; 4], 0.6),
        Some((&green, false)),
    );
    let (d, _) = worst(&inline, &other);
    assert!(
        d > 40,
        "a green neural material looks like the orange one ({d})"
    );
}

#[test]
fn a_loaded_neural_texture_leaves_other_materials_alone() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("skipped: no GPU");
        return;
    };
    let frame = || demo::neural_scene("", [0.6, 0.5, 0.4, 1.0], 0.5);
    let plain = shoot(&gpu, frame(), None);
    let texture = constant(&orm([0.2, 0.9, 0.3], 0.4));
    let with = shoot(&gpu, frame(), Some((&texture, gpu.caps.shader_f16)));
    assert!(plain == with, "an unused neural texture changed the frame");
}

#[test]
fn textures_with_other_channel_sets_share_the_pipelines() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("skipped: no GPU");
        return;
    };
    let base = [0.3, 0.55, 0.85];
    let linear = base.map(srgb_to_linear);
    let inline = shoot(
        &gpu,
        demo::neural_scene("", [linear[0], linear[1], linear[2], 1.0], 0.7),
        None,
    );
    // Nine channels (with height) first, then twelve (with emissive, black): both pad to three
    // four-vectors. The second texture sits after the first in the table.
    let mut nine = orm([0.9, 0.1, 0.1], 0.3);
    nine.push((Channel::Height, 0.5));
    let mut twelve = orm(base, 0.7);
    twelve.extend([
        (Channel::EmissiveR, 0.0),
        (Channel::EmissiveG, 0.0),
        (Channel::EmissiveB, 0.0),
        (Channel::Height, 0.2),
    ]);
    let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, 320, 180);
    r.add_model(demo::NEURAL_GROUND, &demo::neural_ground_model(40.0, 8.0));
    r.add_neural_texture("materials/nine.ntex", &constant(&nine))
        .expect("nine channels load");
    r.add_neural_texture("materials/twelve.ntex", &constant(&twelve))
        .expect("twelve channels share the profile");
    let refused = r.add_neural_texture("materials/eight.ntex", &constant(&orm(base, 0.7)));
    assert!(
        refused.is_err(),
        "eight channels pad to two vectors: another profile"
    );
    r.apply(
        demo::neural_scene("materials/twelve.ntex", [1.0; 4], 0.7),
        0.0,
    );
    r.set_camera_override(Some(demo::neural_camera(1)));
    let mut rgba = Vec::new();
    for _ in 0..3 {
        rgba = r.capture_rgba(1.0 / 60.0).2;
    }
    let (d, over) = worst(&inline, &rgba);
    eprintln!("second texture: worst difference {d}, {over} pixels over 2");
    assert!(
        d <= 3,
        "the second texture draws {d} off its inline equivalent"
    );
}
