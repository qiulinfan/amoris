//! Compares two captures of the same frame (for example the same scene on Direct3D 12 and on
//! Vulkan, docs/bench/dx12.md) and prints a JSON summary: RMSE and maximum difference over the RGB
//! channels in 8-bit units, PSNR, and how many pixels differ by more than a few thresholds.
//!
//! `cargo run --release -p pocket-render --example image_diff -- A.png B.png [--heatmap OUT.png]
//!  [--gain G]`
//!
//! `--heatmap` writes the per-pixel maximum channel difference times `G` (default 8) as grey, so
//! a difference of 32 (out of 255) is already white.

use serde_json::json;

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let paths: Vec<String> = std::env::args()
        .skip(1)
        .take_while(|a| !a.starts_with("--"))
        .collect();
    let [a_path, b_path] = paths.as_slice() else {
        eprintln!("usage: image_diff A.png B.png [--heatmap OUT.png] [--gain G]");
        std::process::exit(2);
    };
    let load = |p: &str| {
        image::open(p)
            .unwrap_or_else(|e| panic!("{p}: {e}"))
            .to_rgba8()
    };
    let (a, b) = (load(a_path), load(b_path));
    if a.dimensions() != b.dimensions() {
        eprintln!(
            "sizes differ: {:?} and {:?}",
            a.dimensions(),
            b.dimensions()
        );
        std::process::exit(1);
    }
    let (w, h) = a.dimensions();
    let gain: f32 = arg("--gain").and_then(|s| s.parse().ok()).unwrap_or(8.0);
    let thresholds = [1u8, 2, 4, 8, 16, 32, 64];
    let mut over = [0u64; 7];
    let mut sum_sq = 0f64;
    let mut max = 0u8;
    let mut max_at = (0u32, 0u32);
    let mut mean_a = [0f64; 3];
    let mut mean_b = [0f64; 3];
    let mut heat = image::GrayImage::new(w, h);
    for (x, y, pa) in a.enumerate_pixels() {
        let pb = b.get_pixel(x, y);
        let mut worst = 0u8;
        for c in 0..3 {
            let d = pa[c].abs_diff(pb[c]);
            sum_sq += f64::from(d) * f64::from(d);
            worst = worst.max(d);
            mean_a[c] += f64::from(pa[c]);
            mean_b[c] += f64::from(pb[c]);
        }
        if worst > max {
            max = worst;
            max_at = (x, y);
        }
        for (t, n) in thresholds.iter().zip(over.iter_mut()) {
            if worst > *t {
                *n += 1;
            }
        }
        heat.put_pixel(
            x,
            y,
            image::Luma([(f32::from(worst) * gain).min(255.0) as u8]),
        );
    }
    let pixels = f64::from(w) * f64::from(h);
    let mse = sum_sq / (pixels * 3.0);
    let rmse = mse.sqrt();
    let psnr = (mse > 0.0).then(|| 10.0 * (255.0f64 * 255.0 / mse).log10());
    let percent = |n: u64| (n as f64 / pixels * 1e4).round() / 1e2;
    let report = json!({
        "a": a_path,
        "b": b_path,
        "width": w,
        "height": h,
        "rmse_8bit": (rmse * 1e3).round() / 1e3,
        "psnr_db": psnr.map(|p| (p * 1e2).round() / 1e2),
        "max_diff_8bit": max,
        "max_diff_at": [max_at.0, max_at.1],
        "mean_rgb_a": mean_a.map(|v| (v / pixels * 1e2).round() / 1e2),
        "mean_rgb_b": mean_b.map(|v| (v / pixels * 1e2).round() / 1e2),
        "pixels_over_percent": thresholds
            .iter()
            .zip(over)
            .map(|(t, n)| (format!(">{t}"), json!(percent(n))))
            .collect::<serde_json::Map<_, _>>(),
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report).unwrap_or_default()
    );
    if let Some(path) = arg("--heatmap") {
        heat.save(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        eprintln!("wrote {path}");
    }
}
