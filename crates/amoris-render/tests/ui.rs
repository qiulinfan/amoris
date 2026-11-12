//! The game UI drawn (docs/spec/ui.md 3 to 6), over a black sky: an image at its anchor with its
//! tint and at its own size; a column of buttons in a stack placed by its padding and spacing,
//! which the hit test answers; a row stack aligning its children; each button state's colour (face,
//! hover, held, on, disabled); a CJK line in a project font, which the engine's font draws as other
//! glyphs, and a text whose font is still on the way left out and counted as pending; a stack that
//! scales with the window's height. Skips when there is no GPU.

use amoris_assets::frame::{EnvironmentView, RenderFrame, UiKind, UiView};
use amoris_render::{BackendChoice, Gpu, Renderer};

const W: u32 = 320;
const H: u32 = 240;

fn frame(ui: Vec<UiView>) -> RenderFrame {
    RenderFrame {
        tick: 1,
        dt_s: 1.0 / 60.0,
        reset: true,
        cameras: Some(vec![]),
        lights: Some(vec![]),
        environment: Some(EnvironmentView {
            sky: 1,
            sky_color: [0.0; 3],
            ambient: 0.0,
            baked_gi: String::new(),
            neural_gi: String::new(),
            gi_intensity: 1.0,
            fog_density: 0.0,
            fog_color: [0.0; 3],
            exposure_ev: 0.0,
            bloom: 0.0,
        }),
        ui: Some(ui),
        ..RenderFrame::default()
    }
}

fn renderer(gpu: &Gpu, ui: Vec<UiView>) -> Renderer {
    let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
    r.apply(frame(ui), 0.0);
    r
}

fn px(img: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2], img[i + 3]]
}

fn near(a: [u8; 4], b: [u8; 3], tol: u8) -> bool {
    (0..3).all(|k| a[k].abs_diff(b[k]) <= tol)
}

fn gpu() -> Option<Gpu> {
    match Gpu::headless(BackendChoice::from_env()) {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("no GPU ({e}): skipped");
            None
        }
    }
}

/// An 8 x 8 image of four 4 x 4 quadrants: red, green / blue, white.
fn quadrants() -> Vec<u8> {
    let mut v = Vec::new();
    for y in 0..8 {
        for x in 0..8 {
            v.extend_from_slice(match (x < 4, y < 4) {
                (true, true) => &[255, 0, 0, 255],
                (false, true) => &[0, 255, 0, 255],
                (true, false) => &[0, 0, 255, 255],
                (false, false) => &[255, 255, 255, 255],
            });
        }
    }
    v
}

fn image(id: u64, size: [f32; 2], offset: [f32; 2], tint: [f32; 4]) -> UiView {
    UiView {
        id,
        kind: UiKind::Image,
        asset: "ui/quads.png".into(),
        size,
        color: tint,
        anchor: 0,
        offset,
        ..UiView::default()
    }
}

#[test]
fn an_image_draws_at_its_anchor_with_its_tint() {
    let Some(gpu) = gpu() else { return };
    let mut r = renderer(
        &gpu,
        vec![
            image(1, [80.0, 80.0], [10.0, 10.0], [1.0; 4]),
            image(2, [80.0, 80.0], [110.0, 10.0], [1.0, 1.0, 1.0, 0.5]),
            image(3, [0.0, 0.0], [210.0, 10.0], [1.0; 4]),
        ],
    );
    r.add_ui_image("ui/quads.png", 8, 8, quadrants());
    let (w, _, img) = r.capture_rgba(0.0);
    for (x, y, c) in [
        (30, 30, [255, 0, 0]),
        (70, 30, [0, 255, 0]),
        (30, 70, [0, 0, 255]),
        (70, 70, [255, 255, 255]),
    ] {
        assert!(near(px(&img, w, x, y), c, 2), "({x}, {y}): {:?}", px(&img, w, x, y));
    }
    assert!(near(px(&img, w, 5, 5), [0, 0, 0], 2), "outside: {:?}", px(&img, w, 5, 5));
    // Half alpha over black: the red half as bright in linear light (186 in sRGB).
    let half = px(&img, w, 130, 30);
    assert!(near(half, [188, 0, 0], 4), "tinted: {half:?}");
    // Its own size: 8 x 8 pixels at (210, 10).
    assert!(near(px(&img, w, 211, 11), [255, 0, 0], 2));
    assert!(near(px(&img, w, 217, 17), [255, 255, 255], 2));
    assert!(near(px(&img, w, 219, 19), [0, 0, 0], 2), "past its own size");
}

fn button(id: u64, parent: u64, size: [f32; 2]) -> UiView {
    UiView {
        id,
        kind: UiKind::Button,
        parent,
        size,
        color: [1.0, 0.0, 0.0, 1.0],
        hover: [0.0, 1.0, 0.0, 1.0],
        press: [0.0, 0.0, 1.0, 1.0],
        enabled: true,
        anchor: 4,
        ..UiView::default()
    }
}

fn stack(id: u64, column: bool, align: u32, offset: [f32; 2]) -> UiView {
    UiView {
        id,
        kind: UiKind::Stack,
        color: [0.2, 0.2, 0.2, 1.0],
        anchor: 0,
        offset,
        column,
        align,
        spacing: 6.0,
        padding: 10.0,
        ..UiView::default()
    }
}

#[test]
fn stacks_lay_out_their_buttons_and_the_hit_test_answers() {
    let Some(gpu) = gpu() else { return };
    let mut items = vec![stack(10, true, 1, [20.0, 20.0])];
    for id in [11, 12, 13] {
        items.push(button(id, 10, [100.0, 30.0]));
    }
    // A row aligned to its end: a short button sits at the bottom beside a tall one.
    items.push(stack(20, false, 2, [160.0, 20.0]));
    items.push(button(21, 20, [40.0, 60.0]));
    items.push(button(22, 20, [40.0, 20.0]));
    let mut r = renderer(&gpu, items);
    let (w, _, img) = r.capture_rgba(0.0);
    let rects: Vec<(u64, [f32; 4])> = r.ui_buttons().iter().map(|(id, b, _)| (*id, *b)).collect();
    assert_eq!(
        rects,
        vec![
            (11, [30.0, 30.0, 130.0, 60.0]),
            (12, [30.0, 66.0, 130.0, 96.0]),
            (13, [30.0, 102.0, 130.0, 132.0]),
            (21, [170.0, 30.0, 210.0, 90.0]),
            (22, [216.0, 70.0, 256.0, 90.0]),
        ]
    );
    assert_eq!(r.ui_hit(80.0, 80.0), Some(12));
    assert_eq!(r.ui_hit(80.0, 63.0), None, "between buttons");
    assert_eq!(r.ui_hit(5.0, 5.0), None);
    // The column's background around its buttons: 120 x 122 at (20, 20).
    let grey = px(&img, w, 22, 22);
    assert!(grey[0] > 40 && grey[0] == grey[1], "background: {grey:?}");
    assert!(near(px(&img, w, 22, 140), grey[..3].try_into().unwrap(), 1));
    assert!(near(px(&img, w, 22, 143), [0, 0, 0], 2), "below the box");
    assert!(near(px(&img, w, 80, 45), [255, 0, 0], 2), "a face");
    assert!(near(px(&img, w, 80, 63), grey[..3].try_into().unwrap(), 1), "the spacing");
}

#[test]
fn a_buttons_states_draw_their_colours() {
    let Some(gpu) = gpu() else { return };
    let at = |r: &mut Renderer| {
        let (w, _, img) = r.capture_rgba(0.0);
        px(&img, w, W / 2, H / 2)
    };
    let mut r = renderer(&gpu, vec![button(1, 0, [100.0, 40.0])]);
    assert!(near(at(&mut r), [255, 0, 0], 2), "face");
    r.set_ui_pointer(Some([(W / 2) as f32, (H / 2) as f32]), false);
    assert!(near(at(&mut r), [0, 255, 0], 2), "hovered");
    r.set_ui_pointer(Some([(W / 2) as f32, (H / 2) as f32]), true);
    assert!(near(at(&mut r), [0, 0, 255], 2), "held");
    r.set_ui_pointer(Some([5.0, 5.0]), true);
    assert!(near(at(&mut r), [255, 0, 0], 2), "held elsewhere");
    r.set_ui_pointer(None, false);
    let mut on = button(1, 0, [100.0, 40.0]);
    on.on = true;
    r.apply(frame(vec![on]), 0.0);
    assert!(near(at(&mut r), [0, 0, 255], 2), "a toggle that is on");
    let mut off = button(1, 0, [100.0, 40.0]);
    off.enabled = false;
    r.apply(frame(vec![off]), 0.0);
    r.set_ui_pointer(Some([(W / 2) as f32, (H / 2) as f32]), true);
    // 45% of red over black in linear light, never hovered or held; no hit either.
    assert!(near(at(&mut r), [178, 0, 0], 4), "disabled: {:?}", at(&mut r));
    assert_eq!(r.ui_hit((W / 2) as f32, (H / 2) as f32), None);
}

fn text(id: u64, s: &str, font: &str) -> UiView {
    UiView {
        id,
        kind: UiKind::Text,
        text: s.into(),
        asset: font.into(),
        size: [32.0, 32.0],
        color: [1.0; 4],
        anchor: 0,
        offset: [10.0, 10.0],
        ..UiView::default()
    }
}

fn lit(img: &[u8]) -> usize {
    img.chunks(4).filter(|p| p[0] > 128).count()
}

const CJK: &str = "fonts/DroidSansFallback.ttf";

#[test]
fn a_cjk_line_draws_in_a_project_font() {
    let Some(gpu) = gpu() else { return };
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/scenes/ui-hud")
        .join(CJK);
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let font = fontdue::Font::from_bytes(bytes.as_slice(), fontdue::FontSettings::default()).unwrap();
    for c in ['中', '文', 'あ', 'ア', '한', '글'] {
        assert_ne!(font.lookup_glyph_index(c), 0, "{c} is in the font");
    }
    let shot = |items: Vec<UiView>, with_font: bool| {
        let mut r = renderer(&gpu, items);
        if with_font {
            r.add_ui_font(CJK, &bytes).unwrap();
        }
        let (_, _, img) = r.capture_rgba(0.0);
        (img, r.last.pending_assets)
    };
    let line = "中文かな한글";
    let (cjk, pending) = shot(vec![text(1, line, CJK)], true);
    assert_eq!(pending, 0);
    let (engine, _) = shot(vec![text(1, line, "")], false);
    assert!(lit(&cjk) > 400, "the glyphs draw: {}", lit(&cjk));
    assert_ne!(cjk, engine, "the engine's font has no such glyphs");
    // Each script alone draws.
    for c in ["中", "か", "한"] {
        let (img, _) = shot(vec![text(1, c, CJK)], true);
        assert!(lit(&img) > 60, "{c}: {}", lit(&img));
    }
    // A font not yet arrived: the text waits, counted as pending.
    let (waiting, pending) = shot(vec![text(1, line, "fonts/elsewhere.ttf")], false);
    assert_eq!(lit(&waiting), 0);
    assert_eq!(pending, 1);
}

#[test]
fn a_stack_with_a_reference_height_scales_with_the_window() {
    let Some(gpu) = gpu() else { return };
    let mut s = stack(1, true, 1, [0.0, 0.0]);
    s.padding = 0.0;
    s.reference_height = 120.0;
    let mut r = renderer(&gpu, vec![s, button(2, 1, [50.0, 20.0])]);
    let _ = r.capture_rgba(0.0);
    assert_eq!(r.ui_buttons()[0].1, [0.0, 0.0, 100.0, 40.0], "240 rows: twice");
    r.resize(W * 2, H * 2);
    let _ = r.capture_rgba(0.0);
    assert_eq!(r.ui_buttons()[0].1, [0.0, 0.0, 200.0, 80.0], "480 rows: four times");
}
