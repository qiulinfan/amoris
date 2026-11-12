//! Game UI (docs/spec/ui.md): text, bars, images, buttons and stacks anchored on the screen or over
//! entities, laid out on the CPU each frame into quads and drawn over the final image. Glyphs of
//! the embedded DejaVu Sans Bold and of any project font are rasterized on demand (fontdue) into
//! one shelf-packed atlas, cached per font, character and size; project fonts and images arrive
//! through the renderer's asset source. The presenter's pointer hovers and holds buttons here; the
//! last frame's button rectangles answer its hit tests.

use std::collections::{HashMap, HashSet};

use amoris_assets::frame::{UiKind, UiView};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

use crate::shaders;

const ATLAS: u32 = 1024;
const FONT: &[u8] = include_bytes!("../fonts/DejaVuSans-Bold.ttf");
/// The embedded font's baseline below a line's top, as a share of its size.
const DEFAULT_BASELINE: f32 = 0.8;
/// A disabled button's face keeps this share of its alpha.
const DISABLED_ALPHA: f32 = 0.45;
/// Stacks nested deeper are not laid out.
const DEPTH: u32 = 32;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
    /// 0: coverage from the glyph atlas; 1: the bound image's colour.
    mode: f32,
}

#[derive(Clone, Copy)]
struct Glyph {
    uv: [f32; 4], // u0 v0 u1 v1
    w: f32,
    h: f32,
    xmin: f32,
    ymin: f32,
    advance: f32,
}

/// A project font or image, decoded (docs/spec/ui.md 5).
pub enum UiAsset {
    Font(Box<fontdue::Font>),
    Image {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
}

/// Whether a UI asset path names a font (`.ttf`, `.otf`, `.ttc`); anything else is an image.
pub fn is_font(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".ttf", ".otf", ".ttc"].iter().any(|e| lower.ends_with(e))
}

/// Decodes a UI asset's bytes: a font parsed, an image (PNG or JPEG) to RGBA8.
pub fn decode_asset(path: &str, bytes: &[u8]) -> Result<UiAsset, String> {
    if is_font(path) {
        return fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default())
            .map(|f| UiAsset::Font(Box::new(f)))
            .map_err(|e| format!("not a font: {e}"));
    }
    #[cfg(feature = "import")]
    {
        let (width, height, rgba) = amoris_assets::import::decode_image(bytes)?;
        Ok(UiAsset::Image {
            width,
            height,
            rgba,
        })
    }
    #[cfg(not(feature = "import"))]
    {
        let _ = bytes;
        Err("this build decodes no images (amoris-render's import feature)".into())
    }
}

enum FontState {
    Pending,
    /// Its index in `Ui::fonts`.
    Ready(u16),
    Failed,
}

struct ImageSlot {
    size: [u32; 2],
    bind: wgpu::BindGroup,
}

enum ImageState {
    Pending,
    Decoded(u32, u32, Vec<u8>),
    Ready(ImageSlot),
    Failed,
}

struct Font {
    font: fontdue::Font,
    /// The baseline below a line's top, as a share of its size.
    baseline: f32,
}

/// A rectangle in device pixels: x0, y0, x1, y1.
type Rect = [f32; 4];

/// Quads gathered for one frame, in runs of the same image.
#[derive(Default)]
struct Batch {
    verts: Vec<Vertex>,
    /// The image each run draws (`None`: the atlas), and where it starts.
    runs: Vec<(Option<String>, u32)>,
}

impl Batch {
    fn quad(&mut self, r: Rect, uv: [f32; 4], c: [f32; 4], image: Option<&str>) {
        let mode = if image.is_some() { 1.0 } else { 0.0 };
        if self.runs.last().is_none_or(|(i, _)| i.as_deref() != image) {
            self.runs
                .push((image.map(str::to_owned), self.verts.len() as u32));
        }
        let v = |x, y, u, vv| Vertex {
            pos: [x, y],
            uv: [u, vv],
            color: c,
            mode,
        };
        let [x0, y0, x1, y1] = r;
        self.verts.extend_from_slice(&[
            v(x0, y0, uv[0], uv[1]),
            v(x1, y0, uv[2], uv[1]),
            v(x1, y1, uv[2], uv[3]),
            v(x0, y0, uv[0], uv[1]),
            v(x1, y1, uv[2], uv[3]),
            v(x0, y1, uv[0], uv[3]),
        ]);
    }
}

/// The white block at the atlas origin, for bars and boxes.
const WHITE: [f32; 4] = [
    0.5 / ATLAS as f32,
    0.5 / ATLAS as f32,
    3.5 / ATLAS as f32,
    3.5 / ATLAS as f32,
];

/// This frame's entities with laid-out nodes (ui.md 3): their elements, the stacks, and each
/// stack's children.
struct Nodes {
    elements: HashMap<u64, Vec<usize>>,
    stacks: HashSet<u64>,
    laid: HashSet<u64>,
    children: HashMap<u64, Vec<u64>>,
}

impl Nodes {
    fn of(items: &[UiView]) -> Nodes {
        let mut elements: HashMap<u64, Vec<usize>> = HashMap::new();
        let mut stacks = HashSet::new();
        let mut laid = HashSet::new();
        for (i, it) in items.iter().enumerate() {
            elements.entry(it.id).or_default().push(i);
            match it.kind {
                UiKind::Stack => {
                    stacks.insert(it.id);
                    laid.insert(it.id);
                }
                UiKind::Button => {
                    laid.insert(it.id);
                }
                _ => {}
            }
        }
        for it in items {
            if it.parent != 0 && stacks.contains(&it.parent) {
                laid.insert(it.id);
            }
        }
        let mut children: HashMap<u64, Vec<u64>> = HashMap::new();
        for it in items {
            if laid.contains(&it.id)
                && it.parent != 0
                && it.parent != it.id
                && stacks.contains(&it.parent)
            {
                let c = children.entry(it.parent).or_default();
                if !c.contains(&it.id) {
                    c.push(it.id);
                }
            }
        }
        for c in children.values_mut() {
            c.sort_unstable();
        }
        Nodes {
            elements,
            stacks,
            laid,
            children,
        }
    }

    fn is_child(&self, it: &UiView) -> bool {
        it.parent != 0 && it.parent != it.id && self.stacks.contains(&it.parent)
    }
}

pub struct Ui {
    pipeline: wgpu::RenderPipeline,
    atlas: wgpu::Texture,
    view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    params: wgpu::Buffer,
    vbuf: Option<wgpu::Buffer>,
    /// The engine's font first, then the project's as they arrive.
    fonts: Vec<Font>,
    font_paths: HashMap<String, FontState>,
    images: HashMap<String, ImageState>,
    /// A white texel for the runs that draw no image.
    blank: Option<wgpu::BindGroup>,
    glyphs: HashMap<(u16, char, u32), Glyph>,
    shelf: (u32, u32, u32), // x, y, row height
    full: bool,
    pending: Vec<(u32, u32, u32, u32, Vec<u8>)>,
    /// Paths to ask the asset source for.
    requests: Vec<String>,
    pub items: Vec<UiView>,
    /// Device pixels per logical pixel.
    pub scale: f32,
    output_srgb: bool,
    /// The last frame's buttons, in draw order: entity, rectangle, enabled.
    hits: Vec<(u64, Rect, bool)>,
    pointer: Option<[f32; 2]>,
    pointer_down: bool,
}

impl Ui {
    pub fn new(device: &wgpu::Device, output: wgpu::TextureFormat) -> Ui {
        let m = shaders::module(device, "ui");
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("game ui"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &m,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &m,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: output,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas"),
            size: wgpu::Extent3d {
                width: ATLAS,
                height: ATLAS,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = atlas.create_view(&Default::default());
        let font = fontdue::Font::from_bytes(FONT, fontdue::FontSettings::default())
            .unwrap_or_else(|e| panic!("embedded font: {e}"));
        let mut ui = Ui {
            pipeline,
            atlas,
            view,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("glyphs"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            params: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ui params"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            vbuf: None,
            fonts: vec![Font {
                font,
                baseline: DEFAULT_BASELINE,
            }],
            font_paths: HashMap::new(),
            images: HashMap::new(),
            blank: None,
            glyphs: HashMap::new(),
            shelf: (4, 0, 0),
            full: false,
            pending: Vec::new(),
            requests: Vec::new(),
            items: Vec::new(),
            scale: 1.0,
            output_srgb: output.is_srgb(),
            hits: Vec::new(),
            pointer: None,
            pointer_down: false,
        };
        // A white block at the atlas origin for bars.
        ui.pending.push((0, 0, 4, 4, vec![255; 16]));
        ui
    }

    /// Notes the fonts and images `items` name that were never asked for; [`Ui::take_requests`]
    /// hands them to the asset source.
    pub fn want(&mut self, items: &[UiView]) {
        for it in items {
            if it.asset.is_empty() {
                continue;
            }
            let known = match it.kind {
                UiKind::Text => self.font_paths.contains_key(&it.asset),
                UiKind::Image => self.images.contains_key(&it.asset),
                _ => true,
            };
            if known {
                continue;
            }
            if it.kind == UiKind::Text {
                self.font_paths.insert(it.asset.clone(), FontState::Pending);
            } else {
                self.images.insert(it.asset.clone(), ImageState::Pending);
            }
            self.requests.push(it.asset.clone());
        }
    }

    pub fn take_requests(&mut self) -> Vec<String> {
        std::mem::take(&mut self.requests)
    }

    /// Fonts and images asked for that have not arrived.
    pub fn pending(&self) -> usize {
        self.font_paths
            .values()
            .filter(|s| matches!(s, FontState::Pending))
            .count()
            + self
                .images
                .values()
                .filter(|s| matches!(s, ImageState::Pending | ImageState::Decoded(..)))
                .count()
    }

    /// A loaded asset (or why it did not load: its texts then use the engine's font, its images
    /// are left out).
    pub fn deliver(&mut self, path: &str, asset: Result<UiAsset, String>) {
        match asset {
            Ok(UiAsset::Font(font)) => self.add_font_parsed(path, *font),
            Ok(UiAsset::Image {
                width,
                height,
                rgba,
            }) => {
                if width == 0 || height == 0 || rgba.len() != (width * height * 4) as usize {
                    log::warn!(
                        "ui image {path}: {width}x{height} with {} bytes",
                        rgba.len()
                    );
                    self.images.insert(path.to_owned(), ImageState::Failed);
                } else {
                    self.images
                        .insert(path.to_owned(), ImageState::Decoded(width, height, rgba));
                }
            }
            Err(e) => {
                log::warn!("ui asset {path}: {e}");
                if is_font(path) {
                    self.font_paths.insert(path.to_owned(), FontState::Failed);
                } else {
                    self.images.insert(path.to_owned(), ImageState::Failed);
                }
            }
        }
    }

    /// Adds a font under `path` from its file's bytes (tests and hosts that hold the file).
    pub fn add_font(&mut self, path: &str, bytes: &[u8]) -> Result<(), String> {
        match decode_asset(path, bytes)? {
            UiAsset::Font(f) => {
                self.add_font_parsed(path, *f);
                Ok(())
            }
            UiAsset::Image { .. } => Err(format!("{path} is not a font")),
        }
    }

    /// Adds an image under `path`, RGBA8 sRGB.
    pub fn add_image(&mut self, path: &str, width: u32, height: u32, rgba: Vec<u8>) {
        self.deliver(
            path,
            Ok(UiAsset::Image {
                width,
                height,
                rgba,
            }),
        );
    }

    fn add_font_parsed(&mut self, path: &str, font: fontdue::Font) {
        let baseline = font
            .horizontal_line_metrics(1.0)
            .filter(|m| m.ascent - m.descent > 0.0)
            .map_or(DEFAULT_BASELINE, |m| m.ascent / (m.ascent - m.descent));
        let Ok(index) = u16::try_from(self.fonts.len()) else {
            self.font_paths.insert(path.to_owned(), FontState::Failed);
            return;
        };
        self.fonts.push(Font { font, baseline });
        self.font_paths
            .insert(path.to_owned(), FontState::Ready(index));
    }

    /// Where the presenter's pointer is, in device pixels (`None`: not over the view or captured),
    /// and whether its primary button is down: the button under it draws hovered or held.
    pub fn set_pointer(&mut self, at: Option<[f32; 2]>, down: bool) {
        self.pointer = at;
        self.pointer_down = down;
    }

    /// The enabled button drawn topmost at `(x, y)` (device pixels) in the last frame; a disabled
    /// button on top answers none.
    pub fn hit(&self, x: f32, y: f32) -> Option<u64> {
        self.hits
            .iter()
            .rev()
            .find(|(_, r, _)| x >= r[0] && x < r[2] && y >= r[1] && y < r[3])
            .and_then(|(id, _, enabled)| enabled.then_some(*id))
    }

    /// The last frame's buttons: entity, rectangle in device pixels (x0, y0, x1, y1), enabled.
    pub fn buttons(&self) -> &[(u64, [f32; 4], bool)] {
        &self.hits
    }

    /// The font a text draws with: `None` while its file is on the way.
    fn font_of(&self, path: &str) -> Option<u16> {
        if path.is_empty() {
            return Some(0);
        }
        match self.font_paths.get(path) {
            Some(FontState::Ready(i)) => Some(*i),
            Some(FontState::Pending) | None => None,
            Some(FontState::Failed) => Some(0),
        }
    }

    fn glyph(&mut self, font: u16, c: char, px: u32) -> Glyph {
        if let Some(g) = self.glyphs.get(&(font, c, px)) {
            return *g;
        }
        let f = &self.fonts[usize::from(font)].font;
        let (m, bitmap) = f.rasterize(c, px as f32);
        let (w, h) = (m.width as u32, m.height as u32);
        let (mut x, mut y, mut row) = self.shelf;
        if x + w + 1 > ATLAS {
            x = 0;
            y += row + 1;
            row = 0;
        }
        let blank = Glyph {
            uv: [0.0; 4],
            w: 0.0,
            h: 0.0,
            xmin: 0.0,
            ymin: 0.0,
            advance: m.advance_width,
        };
        if w == 0 || h == 0 {
            self.glyphs.insert((font, c, px), blank);
            return blank;
        }
        if y + h + 1 > ATLAS {
            // Left out of this frame; the atlas is emptied after it (ui.md 5).
            self.full = true;
            return blank;
        }
        self.pending.push((x, y, w, h, bitmap));
        let a = ATLAS as f32;
        let g = Glyph {
            uv: [
                x as f32 / a,
                y as f32 / a,
                (x + w) as f32 / a,
                (y + h) as f32 / a,
            ],
            w: w as f32,
            h: h as f32,
            xmin: m.xmin as f32,
            ymin: m.ymin as f32,
            advance: m.advance_width,
        };
        x += w + 1;
        row = row.max(h);
        self.shelf = (x, y, row);
        self.glyphs.insert((font, c, px), g);
        g
    }

    fn text_width(&mut self, font: u16, text: &str, px: u32) -> f32 {
        text.chars().map(|c| self.glyph(font, c, px).advance).sum()
    }

    /// A line of text with its top-left at `(bx, by)`: a shadow `shadow` pixels down and right,
    /// then the text.
    #[allow(clippy::too_many_arguments)]
    fn draw_text(
        &mut self,
        batch: &mut Batch,
        font: u16,
        text: &str,
        px: u32,
        (bx, by): (f32, f32),
        color: [f32; 4],
        shadow: f32,
    ) {
        let baseline = by + px as f32 * self.fonts[usize::from(font)].baseline;
        for (dx, dy, col) in [
            (shadow, shadow, [0.0, 0.0, 0.0, color[3] * 0.65]),
            (0.0, 0.0, color),
        ] {
            let mut x = bx + dx;
            for c in text.chars() {
                let g = self.glyph(font, c, px);
                if g.w > 0.0 {
                    let x0 = (x + g.xmin).round();
                    let y1 = (baseline + dy - g.ymin).round();
                    batch.quad([x0, y1 - g.h, x0 + g.w, y1], g.uv, col, None);
                }
                x += g.advance;
            }
        }
    }

    /// An image's size in device pixels at scale `s` (ui.md 2: a side of 0 takes the image's own).
    fn image_size(&self, it: &UiView, s: f32) -> [f32; 2] {
        let own = match self.images.get(&it.asset) {
            Some(ImageState::Ready(slot)) => Some([slot.size[0] as f32, slot.size[1] as f32]),
            Some(ImageState::Decoded(w, h, _)) => Some([*w as f32, *h as f32]),
            _ => None,
        };
        let [w, h] = it.size;
        let [w, h] = match own {
            Some([iw, ih]) => match (w > 0.0, h > 0.0) {
                (true, true) => [w, h],
                (true, false) => [w, w * ih / iw.max(1.0)],
                (false, true) => [h * iw / ih.max(1.0), h],
                (false, false) => [iw, ih],
            },
            None => [w.max(0.0), h.max(0.0)],
        };
        [w * s, h * s]
    }

    fn text_px(it: &UiView, s: f32) -> u32 {
        (it.size[1] * s).round().max(4.0) as u32
    }

    /// One element's size in device pixels at scale `s`, `None` for a stack (its box is its
    /// children's).
    fn element_size(&mut self, it: &UiView, s: f32) -> Option<[f32; 2]> {
        match it.kind {
            UiKind::Stack => None,
            UiKind::Button | UiKind::Bar => Some([it.size[0] * s, it.size[1] * s]),
            UiKind::Image => Some(self.image_size(it, s)),
            UiKind::Text => {
                let px = Self::text_px(it, s);
                let w = match self.font_of(&it.asset) {
                    Some(f) => self.text_width(f, &it.text, px),
                    None => 0.0,
                };
                Some([w, px as f32])
            }
        }
    }

    /// A laid-out node's box in device pixels at scale `s` (ui.md 3).
    fn measure(
        &mut self,
        items: &[UiView],
        nodes: &Nodes,
        id: u64,
        s: f32,
        depth: u32,
    ) -> [f32; 2] {
        let mut size = [0.0f32; 2];
        let Some(own) = nodes.elements.get(&id) else {
            return size;
        };
        let mut stack = None;
        for &i in own {
            match self.element_size(&items[i], s) {
                Some([w, h]) => size = [size[0].max(w), size[1].max(h)],
                None => stack = Some(i),
            }
        }
        if let Some(i) = stack {
            let st = &items[i];
            let kids = nodes.children.get(&id).map_or(&[][..], Vec::as_slice);
            let (mut main, mut cross) = (0.0f32, 0.0f32);
            if depth < DEPTH {
                for (k, &c) in kids.iter().enumerate() {
                    let [w, h] = self.measure(items, nodes, c, s, depth + 1);
                    let (m, x) = if st.column { (h, w) } else { (w, h) };
                    main += m + if k > 0 { st.spacing * s } else { 0.0 };
                    cross = cross.max(x);
                }
            }
            let pad = 2.0 * st.padding * s;
            let (w, h) = if st.column {
                (cross + pad, main + pad)
            } else {
                (main + pad, cross + pad)
            };
            size = [
                size[0].max(w).max(st.size[0] * s),
                size[1].max(h).max(st.size[1] * s),
            ];
        }
        size
    }

    /// The colour a button's face draws in now (ui.md 2).
    fn button_color(&self, it: &UiView, hovered: Option<u64>) -> [f32; 4] {
        if !it.enabled {
            let c = it.color;
            return [c[0], c[1], c[2], c[3] * DISABLED_ALPHA];
        }
        let over = hovered == Some(it.id);
        if it.on || (over && self.pointer_down) {
            it.press
        } else if over {
            it.hover
        } else {
            it.color
        }
    }

    /// Draws one element centred in the box `r`.
    #[allow(clippy::too_many_arguments)]
    fn draw_centred(
        &mut self,
        batch: &mut Batch,
        it: &UiView,
        r: Rect,
        s: f32,
        hovered: Option<u64>,
        hits: &mut Vec<(u64, Rect, bool)>,
    ) {
        let (cx, cy) = ((r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5);
        let centred = |[w, h]: [f32; 2]| -> Rect {
            let (x0, y0) = ((cx - w * 0.5).round(), (cy - h * 0.5).round());
            [x0, y0, x0 + w, y0 + h]
        };
        match it.kind {
            UiKind::Stack => {
                if it.color[3] > 0.0 {
                    batch.quad(r, WHITE, it.color, None);
                }
            }
            UiKind::Button => {
                let b = centred([it.size[0] * s, it.size[1] * s]);
                batch.quad(b, WHITE, self.button_color(it, hovered), None);
                hits.push((it.id, b, it.enabled));
            }
            UiKind::Image => {
                let b = centred(self.image_size(it, s));
                if let Some(ImageState::Ready(_)) = self.images.get(&it.asset) {
                    batch.quad(b, [0.0, 0.0, 1.0, 1.0], it.color, Some(&it.asset));
                }
            }
            UiKind::Bar => {
                let b = centred([it.size[0] * s, it.size[1] * s]);
                self.draw_bar(batch, it, b, s);
            }
            UiKind::Text => {
                let Some(font) = self.font_of(&it.asset) else {
                    return;
                };
                let px = Self::text_px(it, s);
                let w = self.text_width(font, &it.text, px);
                let b = centred([w, px as f32]);
                self.draw_text(batch, font, &it.text, px, (b[0], b[1]), it.color, s);
            }
        }
    }

    fn draw_bar(&self, batch: &mut Batch, it: &UiView, r: Rect, s: f32) {
        batch.quad(r, WHITE, it.back, None);
        let inset = s.max(1.0);
        let fw = (r[2] - r[0] - 2.0 * inset) * it.fill.clamp(0.0, 1.0);
        if fw > 0.0 {
            batch.quad(
                [r[0] + inset, r[1] + inset, r[0] + inset + fw, r[3] - inset],
                WHITE,
                it.color,
                None,
            );
        }
    }

    /// Draws the laid-out node `id` in the box `r`: its own elements centred (a stack's background
    /// first), then its children (ui.md 3, 4).
    #[allow(clippy::too_many_arguments)]
    fn draw_node(
        &mut self,
        batch: &mut Batch,
        items: &[UiView],
        nodes: &Nodes,
        id: u64,
        r: Rect,
        s: f32,
        hovered: Option<u64>,
        hits: &mut Vec<(u64, Rect, bool)>,
        depth: u32,
    ) {
        let Some(own) = nodes.elements.get(&id) else {
            return;
        };
        let mut stack = None;
        for &i in own {
            let it = &items[i];
            if it.kind == UiKind::Stack {
                stack = Some(i);
            }
            self.draw_centred(batch, it, r, s, hovered, hits);
        }
        let Some(i) = stack else { return };
        if depth >= DEPTH {
            return;
        }
        let st = &items[i];
        let kids = nodes.children.get(&id).cloned().unwrap_or_default();
        let pad = st.padding * s;
        let (mut x, mut y) = (r[0] + pad, r[1] + pad);
        let (inner_w, inner_h) = (r[2] - r[0] - 2.0 * pad, r[3] - r[1] - 2.0 * pad);
        for c in kids {
            let [w, h] = self.measure(items, nodes, c, s, depth + 1);
            let across = |room: f32, size: f32| match st.align {
                0 => 0.0,
                2 => room - size,
                _ => (room - size) * 0.5,
            };
            let (x0, y0) = if st.column {
                ((x + across(inner_w, w)).round(), y.round())
            } else {
                (x.round(), (y + across(inner_h, h)).round())
            };
            self.draw_node(
                batch,
                items,
                nodes,
                c,
                [x0, y0, x0 + w, y0 + h],
                s,
                hovered,
                hits,
                depth + 1,
            );
            if st.column {
                y += h + st.spacing * s;
            } else {
                x += w + st.spacing * s;
            }
        }
    }

    /// The top-left of a box `(bw, bh)` at an element's anchor (offsets point inward from the
    /// right and bottom edges, right and down otherwise; `anchor: entity` centres it above the
    /// projected world point), or `None` behind the camera.
    fn place(
        it: &UiView,
        (bw, bh): (f32, f32),
        (w, h): (f32, f32),
        s: f32,
        view_proj: Mat4,
    ) -> Option<(f32, f32)> {
        if it.anchor == 9 {
            let p = view_proj * Vec3::from(it.world).extend(1.0);
            if p.w <= 0.0 {
                return None;
            }
            let ndc = p.truncate() / p.w;
            let sx = (ndc.x * 0.5 + 0.5) * w;
            let sy = (0.5 - ndc.y * 0.5) * h;
            return Some((sx - bw * 0.5 + it.offset[0] * s, sy - bh + it.offset[1] * s));
        }
        let (ax, ay) = ((it.anchor % 3) as f32 * 0.5, (it.anchor / 3) as f32 * 0.5);
        let sx = if ax == 1.0 { -1.0 } else { 1.0 };
        let sy = if ay == 1.0 { -1.0 } else { 1.0 };
        Some((
            ax * (w - bw) + it.offset[0] * s * sx,
            ay * (h - bh) + it.offset[1] * s * sy,
        ))
    }

    /// Creates the GPU textures of images decoded since the last frame.
    fn upload_images(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let layout = self.pipeline.get_bind_group_layout(1);
        let make = |w: u32, h: u32, rgba: &[u8], label: &str| -> wgpu::BindGroup {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 4),
                    rows_per_image: Some(h),
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
            let view = tex.create_view(&Default::default());
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                }],
            })
        };
        if self.blank.is_none() {
            self.blank = Some(make(1, 1, &[255; 4], "ui blank image"));
        }
        let decoded: Vec<String> = self
            .images
            .iter()
            .filter(|(_, s)| matches!(s, ImageState::Decoded(..)))
            .map(|(p, _)| p.clone())
            .collect();
        for path in decoded {
            if let Some(ImageState::Decoded(w, h, rgba)) = self.images.remove(&path) {
                let bind = make(w, h, &rgba, "ui image");
                self.images
                    .insert(path, ImageState::Ready(ImageSlot { size: [w, h], bind }));
            }
        }
    }

    /// Lays out and draws the UI over `output`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
        size: (u32, u32),
        view_proj: Mat4,
    ) {
        if self.items.is_empty() {
            self.hits.clear();
            return;
        }
        self.upload_images(device, queue);
        let (w, h) = (size.0 as f32, size.1 as f32);
        let s = self.scale;
        let hovered = self.pointer.and_then(|[x, y]| self.hit(x, y));
        let mut batch = Batch::default();
        let mut hits = Vec::new();
        let items = std::mem::take(&mut self.items);
        let nodes = Nodes::of(&items);
        let mut done: HashSet<u64> = HashSet::new();
        for it in &items {
            if nodes.laid.contains(&it.id) {
                // A laid-out root, drawn whole the first time one of its elements comes.
                if nodes.is_child(it) || !done.insert(it.id) {
                    continue;
                }
                let own = &nodes.elements[&it.id];
                let pick = |k: UiKind| own.iter().find(|&&i| items[i].kind == k).copied();
                let anchor = pick(UiKind::Stack)
                    .or_else(|| pick(UiKind::Button))
                    .unwrap_or(own[0]);
                let a = &items[anchor];
                let ts = if a.kind == UiKind::Stack && a.reference_height > 0.0 {
                    h / a.reference_height
                } else {
                    s
                };
                let [bw, bh] = self.measure(&items, &nodes, it.id, ts, 0);
                let Some((bx, by)) = Self::place(a, (bw, bh), (w, h), ts, view_proj) else {
                    continue;
                };
                let (bx, by) = (bx.round(), by.round());
                self.draw_node(
                    &mut batch,
                    &items,
                    &nodes,
                    it.id,
                    [bx, by, bx + bw, by + bh],
                    ts,
                    hovered,
                    &mut hits,
                    0,
                );
                continue;
            }
            // An element at its own anchor, as before stacks and buttons (ui.md 3).
            match it.kind {
                UiKind::Bar => {
                    let (bw, bh) = (it.size[0] * s, it.size[1] * s);
                    let Some((bx, by)) = Self::place(it, (bw, bh), (w, h), s, view_proj) else {
                        continue;
                    };
                    self.draw_bar(&mut batch, it, [bx, by, bx + bw, by + bh], s);
                }
                UiKind::Image => {
                    let [bw, bh] = self.image_size(it, s);
                    let Some((bx, by)) = Self::place(it, (bw, bh), (w, h), s, view_proj) else {
                        continue;
                    };
                    if let Some(ImageState::Ready(_)) = self.images.get(&it.asset) {
                        let (bx, by) = (bx.round(), by.round());
                        batch.quad(
                            [bx, by, bx + bw, by + bh],
                            [0.0, 0.0, 1.0, 1.0],
                            it.color,
                            Some(&it.asset),
                        );
                    }
                }
                UiKind::Text => {
                    let Some(font) = self.font_of(&it.asset) else {
                        continue;
                    };
                    let px = Self::text_px(it, s);
                    let bw = self.text_width(font, &it.text, px);
                    let Some((bx, by)) = Self::place(it, (bw, px as f32), (w, h), s, view_proj)
                    else {
                        continue;
                    };
                    self.draw_text(&mut batch, font, &it.text, px, (bx, by), it.color, s);
                }
                UiKind::Button | UiKind::Stack => {}
            }
        }
        self.items = items;
        self.hits = hits;
        for (x, y, gw, gh, data) in self.pending.drain(..) {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.atlas,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x, y, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                &data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(gw),
                    rows_per_image: Some(gh),
                },
                wgpu::Extent3d {
                    width: gw,
                    height: gh,
                    depth_or_array_layers: 1,
                },
            );
        }
        if self.full {
            // The next frame rasterizes what it draws again into an empty atlas (ui.md 5).
            self.full = false;
            self.glyphs.clear();
            self.shelf = (4, 0, 0);
        }
        if batch.verts.is_empty() {
            return;
        }
        let bytes = bytemuck::cast_slice::<Vertex, u8>(&batch.verts);
        if self
            .vbuf
            .as_ref()
            .is_none_or(|b| b.size() < bytes.len() as u64)
        {
            self.vbuf = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ui vertices"),
                size: (bytes.len() as u64).next_power_of_two(),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let Some(vb) = &self.vbuf else { return };
        queue.write_buffer(vb, 0, bytes);
        queue.write_buffer(
            &self.params,
            0,
            bytemuck::cast_slice(&[w, h, if self.output_srgb { 1.0 } else { 0.0 }, 0.0]),
        );
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ui"),
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let Some(blank) = &self.blank else { return };
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("game ui"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.set_vertex_buffer(0, vb.slice(..));
        let end = batch.verts.len() as u32;
        for (k, (image, start)) in batch.runs.iter().enumerate() {
            let stop = batch.runs.get(k + 1).map_or(end, |r| r.1);
            let bind = match image.as_deref().and_then(|p| self.images.get(p)) {
                Some(ImageState::Ready(slot)) => &slot.bind,
                _ => blank,
            };
            pass.set_bind_group(1, bind, &[]);
            pass.draw(*start..stop, 0..1);
        }
    }
}
