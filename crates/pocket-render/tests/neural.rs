//! Neural textures on the GPU (docs/spec/neural-textures.md 7): the trainer learns, is
//! deterministic for a seed, and the runtime decoder (neural_texture.wgsl, f32 and f16) agrees
//! with the CPU reference (`pocket_assets::neural::Decoder`) on every texel of every mip.
//!
//! Skips without a GPU.

use pocket_assets::neural::{
    Channel, Decoder, GridSpec, NeuralLayout, NeuralTexture, Sampling, full_mip_count, mip_size,
};
use pocket_render::neural::decode_on_gpu;
use pocket_render::neural::train::{Reference, TrainConfig, Trainer};
use pocket_render::{BackendChoice, Gpu};

fn gpu() -> Option<Gpu> {
    match Gpu::headless(BackendChoice::from_env()) {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("skipped: no GPU ({e})");
            None
        }
    }
}

fn layout(sampling: Sampling) -> NeuralLayout {
    NeuralLayout {
        channels: vec![
            Channel::BaseR,
            Channel::BaseG,
            Channel::BaseB,
            Channel::NormalX,
            Channel::NormalY,
            Channel::Roughness,
        ],
        fine: GridSpec {
            features: 8,
            bits: 8,
        },
        coarse: GridSpec {
            features: 16,
            bits: 4,
        },
        fine_shift: 2,
        sampling,
        pe_octaves: 2,
        hidden: [16, 16],
    }
}

/// A 64x64 tile: soft stripes of colour, a bump, roughness following the stripes.
fn reference() -> Reference {
    let (w, h) = (64u32, 64u32);
    let mut v = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let u = x as f32 / w as f32 * std::f32::consts::TAU;
            let t = y as f32 / h as f32 * std::f32::consts::TAU;
            let stripe = 0.5 + 0.5 * (2.0 * u + t).sin();
            v.extend([
                0.2 + 0.6 * stripe,
                0.3 + 0.2 * t.cos(),
                0.5 - 0.3 * stripe,
                0.5 + 0.3 * u.cos(),
                0.5 + 0.3 * (2.0 * t).sin(),
                0.3 + 0.5 * stripe,
            ]);
        }
    }
    Reference::new(
        w,
        h,
        layout(Sampling::Bilinear).channels,
        v,
        full_mip_count(w, h),
    )
    .unwrap()
}

fn config(steps: u32, seed: u32) -> TrainConfig {
    TrainConfig {
        steps,
        batch: 4096,
        seed,
        ..TrainConfig::default()
    }
}

fn train(gpu: &Gpu, sampling: Sampling, steps: u32, seed: u32) -> (NeuralTexture, Vec<f32>) {
    let r = reference();
    let mut t = Trainer::new(&gpu.device, &r, layout(sampling), config(steps, seed)).unwrap();
    t.run(&gpu.device, &gpu.queue, steps, &mut |_, _| {})
        .unwrap();
    let history = t.history(&gpu.device, &gpu.queue).unwrap();
    (t.finish(&gpu.device, &gpu.queue, "test").unwrap(), history)
}

fn psnr(a: &[f32], b: &[f32]) -> f64 {
    let mse = a
        .iter()
        .zip(b)
        .map(|(x, y)| f64::from(x.clamp(0.0, 1.0) - y).powi(2))
        .sum::<f64>()
        / a.len() as f64;
    10.0 * (1.0 / mse.max(1e-12)).log10()
}

#[test]
fn trains_and_decodes_like_the_reference() {
    let Some(gpu) = gpu() else { return };
    for sampling in [Sampling::Bilinear, Sampling::Taps4] {
        let (texture, history) = train(&gpu, sampling, 600, 3);
        let early = history[..20].iter().sum::<f32>() / 20.0;
        let late = history[history.len() - 20..].iter().sum::<f32>() / 20.0;
        assert!(late < early * 0.2, "{sampling:?}: loss {early} -> {late}");
        let reference = reference();
        let mip0 = &reference.mips[0];
        let cpu = Decoder::new(&texture);
        let gpu32 = decode_on_gpu(&gpu.device, &gpu.queue, &texture, false).unwrap();
        let gpu16 = gpu
            .caps
            .shader_f16
            .then(|| decode_on_gpu(&gpu.device, &gpu.queue, &texture, true).unwrap());
        let c = texture.layout.channels.len();
        let mut worst32 = 0.0f32;
        let mut worst16 = 0.0f32;
        for mip in 0..texture.mip_count {
            let [w, h] = mip_size(texture.width, texture.height, mip);
            for y in 0..h {
                for x in 0..w {
                    let want = cpu.decode_texel(mip, x, y);
                    let at = ((y * w + x) as usize) * c;
                    for (ch, v) in want.iter().enumerate() {
                        worst32 = worst32.max((gpu32[mip as usize][at + ch] - v).abs());
                        if let Some(g) = &gpu16 {
                            worst16 = worst16.max((g[mip as usize][at + ch] - v).abs());
                        }
                    }
                }
            }
        }
        assert!(
            worst32 < 1e-3,
            "{sampling:?}: f32 decoder differs by {worst32}"
        );
        if gpu16.is_some() {
            assert!(
                worst16 < 2e-2,
                "{sampling:?}: f16 decoder differs by {worst16}"
            );
        }
        let quality = psnr(&gpu32[0], mip0);
        assert!(quality > 28.0, "{sampling:?}: mip 0 at {quality:.1} dB");
        eprintln!(
            "{sampling:?}: loss {early:.4} -> {late:.5}, mip 0 {quality:.1} dB, \
             decoder vs reference f32 {worst32:.2e}, f16 {worst16:.2e}"
        );
    }
}

#[test]
fn training_is_deterministic_for_a_seed() {
    let Some(gpu) = gpu() else { return };
    let (a, ha) = train(&gpu, Sampling::Bilinear, 60, 11);
    let (b, hb) = train(&gpu, Sampling::Bilinear, 60, 11);
    assert_eq!(ha, hb, "the loss history differs between runs of one seed");
    assert!(
        a.to_bytes() == b.to_bytes(),
        "the files differ between runs of one seed"
    );
    let (c, _) = train(&gpu, Sampling::Bilinear, 60, 12);
    assert!(
        a.to_bytes() != c.to_bytes(),
        "another seed gives the same file"
    );
}
