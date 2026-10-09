//! Neural texture materials in the forward pass (docs/spec/neural-textures.md 5, 7): a material
//! whose network outputs constants must draw like the inline material with those values, in half
//! and single precision, on WebGPU's baseline (no optional features, default limits), and as the
//! second of two loaded textures with other channel sets; a texture of another shape is refused;
//! and a scene with no neural material must not change at all when the renderer has one loaded.
//! A probe network whose output depends on the mip and the latents (not on the position) checks
//! the level of detail, the blend between mips and the latent fetches of `fs_neural` against the
//! CPU reference decoder at known fractional levels of detail, also after the latent texture grew.
//! Material names that are not ASCII must not crash the renderer. Skips without a GPU.

use pocket_assets::RenderFrame;
use pocket_assets::neural::{
    Channel, Decoder, GridSpec, LatentGrid, LatentLevel, NeuralLayout, NeuralTexture, Sampling,
    f32_to_f16, full_mip_count, level_dims,
};
use pocket_render::gpu::Minimal;
use pocket_render::neural::train::srgb_to_linear;
use pocket_render::{BackendChoice, CameraState, Gpu, Renderer, demo};

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

/// The default profile's shape (8x8-bit grids at a quarter resolution, bilinear, two octaves, a
/// 32-32 network) with `channels`.
fn layout(channels: Vec<Channel>) -> NeuralLayout {
    NeuralLayout {
        channels,
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
    }
}

/// A texture whose every texel is `values`: zero weights, the values as output biases, latents
/// anything.
fn constant(values: &[(Channel, f32)]) -> NeuralTexture {
    let layout = layout(values.iter().map(|(c, _)| *c).collect());
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

/// The probe's roughness output (a constant).
const PROBE_ROUGHNESS: f32 = 0.6;

/// A texture of `size` whose decode does not depend on the position, only on the mip `m` (level
/// `l = m / 2`) and the latents, so every pixel of a frame at one level of detail shows the same
/// material: base color red `0.15 + 0.6 (m & 1)` and green `0.1 + 1.5 l / 8` (the network's two
/// level-of-detail inputs), blue `0.05 + 0.6 f + 0.3 c`, where `f` and `c` are feature 0 of level
/// `l`'s fine and coarse grids. Every texel of a grid holds the same feature 0 (`fine[l]`,
/// `coarse[l]`, of 255); the other features are noise the network ignores.
fn probe(size: u32, fine: &[u8], coarse: &[u8]) -> NeuralTexture {
    let mut outputs = orm([0.15, 0.1, 0.05], PROBE_ROUGHNESS);
    let layout = layout(outputs.iter().map(|(c, _)| *c).collect());
    let mips = full_mip_count(size, size);
    let dims = level_dims(&layout, size, size, mips);
    assert!(dims.len() <= fine.len() && dims.len() <= coarse.len());
    let levels = dims
        .iter()
        .enumerate()
        .map(|(l, d)| {
            let grid = |[w, h]: [u32; 2], q: u8| LatentGrid {
                width: w,
                height: h,
                words: (0..w * h)
                    .flat_map(|i| {
                        let noise = (i + 7919 * l as u32).wrapping_mul(2_654_435_761);
                        [noise & !0xff | u32::from(q), noise.rotate_left(13)]
                    })
                    .collect(),
            };
            LatentLevel {
                fine: grid(d.fine, fine[l]),
                coarse: grid(d.coarse, coarse[l]),
            }
        })
        .collect();
    let mut w = vec![0.0f32; layout.weight_count()];
    let [(w1, _), (w2, _), (w3, b3)] = layout.offsets();
    let inputs = layout.inputs_padded() as usize;
    let [h1, h2] = layout.hidden.map(|h| h as usize);
    // Hidden units 0 to 3 carry `m & 1`, `l / 8` and the two features 0 (all at least 0, so the
    // ReLUs pass them) through both hidden layers.
    let lod = layout.inputs() as usize - 2;
    let coarse0 = layout.fine_inputs() as usize;
    for (unit, input) in [(0, lod), (1, lod + 1), (2, 0), (3, coarse0)] {
        w[w1 + unit * inputs + input] = 1.0;
        w[w2 + unit * h1 + unit] = 1.0;
    }
    let out = |c: Channel| layout.output_of(c).expect("the probe has the channel");
    for (channel, unit, weight) in [
        (Channel::BaseR, 0, 0.6),
        (Channel::BaseG, 1, 1.5),
        (Channel::BaseB, 2, 0.6),
        (Channel::BaseB, 3, 0.3),
    ] {
        w[w3 + out(channel) * h2 + unit] = weight;
    }
    for (i, (_, bias)) in outputs.drain(..).enumerate() {
        w[b3 + i] = bias;
    }
    NeuralTexture {
        name: "probe".into(),
        width: size,
        height: size,
        mip_count: mips,
        layout,
        levels,
        weights: w.into_iter().map(f32_to_f16).collect(),
    }
}

/// What `fs_neural` must decode at level of detail `lod` (docs/spec/neural-textures.md 5), from
/// the CPU reference decoder: the level clamped to the mip chain; mip `m0 = floor(level)` alone,
/// or, within a quarter of a level around the transition, blended with `m0 + 1` by the position
/// in that band; the last mip alone.
fn expected(texture: &NeuralTexture, lod: f32) -> Vec<f32> {
    let decoder = Decoder::new(texture);
    let last = texture.mip_count - 1;
    let level = lod.clamp(0.0, last as f32);
    let m0 = level.floor() as u32;
    let t = ((level - m0 as f32 - 0.375) / 0.25).clamp(0.0, 1.0);
    // The probe decodes alike everywhere.
    let uv = [0.3, 0.6];
    let below = decoder.decode(m0, uv);
    if m0 == last {
        return below;
    }
    let above = decoder.decode(m0 + 1, uv);
    below
        .iter()
        .zip(&above)
        .map(|(a, b)| a * (1.0 - t) + b * t)
        .collect()
}

/// The orthographic view height of the level-of-detail frames, metres.
const LOD_VIEW: f32 = 8.0;
/// The side of their ground, metres (it fills the 16:9 view).
const LOD_SIDE: f32 = 16.0;
/// The frames' size.
const LOD_FRAME: [u32; 2] = [320, 180];

/// Straight down with an orthographic view: every pixel covers as much ground, so a texture's
/// level of detail is the same everywhere.
fn lod_camera() -> CameraState {
    CameraState {
        ortho_height: Some(LOD_VIEW),
        ..demo::neural_camera(0)
    }
}

/// Registers a ground under [`lod_camera`] on which a texture of `size` texels draws at level of
/// detail `lod` (`2^lod` texels a pixel), under [`ground_path`]. Each level of detail gets its own
/// ground rather than its own camera height, so the texture coordinates stay a fixed multiple of
/// their change per pixel and interpolation error cannot move the level.
fn lod_ground(r: &mut Renderer, size: u32, lod: f32) {
    let texels_per_metre = lod.exp2() * LOD_FRAME[1] as f32 / LOD_VIEW;
    let tiles = texels_per_metre * LOD_SIDE / size as f32;
    r.add_model(
        &ground_path(size, lod),
        &demo::neural_ground_model(LOD_SIDE, tiles),
    );
}

fn ground_path(size: u32, lod: f32) -> String {
    format!("demo/lod-{size}-{lod}.glb")
}

/// The neural scene with `material` on the ground registered as `ground`.
fn on_ground(material: &str, ground: &str, color: [f32; 4], roughness: f32) -> RenderFrame {
    let mut frame = demo::neural_scene(material, color, roughness);
    for instance in &mut frame.instances {
        if let Some(look) = &mut instance.look {
            look.mesh = ground.into();
        }
    }
    frame
}

fn draw(r: &mut Renderer, frame: RenderFrame) -> Vec<u8> {
    r.apply(frame, 0.0);
    r.set_camera_override(Some(lod_camera()));
    let mut rgba = Vec::new();
    for _ in 0..3 {
        rgba = r.capture_rgba(1.0 / 60.0).2;
    }
    rgba
}

/// The probe `texture` drawn as `material` at `lod` (on the ground [`lod_ground`] registered in
/// both renderers) against the inline material equal to its expected decode there, as (worst
/// channel difference, pixels over 2, the inline frame's centre pixel).
fn compare(
    neural: &mut Renderer,
    inline: &mut Renderer,
    material: &str,
    texture: &NeuralTexture,
    lod: f32,
) -> (u8, usize, [u8; 3]) {
    let ground = ground_path(texture.width, lod);
    let want = expected(texture, lod);
    let channel = |c: Channel| want[texture.layout.output_of(c).unwrap()].clamp(0.0, 1.0);
    let linear =
        [Channel::BaseR, Channel::BaseG, Channel::BaseB].map(|c| srgb_to_linear(channel(c)));
    let roughness = channel(Channel::Roughness);
    let color = [linear[0], linear[1], linear[2], 1.0];
    let a = draw(inline, on_ground("", &ground, color, roughness));
    let b = draw(neural, on_ground(material, &ground, [1.0; 4], roughness));
    let (d, over) = worst(&a, &b);
    let centre = 4 * (LOD_FRAME[0] * LOD_FRAME[1] / 2 + LOD_FRAME[0] / 2) as usize;
    (d, over, [a[centre], a[centre + 1], a[centre + 2]])
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

#[test]
fn a_constant_neural_material_draws_on_the_webgpu_baseline() {
    let choice = BackendChoice::from_env();
    // WebGPU's defaults: no optional features (no shader-f16, no indirect-first-instance: the
    // baseline draw path) and the default limits (8 storage buffers per stage, 64 KiB uniforms).
    let minimal = Minimal::parse("features,limits");
    let Ok(gpu) = Gpu::headless_with(choice, minimal) else {
        eprintln!("skipped: no GPU");
        return;
    };
    assert!(!gpu.caps.shader_f16 && !gpu.caps.indirect_first_instance);
    let base = [0.8, 0.35, 0.1];
    let linear = base.map(srgb_to_linear);
    let inline = shoot(
        &gpu,
        demo::neural_scene("", [linear[0], linear[1], linear[2], 1.0], 0.6),
        None,
    );
    let neural = shoot(
        &gpu,
        demo::neural_scene("materials/constant.ntex", [1.0; 4], 0.6),
        Some((&constant(&orm(base, 0.6)), true)),
    );
    let (d, over) = worst(&inline, &neural);
    eprintln!("baseline: worst difference {d}, {over} pixels over 2");
    assert!(
        d <= 3,
        "on the baseline the neural material differs by up to {d}"
    );
}

/// The probe's fine and coarse feature 0 per level: a different value per level and grid.
const FINE: [u8; 5] = [40, 200, 90, 250, 130];
const COARSE: [u8; 5] = [230, 20, 160, 70, 110];

/// Neighbouring mips of a probe decode at least `margin` apart in some base color channel, so
/// decoding the wrong mip, or blending two with the wrong weights, shows.
fn assert_mips_distinct(texture: &NeuralTexture, margin: f32) {
    let decoder = Decoder::new(texture);
    for m in 0..texture.mip_count - 1 {
        let (a, b) = (decoder.decode(m, [0.5; 2]), decoder.decode(m + 1, [0.5; 2]));
        let d = (0..3).map(|i| (a[i] - b[i]).abs()).fold(0.0, f32::max);
        assert!(
            d > margin,
            "mips {m} and {} of the probe decode alike",
            m + 1
        );
    }
}

#[test]
fn the_level_of_detail_selects_and_blends_mips_like_the_reference() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("skipped: no GPU");
        return;
    };
    let texture = probe(64, &FINE, &COARSE);
    assert_mips_distinct(&texture, 0.2);
    // Below mip 0; a blend near either end of the band (weights 0.3 and 0.7, so swapped weights
    // show); outside the band on either side; across a level (mips 1 and 2 read different
    // latent grids); the middle of the band; and beyond the last mip (64 texels: mips 0 to 6).
    let lods = [-0.5, 0.2, 0.45, 1.55, 2.8, 3.42, 4.6, 5.5, 6.4];
    let [w, h] = LOD_FRAME;
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut inline = Renderer::new(&gpu, format, w, h);
    let mut neural = Renderer::new(&gpu, format, w, h);
    neural
        .add_neural_texture("materials/probe.ntex", &texture)
        .expect("the probe loads");
    for &lod in &lods {
        lod_ground(&mut inline, 64, lod);
        lod_ground(&mut neural, 64, lod);
    }
    let precisions: &[bool] = if gpu.caps.shader_f16 {
        &[true, false]
    } else {
        &[false]
    };
    let mut failed = Vec::new();
    for &f16 in precisions {
        neural.set_neural_half_precision(f16);
        for &lod in &lods {
            let (d, over, centre) = compare(
                &mut neural,
                &mut inline,
                "materials/probe.ntex",
                &texture,
                lod,
            );
            eprintln!(
                "f16 {f16}, lod {lod}: worst difference {d}, {over} pixels over 2 ({centre:?})"
            );
            if d > 3 {
                failed.push((f16, lod, d));
            }
        }
    }
    assert!(
        failed.is_empty(),
        "(f16, level of detail, worst difference) off the reference: {failed:?}"
    );
}

#[test]
fn a_grown_latent_table_keeps_the_textures_it_holds() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("skipped: no GPU");
        return;
    };
    // The latent texture starts one row (4096 texels) high: a 64x64 probe fits in it, a 512x512
    // one (about 21,800 texels) does not, so loading the second copies the first into a texture
    // of eight rows and replaces what the forward pass binds.
    let small = probe(64, &FINE, &COARSE);
    let mut other = FINE;
    other.reverse();
    let large = probe(512, &COARSE, &other);
    assert_mips_distinct(&large, 0.2);
    let [w, h] = LOD_FRAME;
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut inline = Renderer::new(&gpu, format, w, h);
    let mut neural = Renderer::new(&gpu, format, w, h);
    for (size, lod) in [(64, 1.55), (512, 2.45), (512, 0.45)] {
        lod_ground(&mut inline, size, lod);
        lod_ground(&mut neural, size, lod);
    }
    neural
        .add_neural_texture("materials/small.ntex", &small)
        .expect("the small probe loads");
    let mut results = Vec::new();
    let mut check = |neural: &mut Renderer, what: &str, material: &str, t: &NeuralTexture, lod| {
        let (d, over, centre) = compare(neural, &mut inline, material, t, lod);
        eprintln!("{what}: worst difference {d}, {over} pixels over 2 ({centre:?})");
        results.push((what.to_owned(), d));
    };
    check(
        &mut neural,
        "small before",
        "materials/small.ntex",
        &small,
        1.55,
    );
    neural
        .add_neural_texture("materials/large.ntex", &large)
        .expect("the large probe loads");
    // The second texture after the growth (its latents only exist in the new texture), then the
    // first again (its latents were copied).
    check(&mut neural, "large", "materials/large.ntex", &large, 2.45);
    check(
        &mut neural,
        "large, mip 0",
        "materials/large.ntex",
        &large,
        0.45,
    );
    check(
        &mut neural,
        "small after",
        "materials/small.ntex",
        &small,
        1.55,
    );
    let failed: Vec<_> = results.iter().filter(|(_, d)| *d > 3).collect();
    assert!(failed.is_empty(), "drawn off the reference: {failed:?}");
}

#[test]
fn material_names_that_are_not_ascii_do_not_crash_the_renderer() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("skipped: no GPU");
        return;
    };
    let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, 64, 36);
    r.add_model(demo::NEURAL_GROUND, &demo::neural_ground_model(40.0, 8.0));
    // Every look's material passes the neural texture check; these end inside a character.
    for material in ["金属", "materials/红砖", "models/x.glb#屋顶"] {
        r.apply(demo::neural_scene(material, [1.0; 4], 0.5), 0.0);
        r.capture_rgba(1.0 / 60.0);
    }
}
