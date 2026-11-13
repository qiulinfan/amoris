//! Draws a fixed set of game UI texts and bars without laid-out nodes, the elements every sample
//! had before stacks and buttons (docs/spec/ui.md 9), and saves each frame as a PNG:
//! `cargo run --release -p amoris-render --example ui_frames -- OUT_DIR`. The elements are read
//! from JSON carrying both forms of the feed's `UiView` (`bar` before 2026-11-12, `kind` after), so
//! the same file builds in an older tree: two builds' frames compared with the `image_diff`
//! example say whether the engine font's text and the bars draw as they did.

use amoris_assets::frame::{EnvironmentView, LightKindView, LightView, RenderFrame, UiView};
use amoris_render::{BackendChoice, CameraState, Gpu, Renderer};
use glam::Vec3;
use serde_json::{Value, json};

/// One element in both forms: a bar is `kind: "Bar"` and `bar: true`.
fn element(
    id: u64,
    text: &str,
    size: f32,
    anchor: u32,
    offset: [f32; 2],
    world: [f32; 3],
) -> Value {
    let bar = text.is_empty();
    json!({
        "id": id, "text": text, "fill": if bar { 0.62 } else { 0.0 },
        "bar": bar, "kind": if bar { "Bar" } else { "Text" },
        "size": if bar { [90.0, size] } else { [size, size] },
        "color": if bar { [0.35, 0.85, 0.35, 1.0] } else { [1.0, 0.92, 0.7, 1.0] },
        "back": [0.0, 0.0, 0.0, 0.6],
        "anchor": anchor, "offset": offset, "world": world,
    })
}

fn items() -> Vec<UiView> {
    let mut v = Vec::new();
    let mut id = 1;
    for (anchor, text, size) in [
        (0, "Top left: Score 1,250", 20.0),
        (1, "Top centre: Wave 3 of 7", 24.0),
        (2, "Right 9", 15.0),
        (3, "Left, middle", 18.0),
        (4, "+", 24.0),
        (5, "Ammo 30/90", 22.0),
        (6, "Press E to open the gate", 16.0),
        (7, "Bottom centre: àéîõü ß Ω ‰", 20.0),
        (8, "Bottom right", 28.0),
    ] {
        v.push(element(id, text, size, anchor, [16.0, 16.0], [0.0; 3]));
        id += 1;
    }
    for (x, label) in [(-2.0, "Hero 1"), (0.0, "Hero 2  ×1.0"), (2.0, "Hero 3")] {
        v.push(element(id, label, 15.0, 9, [0.0, -12.0], [x, 2.05, 0.0]));
        v.push(element(id, "", 7.0, 9, [0.0, 0.0], [x, 2.2, 0.0]));
        id += 1;
    }
    v.push(element(id, "", 12.0, 0, [16.0, 48.0], [0.0; 3]));
    v.into_iter()
        .map(|e| serde_json::from_value(e).unwrap_or_else(|e| panic!("UiView: {e}")))
        .collect()
}

fn main() {
    let out = std::path::PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "out/ui-frames".into()),
    );
    std::fs::create_dir_all(&out).unwrap_or_else(|e| panic!("{}: {e}", out.display()));
    let gpu = Gpu::headless(BackendChoice::from_env()).unwrap_or_else(|e| panic!("no GPU: {e}"));
    for (name, (w, h), scale) in [("1x", (960, 540), 1.0), ("2x", (1280, 720), 2.0)] {
        let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, w, h);
        r.set_ui_scale(scale);
        r.apply(
            RenderFrame {
                tick: 1,
                dt_s: 1.0 / 60.0,
                reset: true,
                cameras: Some(vec![]),
                lights: Some(vec![LightView {
                    id: 0,
                    kind: LightKindView::Directional,
                    position: [0.0; 3],
                    direction: Vec3::new(0.3, -0.8, -0.5).normalize().to_array(),
                    color: [1.0; 3],
                    intensity: 3.0,
                    range: 0.0,
                    inner_deg: 0.0,
                    outer_deg: 0.0,
                    shadows: false,
                }]),
                environment: Some(EnvironmentView {
                    sky: 1,
                    sky_color: [0.25, 0.35, 0.5],
                    ambient: 0.5,
                    baked_gi: String::new(),
                    neural_gi: String::new(),
                    gi_intensity: 1.0,
                    fog_density: 0.0,
                    fog_color: [0.5; 3],
                    exposure_ev: 0.0,
                    bloom: 0.0,
                    color_lut: String::new(),
                    color_lut_intensity: 1.0,
                }),
                ui: Some(items()),
                ..RenderFrame::default()
            },
            0.0,
        );
        r.set_camera_override(Some(CameraState::look_at(
            Vec3::new(0.0, 2.0, 6.0),
            Vec3::new(0.0, 1.5, 0.0),
        )));
        let (cw, ch, px) = r.capture_rgba(0.0);
        let path = out.join(format!("ui-{name}.png"));
        image::save_buffer(&path, &px, cw, ch, image::ColorType::Rgba8)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        println!("{}", path.display());
    }
}
