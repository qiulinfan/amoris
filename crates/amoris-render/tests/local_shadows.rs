//! Spot and point lights' shadow maps: the local shadow views (docs/spec/shadows.md; charter 4.4,
//! 2026-11-12). Under a perspective camera (the light clusters assume one), a flat sky's ambient
//! light and no sun unless said, read from the HDR image:
//!
//! - a spot aimed obliquely at a floor past a cube: in the cube's shadow the floor keeps only its
//!   ambient light (the frame with the spot's intensity 0), everywhere else on the floor the
//!   frame is the one drawn with `shadows` off, the cube's lit faces are not shadowed, and with
//!   no cube at all the shadowed spot changes no pixel (no acne anywhere in the cone, the floor a
//!   caster too); with one sample and with 4x MSAA;
//! - a point light in the spot's place does the same, the floor lit from straight down to 73
//!   degrees across its cube map's seams; a point light with a cube on each side and one below
//!   shadows the floor in five faces, and local shadows off draws its `shadows: false` frame;
//! - `shadows: false`, and `shadows: true` with local shadows off, draw the same frame to the bit,
//!   with the lists of a scene without the light; each view in use adds 4 bytes per list entry;
//! - casters cast by their opacity into a local view as into the cascades (an opaque cube and a
//!   blend of alpha 0.6 do, clear glass and a blend of alpha 0.35 do not, an alpha-masked quad
//!   casts its checker), and a transparent receiver is shadowed (the transparent stage's loop);
//! - with one spot more than [`LOCAL_VIEWS`] in view, the smallest on screen is left without a map
//!   and counted, whatever the order of the lights; equal ones are settled by the lower id; spots
//!   out of view take no view and are not counted;
//! - a ball does not shadow its own lit side (the receiver's offsets keep acne off a curve);
//! - the three draw paths of batches.rs draw the same local view;
//! - a local view draws each caster at the camera's level of detail;
//! - a spot's shadow stands beside a shadow-casting sun's, and under ray-traced sun shadows (no
//!   cascade drawn) the local view is still drawn.
//!
//! Runs on the device `AMORIS_BACKEND`, `AMORIS_ADAPTER` and `AMORIS_GPU_MINIMAL` describe. Skips
//! when there is no GPU.

use amoris_assets::frame::{
    EnvironmentView, InstanceUpdate, LightKindView, LightView, Look, Pose, RenderFrame,
};
use amoris_assets::mesh::{AlphaMode, ImageData, MaterialData, ModelAsset, NodeData};
use amoris_assets::primitives::primitive;
use amoris_render::gpu::Minimal;
use amoris_render::{
    Antialiasing, BackendChoice, CameraState, Gpu, Gtao, LOCAL_VIEWS, LocalShadows, LodMode,
    OcclusionMode, PrepassMode, Renderer, demo,
};
use glam::Vec3;

const W: u32 = 480;
const H: u32 = 270;
const MODEL: &str = "test/local.glb";

/// A unit quad (the plane primitive, facing +y) cut by a checker of 8 x 8 cells in its texture's
/// alpha: `MODEL#holes`.
fn model() -> ModelAsset {
    let size = 64u32;
    let mut rgba8 = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let on = ((x / 8) + (y / 8)) % 2 == 0;
            rgba8.extend_from_slice(&[255, 255, 255, if on { 255 } else { 0 }]);
        }
    }
    let mut mesh = primitive("plane").expect("the plane primitive");
    mesh.name = "holes".into();
    mesh.material = Some(0);
    ModelAsset {
        images: vec![ImageData {
            name: "holes".into(),
            width: size,
            height: size,
            rgba8,
            srgb: true,
            nearest: false,
        }],
        meshes: vec![mesh],
        materials: vec![MaterialData {
            name: "holes".into(),
            base_color: [0.8, 0.8, 0.8, 1.0],
            roughness: 1.0,
            base_color_texture: Some(0),
            alpha_mode: AlphaMode::Mask,
            double_sided: true,
            ..MaterialData::default()
        }],
        nodes: vec![NodeData {
            name: "holes".into(),
            mesh: 0,
            transform: glam::Mat4::IDENTITY.to_cols_array(),
            skin: None,
        }],
        ..ModelAsset::default()
    }
}

fn look(mesh: &str, color: [f32; 4], casts: bool) -> Look {
    Look {
        mesh: mesh.into(),
        material: String::new(),
        color,
        metallic: 0.0,
        roughness: 1.0,
        transmission: None,
        ior: None,
        emissive: [0.0; 3],
        cast_shadows: casts,
        visible: true,
    }
}

fn at(id: u64, position: Vec3, scale: Vec3, look: Look) -> InstanceUpdate {
    InstanceUpdate {
        id,
        pose: Some(Pose {
            position: position.to_array(),
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: scale.to_array(),
        }),
        look: Some(look),
        anim: None,
    }
}

const GREY: [f32; 4] = [0.8, 0.8, 0.8, 1.0];

/// A 40 m floor at y = 0 that casts shadows too (a map must not shadow its own caster).
fn floor(id: u64) -> InstanceUpdate {
    at(
        id,
        Vec3::ZERO,
        Vec3::new(40.0, 1.0, 40.0),
        look("plane", GREY, true),
    )
}

fn cube(id: u64, centre: Vec3, size: f32, color: [f32; 4]) -> InstanceUpdate {
    at(id, centre, Vec3::splat(size), look("cube", color, true))
}

/// A spot at `pos` aimed at `target`, its inner cone 10 degrees inside the outer.
fn spot(id: u64, pos: Vec3, target: Vec3, outer_deg: f32, range: f32, intensity: f32) -> LightView {
    LightView {
        id,
        kind: LightKindView::Spot,
        position: pos.to_array(),
        direction: (target - pos).normalize().to_array(),
        color: [1.0; 3],
        intensity,
        range,
        inner_deg: outer_deg - 10.0,
        outer_deg,
        shadows: true,
    }
}

fn unshadowed(l: &LightView) -> LightView {
    LightView {
        shadows: false,
        ..l.clone()
    }
}

fn dark(l: &LightView) -> LightView {
    LightView {
        intensity: 0.0,
        ..l.clone()
    }
}

/// A flat sky's light (ambient 0.3) and the lights given: no sun unless one is among them.
fn frame(instances: Vec<InstanceUpdate>, lights: Vec<LightView>) -> RenderFrame {
    RenderFrame {
        tick: 1,
        dt_s: 1.0 / 60.0,
        reset: true,
        instances,
        lights: Some(lights),
        cameras: Some(vec![]),
        environment: Some(EnvironmentView {
            sky: 1,
            sky_color: [0.35, 0.5, 0.75],
            ambient: 0.3,
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

/// A renderer drawing with `aa`, no GTAO, one opaque pass (no occlusion culling, no prepass) and
/// local shadows on whatever the environment says.
fn renderer(gpu: &Gpu, aa: Antialiasing, camera: &CameraState) -> Renderer {
    let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
    r.set_antialiasing(aa);
    r.set_gtao(Gtao::Off);
    r.set_occlusion(OcclusionMode::Off);
    r.set_prepass(PrepassMode::Off);
    r.set_local_shadows(LocalShadows::On);
    r.add_model(MODEL, &model());
    r.set_camera_override(Some(*camera));
    r
}

/// The HDR image of `f`.
fn shot(r: &mut Renderer, f: RenderFrame) -> Vec<f32> {
    r.apply(f, 0.0);
    r.capture_hdr(0.0).2
}

/// Where `p` is drawn, in pixels.
fn project(cam: &CameraState, p: Vec3) -> (f32, f32) {
    let clip = cam.proj(W as f32 / H as f32) * cam.view() * p.extend(1.0);
    assert!(clip.w > 0.0, "{p} is behind the camera");
    (
        (clip.x / clip.w * 0.5 + 0.5) * W as f32,
        (0.5 - clip.y / clip.w * 0.5) * H as f32,
    )
}

/// The direction of the camera's ray through (`x`, `y`) pixels.
fn ray(cam: &CameraState, x: f32, y: f32) -> Vec3 {
    let t = (cam.fov_y * 0.5).tan();
    let nx = (x / W as f32 * 2.0 - 1.0) * t * (W as f32 / H as f32);
    let ny = (1.0 - y / H as f32 * 2.0) * t;
    (cam.rotation * Vec3::new(nx, ny, -1.0)).normalize()
}

fn rgb(img: &[f32], x: u32, y: u32) -> [f32; 3] {
    let i = ((y * W + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2]]
}

fn luminance(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// The mean colour of the pixels within `r` pixels of where `p` is drawn.
fn patch(img: &[f32], cam: &CameraState, p: Vec3, r: i32) -> [f32; 3] {
    let (x, y) = project(cam, p);
    let (cx, cy) = (x as i32, y as i32);
    assert!(
        cx - r >= 0 && cy - r >= 0 && cx + r < W as i32 && cy + r < H as i32,
        "{p} is drawn at ({x}, {y}), off the image"
    );
    let mut sum = [0.0f32; 3];
    for py in cy - r..=cy + r {
        for px in cx - r..=cx + r {
            let c = rgb(img, px as u32, py as u32);
            for k in 0..3 {
                sum[k] += c[k];
            }
        }
    }
    let n = ((2 * r + 1) * (2 * r + 1)) as f32;
    sum.map(|s| s / n)
}

/// The largest difference between two colours' channels, relative to the brighter one's luminance.
fn off(a: [f32; 3], b: [f32; 3]) -> f32 {
    let scale = luminance(a).max(luminance(b)).max(1e-3);
    (0..3).map(|k| (a[k] - b[k]).abs()).fold(0.0, f32::max) / scale
}

fn assert_same(what: &str, got: [f32; 3], want: [f32; 3], tolerance: f32) {
    assert!(
        off(got, want) <= tolerance,
        "{what}: got {got:?}, expected {want:?} (off by {})",
        off(got, want)
    );
}

/// Where the ray from `o` along `d` (any length) enters the box `lo..hi`: the parameter, and the
/// axis of the face it enters by. `None` when it misses or the box is behind.
fn enters(o: Vec3, d: Vec3, lo: Vec3, hi: Vec3) -> Option<(f32, usize)> {
    let (mut near, mut far, mut axis) = (f32::NEG_INFINITY, f32::INFINITY, 0);
    for i in 0..3 {
        if d[i].abs() < 1e-9 {
            if o[i] < lo[i] || o[i] > hi[i] {
                return None;
            }
            continue;
        }
        let (a, b) = ((lo[i] - o[i]) / d[i], (hi[i] - o[i]) / d[i]);
        if a.min(b) > near {
            near = a.min(b);
            axis = i;
        }
        far = far.min(a.max(b));
    }
    (near <= far && far > 0.0).then_some((near, axis))
}

/// Whether the box `lo..hi` stands between `p` and `light`.
fn blocked(p: Vec3, light: Vec3, lo: Vec3, hi: Vec3) -> bool {
    enters(p, light - p, lo, hi).is_some_and(|(t, _)| t < 1.0)
}

// --- The spot, the cube and the floor -------------------------------------------------------------

/// The spot 6 m up, aimed at the floor 4 m to its side: light meets the floor from straight down
/// (under the spot) to about 68 degrees (the cone's far edge).
const LIGHT: Vec3 = Vec3::new(-3.0, 6.0, 0.0);
const AIM: Vec3 = Vec3::new(1.0, 0.0, 0.0);
/// A 1 m cube halfway along the spot's axis: its shadow lies around `AIM`.
const CUBE: Vec3 = Vec3::new(-1.0, 3.0, 0.0);
const CUBE_LO: Vec3 = Vec3::new(-1.5, 2.5, -0.5);
const CUBE_HI: Vec3 = Vec3::new(-0.5, 3.5, 0.5);
/// A floor point the spot lights and the cube does not shadow.
const BESIDE: Vec3 = Vec3::new(1.0, 0.0, -3.0);

/// From the spot's side and above: the floor around the shadow, the cube's top and its lit side.
fn eye() -> CameraState {
    CameraState::look_at(Vec3::new(-6.0, 7.0, 8.0), Vec3::new(0.0, 0.5, 0.0))
}

/// A shadowed point light at `pos`, as bright and as far-reaching as the spot.
fn point_light(pos: Vec3) -> LightView {
    LightView {
        id: 10,
        kind: LightKindView::Point,
        position: pos.to_array(),
        direction: [0.0; 3],
        color: [1.0; 3],
        intensity: 200.0,
        range: 20.0,
        inner_deg: 0.0,
        outer_deg: 0.0,
        shadows: true,
    }
}

fn the_spot() -> LightView {
    spot(10, LIGHT, AIM, 35.0, 20.0, 200.0)
}

fn spot_scene(light: LightView, with_cube: bool) -> RenderFrame {
    let mut v = vec![floor(1)];
    if with_cube {
        v.push(cube(2, CUBE, 1.0, GREY));
    }
    frame(v, vec![light])
}

/// What a pixel of the spot scene shows.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Shows {
    /// The floor where the cube hides the whole light, 12 cm inside the shadow's outline.
    Umbra,
    /// The floor at least 12 cm outside the shadow's outline (lit by the spot or beyond it).
    Floor,
    /// The cube's top, which faces the spot.
    Top,
    /// The cube's side toward the spot.
    Side,
    /// Anything else: the sky, the cube's other sides, the shadow's outline.
    Other,
}

fn shows_at(cam: &CameraState, x: f32, y: f32) -> Shows {
    let (o, d) = (cam.position, ray(cam, x, y));
    let on_floor = (d.y < 0.0).then(|| -o.y / d.y);
    if let Some((t, axis)) = enters(o, d, CUBE_LO, CUBE_HI)
        && on_floor.is_none_or(|f| t < f)
    {
        return match axis {
            1 => Shows::Top,
            0 => Shows::Side,
            _ => Shows::Other,
        };
    }
    let Some(t) = on_floor else {
        return Shows::Other;
    };
    let p = o + d * t;
    if p.x.abs() > 19.5 || p.z.abs() > 19.5 {
        return Shows::Other;
    }
    let m = Vec3::splat(0.12);
    if blocked(p, LIGHT, CUBE_LO + m, CUBE_HI - m) {
        Shows::Umbra
    } else if !blocked(p, LIGHT, CUBE_LO - m, CUBE_HI + m) {
        Shows::Floor
    } else {
        Shows::Other
    }
}

/// What the pixel shows, when the pixels around it show the same.
fn shows(cam: &CameraState, x: u32, y: u32) -> Shows {
    let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
    let centre = shows_at(cam, cx, cy);
    let same = [(-1.5, -1.5), (1.5, -1.5), (-1.5, 1.5), (1.5, 1.5)]
        .iter()
        .all(|(dx, dy)| shows_at(cam, cx + dx, cy + dy) == centre);
    if same { centre } else { Shows::Other }
}

/// The spot's scene with `light` at the spot's place: a spot (one local view) or a point light
/// (six).
fn check_spot(gpu: &Gpu, aa: Antialiasing, light: LightView) {
    let cam = eye();
    let mut r = renderer(gpu, aa, &cam);
    let views = if light.kind == LightKindView::Point {
        6
    } else {
        1
    };
    let shadowed = shot(&mut r, spot_scene(light.clone(), true));
    assert_eq!(
        (r.last.local_shadows, r.last.local_shadows_dropped),
        (1, 0),
        "{aa:?}: the light's local views"
    );
    let counts = r.draw_counts();
    if views == 1 {
        assert_eq!(
            counts.local[0].instances(),
            2,
            "{aa:?}: the floor and the cube cast into the local view ({counts:?})"
        );
    } else {
        // The floor reaches every face but the one looking up; the cube lies below the light.
        assert!(
            counts.local[3].instances() == 2
                && counts.local[..views]
                    .iter()
                    .filter(|c| c.instances() > 0)
                    .count()
                    >= 5,
            "{aa:?}: the faces' casters ({counts:?})"
        );
    }
    assert!(
        counts.local[views..].iter().all(|c| c.instances() == 0),
        "{aa:?}: the other local views draw nothing ({counts:?})"
    );
    assert!(
        counts.cascades.iter().all(|c| c.instances() == 0),
        "{aa:?}: no cascade is drawn without a sun ({counts:?})"
    );
    let plain = shot(&mut r, spot_scene(unshadowed(&light), true));
    assert_eq!(r.last.local_shadows, 0, "{aa:?}: `shadows` off");
    let ambient = shot(&mut r, spot_scene(dark(&light), true));
    assert_eq!(r.last.local_shadows, 0, "{aa:?}: a light of intensity 0");
    let mut seen = [0usize; 4];
    for y in 0..H {
        for x in 0..W {
            let (s, p, a) = (rgb(&shadowed, x, y), rgb(&plain, x, y), rgb(&ambient, x, y));
            let what = format!("{aa:?}, pixel ({x}, {y})");
            match shows(&cam, x, y) {
                Shows::Umbra => {
                    seen[0] += 1;
                    assert_same(&format!("{what}, in the cube's shadow"), s, a, 2e-3);
                    assert!(
                        luminance(p) > 1.5 * luminance(s),
                        "{what}: the spot lights the floor here without its shadow ({p:?} \
                         against {s:?})"
                    );
                }
                Shows::Floor => {
                    seen[1] += 1;
                    assert_same(&format!("{what}, the floor outside the shadow"), s, p, 1e-3);
                }
                kind @ (Shows::Top | Shows::Side) => {
                    seen[if kind == Shows::Top { 2 } else { 3 }] += 1;
                    assert!(
                        luminance(s) >= 0.98 * luminance(p),
                        "{what}: the cube shadows itself ({kind:?}: {s:?} against {p:?})"
                    );
                    assert!(
                        luminance(p) > 1.5 * luminance(a),
                        "{what}: the spot lights the cube here ({kind:?}: {p:?} against {a:?})"
                    );
                }
                Shows::Other => {}
            }
        }
    }
    eprintln!("{aa:?}: umbra, floor, top and side pixels checked: {seen:?}");
    assert!(
        seen[0] > 150 && seen[1] > 20_000 && seen[2] > 60 && seen[3] > 100,
        "{aa:?}: {seen:?}"
    );
    // The shadow lies where the cube's outline says, and the floor beside it is lit.
    assert!(
        luminance(patch(&plain, &cam, BESIDE, 2))
            > 1.5 * luminance(patch(&ambient, &cam, BESIDE, 2)),
        "{aa:?}: the spot lights the floor beside the shadow"
    );
    assert_same(
        &format!("{aa:?}: beside the shadow"),
        patch(&shadowed, &cam, BESIDE, 2),
        patch(&plain, &cam, BESIDE, 2),
        1e-3,
    );
    // No occluder: the shadowed spot draws the frame of the unshadowed one, the floor (a caster
    // of its own map) from under the spot to the cone's grazing far edge.
    let alone = shot(&mut r, spot_scene(light.clone(), false));
    assert_eq!(r.last.local_shadows, 1);
    if views == 1 {
        assert_eq!(r.draw_counts().local[0].instances(), 1);
    }
    let alone_plain = shot(&mut r, spot_scene(unshadowed(&light), false));
    let mut lit = 0;
    for y in 0..H {
        for x in 0..W {
            let (s, p) = (rgb(&alone, x, y), rgb(&alone_plain, x, y));
            assert_same(
                &format!("{aa:?}, pixel ({x}, {y}), no occluder"),
                s,
                p,
                1e-3,
            );
            lit += usize::from(luminance(p) > 1.5 * luminance(rgb(&ambient, x, y)));
        }
    }
    assert!(lit > 10_000, "{aa:?}: {lit} pixels lit by the spot");
}

#[test]
fn a_spot_shadows_the_floor_under_an_occluder() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    for aa in [Antialiasing::Off, Antialiasing::Msaa] {
        check_spot(&gpu, aa, the_spot());
    }
}

/// A point light in the spot's place: the cube's shadow on the floor, the floor out to the range
/// (light meeting it from straight down to 73 degrees, across the cube map's seams) unshadowed
/// where nothing stands between, the cube's lit faces not shadowed by it.
#[test]
fn a_point_light_shadows_the_floor_under_an_occluder() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    for aa in [Antialiasing::Off, Antialiasing::Msaa] {
        check_spot(&gpu, aa, point_light(LIGHT));
    }
}

/// A point light 2 m up with a cube on each side and one below it: each face of its cube map
/// that looks at the floor holds a shadow, and the floor outside the shadows is the frame of
/// `shadows: false`; local shadows off draws that frame to the bit.
#[test]
fn a_point_light_shadows_on_every_side() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    let light_at = Vec3::new(0.0, 2.0, 0.0);
    let light = point_light(light_at);
    // Hovering cubes: 0.8 m, centred 0.9 m up, 2.5 m out along +x, -x, +z and -z, and a flat one
    // under the light.
    let boxes: Vec<(Vec3, Vec3)> = [
        (Vec3::new(2.5, 0.9, 0.0), Vec3::splat(0.8)),
        (Vec3::new(-2.5, 0.9, 0.0), Vec3::splat(0.8)),
        (Vec3::new(0.0, 0.9, 2.5), Vec3::splat(0.8)),
        (Vec3::new(0.0, 0.9, -2.5), Vec3::splat(0.8)),
        (Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.6, 0.1, 0.6)),
    ]
    .to_vec();
    let instances = || {
        let mut v = vec![floor(1)];
        for (i, &(c, size)) in boxes.iter().enumerate() {
            v.push(at(2 + i as u64, c, size, look("cube", GREY, true)));
        }
        v
    };
    let cam = CameraState::look_at(Vec3::new(1.0, 15.0, 9.0), Vec3::new(0.0, 0.0, 0.5));
    let hit_box = |o: Vec3, d: Vec3, grow: f32| {
        boxes
            .iter()
            .filter_map(|&(c, s)| enters(o, d, c - s * 0.5 - grow, c + s * 0.5 + grow))
            .map(|(t, _)| t)
            .fold(f32::INFINITY, f32::min)
    };
    let blocked_by_any = |p: Vec3, grow: f32| {
        boxes.iter().any(|&(c, s)| {
            blocked(
                p,
                light_at,
                c - s * 0.5 - Vec3::splat(grow),
                c + s * 0.5 + Vec3::splat(grow),
            )
        })
    };
    for aa in [Antialiasing::Off, Antialiasing::Msaa] {
        let mut r = renderer(&gpu, aa, &cam);
        let shadowed = shot(&mut r, frame(instances(), vec![light.clone()]));
        assert_eq!((r.last.local_shadows, r.last.local_shadows_dropped), (1, 0));
        let counts = r.draw_counts();
        for (f, want) in [(0, 2), (1, 2), (3, 2), (4, 2), (5, 2)] {
            assert!(
                counts.local[f].instances() >= want,
                "{aa:?}: face {f} draws the floor and its cube ({counts:?})"
            );
        }
        let plain = shot(&mut r, frame(instances(), vec![unshadowed(&light)]));
        let ambient = shot(&mut r, frame(instances(), vec![dark(&light)]));
        let mut off = renderer(&gpu, aa, &cam);
        off.set_local_shadows(LocalShadows::Off);
        let by_option = shot(&mut off, frame(instances(), vec![light.clone()]));
        assert!(by_option == plain, "{aa:?}: local shadows off");
        // Floor pixels (every sample of the pixel's neighbourhood on the floor, no cube in front):
        // in the umbra (12 cm inside every outline) the ambient frame, outside every shadow (12 cm
        // out) the unshadowed one.
        let mut umbra = [0usize; 5];
        let mut floor_px = 0;
        for y in 0..H {
            for x in 0..W {
                let floor_at = |dx: f32, dy: f32| {
                    let (o, d) = (
                        cam.position,
                        ray(&cam, x as f32 + 0.5 + dx, y as f32 + 0.5 + dy),
                    );
                    let t = -o.y / d.y;
                    (d.y < 0.0 && t < hit_box(o, d, 0.05)).then(|| o + d * t)
                };
                let Some(p) = floor_at(0.0, 0.0) else {
                    continue;
                };
                if [(-1.5, -1.5), (1.5, -1.5), (-1.5, 1.5), (1.5, 1.5)]
                    .iter()
                    .any(|&(dx, dy)| floor_at(dx, dy).is_none())
                    || (p - light_at).length() > 19.0
                {
                    continue;
                }
                let (s, u, a) = (rgb(&shadowed, x, y), rgb(&plain, x, y), rgb(&ambient, x, y));
                let what = format!("{aa:?}, pixel ({x}, {y}), floor at {p}");
                // 12 cm inside the outlines (the flat cube shrunk by less than its thickness).
                let inside = |sz: Vec3| sz * 0.5 - (sz * 0.4).min(Vec3::splat(0.12));
                if let Some(i) = boxes
                    .iter()
                    .position(|&(c, sz)| blocked(p, light_at, c - inside(sz), c + inside(sz)))
                {
                    umbra[i] += 1;
                    assert_same(&format!("{what}, in a shadow"), s, a, 2e-3);
                } else if !blocked_by_any(p, 0.12) {
                    floor_px += 1;
                    assert_same(&format!("{what}, outside the shadows"), s, u, 1e-3);
                }
            }
        }
        eprintln!("{aa:?}: umbra pixels by cube {umbra:?}, floor pixels {floor_px}");
        assert!(
            umbra.iter().all(|&n| n > 40) && floor_px > 20_000,
            "{aa:?}: {umbra:?}, {floor_px}"
        );
    }
}

/// The entity each pixel of the scene last applied shows (0 for none), from the id pass.
fn id_image(r: &mut Renderer) -> Vec<u64> {
    r.request_id_image();
    for _ in 0..30 {
        let _ = r.capture_rgba(0.0);
        if r.take_visible().is_some() {
            return r.take_id_image().expect("the id pass kept its image");
        }
    }
    panic!("the id pass did not answer");
}

/// A ball in the cube's place: nowhere on its lit side does it shadow itself, up to 80 degrees
/// from the light (past that a tessellated ball's own facets may), and it shadows the floor behind
/// it. (Without the receiver's offsets along its normal and toward the light, forward.wgsl
/// `local_shadow`, the ball's curve shows acne here: the casters' depth bias follows each
/// triangle's slope, which a neighbouring triangle's receiver does not share.) Drawn as its full
/// mesh, which is convex, so any shadow on its lit side is the map's error; a coarser level can
/// shadow itself where its simplified shape differs (the sphere's first level does at one pixel
/// near its pole from 7 m).
#[test]
fn a_curved_caster_does_not_shadow_its_lit_side() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    let light = the_spot();
    let radius = 0.75;
    let scene = |light: LightView| {
        frame(
            vec![
                floor(1),
                at(
                    2,
                    CUBE,
                    Vec3::splat(2.0 * radius),
                    look("sphere", GREY, true),
                ),
            ],
            vec![light],
        )
    };
    // (the camera, whether it sees the floor where the ball's shadow falls).
    for (what, from, sees_floor) in [
        ("from the side", Vec3::new(-6.0, 7.0, 8.0), true),
        ("along the light", Vec3::new(-5.0, 9.0, 0.5), false),
    ] {
        let cam = CameraState::look_at(from, CUBE);
        let mut r = renderer(&gpu, Antialiasing::Off, &cam);
        r.set_lod(LodMode::Off);
        let shadowed = shot(&mut r, scene(light.clone()));
        assert_eq!(r.last.local_shadows, 1, "{what}");
        assert_eq!(r.draw_counts().local[0].instances(), 2, "{what}");
        let ids = id_image(&mut r);
        let plain = shot(&mut r, scene(unshadowed(&light)));
        let ambient = shot(&mut r, scene(dark(&light)));
        // The ball's pixels, two pixels inside its outline.
        let ball = |x: u32, y: u32| {
            (-2..=2).all(|dy: i32| {
                (-2..=2).all(|dx: i32| {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    nx >= 0
                        && ny >= 0
                        && nx < W as i32
                        && ny < H as i32
                        && ids[(ny as u32 * W + nx as u32) as usize] == 2
                })
            })
        };
        let mut lit = 0;
        for y in 0..H {
            for x in 0..W {
                if !ball(x, y) {
                    continue;
                }
                // Where the pixel's ray meets the ball.
                let d = ray(&cam, x as f32 + 0.5, y as f32 + 0.5);
                let oc = cam.position - CUBE;
                let b = oc.dot(d);
                let disc = b * b - (oc.length_squared() - radius * radius);
                let hit = cam.position + d * (-b - disc.max(0.0).sqrt());
                let n = (hit - CUBE).normalize();
                if n.dot((LIGHT - hit).normalize()) < 0.17 {
                    continue;
                }
                let (s, p) = (rgb(&shadowed, x, y), rgb(&plain, x, y));
                assert!(
                    luminance(s) >= 0.98 * luminance(p),
                    "{what}, pixel ({x}, {y}): the ball shadows itself ({s:?} against {p:?})"
                );
                lit += 1;
            }
        }
        eprintln!("{what}: {lit} pixels of the ball's lit side checked");
        assert!(lit > 400, "{what}: {lit} pixels");
        if sees_floor {
            assert_same(
                &format!("{what}: the floor in the ball's shadow"),
                patch(&shadowed, &cam, AIM, 1),
                patch(&ambient, &cam, AIM, 1),
                2e-3,
            );
        }
    }
}

/// `n` spots like the scene's, side by side along z (all shadowed, all lighting the floor).
fn spots(n: usize) -> Vec<LightView> {
    (0..n)
        .map(|i| {
            let dz = Vec3::new(0.0, 0.0, 2.0 * i as f32);
            spot(10 + i as u64, LIGHT + dz, AIM + dz, 35.0, 20.0, 200.0)
        })
        .collect()
}

#[test]
fn shadows_false_and_the_option_off_draw_the_same_frame() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    let cam = eye();
    let light = the_spot();
    let scene = |lights: Vec<LightView>| frame(vec![floor(1), cube(2, CUBE, 1.0, GREY)], lights);
    let none_drawn = |r: &Renderer, what: &str| {
        assert_eq!(
            (r.last.local_shadows, r.last.local_shadows_dropped),
            (0, 0),
            "{what}"
        );
        let counts = r.draw_counts();
        assert!(
            counts.local.iter().all(|c| c.instances() == 0),
            "{what}: {counts:?}"
        );
    };
    // The lists of the scene without the light.
    let mut r = renderer(&gpu, Antialiasing::Off, &cam);
    let _ = shot(&mut r, scene(vec![]));
    let bare = r.list_bytes();
    // `shadows` off.
    let plain = shot(&mut r, scene(vec![unshadowed(&light)]));
    none_drawn(&r, "`shadows` off");
    assert_eq!(r.list_bytes(), bare, "`shadows` off: the lists");
    // `shadows` on, local shadows off: the same frame, the same lists.
    let mut off = renderer(&gpu, Antialiasing::Off, &cam);
    off.set_local_shadows(LocalShadows::Off);
    assert_eq!(off.local_shadows(), LocalShadows::Off);
    let unshadowed_by_option = shot(&mut off, scene(vec![light.clone()]));
    none_drawn(&off, "local shadows off");
    assert_eq!(off.list_bytes(), bare, "local shadows off: the lists");
    assert!(
        unshadowed_by_option == plain,
        "local shadows off does not draw the frame of `shadows: false`"
    );
    // On: another image, and 4 bytes more per list entry for the one view (48 + 4 x 5 without).
    off.set_local_shadows(LocalShadows::On);
    let shadowed = shot(&mut off, scene(vec![light.clone()]));
    assert_eq!(off.last.local_shadows, 1);
    assert!(shadowed != plain, "the shadow changes no pixel");
    assert_eq!(off.list_bytes() * 68, bare * 72, "one local view's list");
    // Two spots, five, and a point light (six views).
    let _ = shot(&mut off, scene(spots(2)));
    assert_eq!(off.last.local_shadows, 2);
    assert_eq!(off.list_bytes() * 68, bare * 76, "two local views' lists");
    let _ = shot(&mut off, scene(spots(5)));
    assert_eq!(
        (off.last.local_shadows, off.last.local_shadows_dropped),
        (5, 0)
    );
    assert_eq!(off.list_bytes() * 68, bare * 88, "five local views' lists");
    let _ = shot(&mut off, scene(vec![point_light(LIGHT)]));
    assert_eq!(
        (off.last.local_shadows, off.last.local_shadows_dropped),
        (1, 0)
    );
    assert_eq!(
        off.list_bytes() * 68,
        bare * 92,
        "a point light's six views' lists"
    );
    // Back and forth on one renderer: each frame comes back to the bit, with its lists.
    for round in 0..2 {
        off.set_local_shadows(LocalShadows::Off);
        let img = shot(&mut off, scene(vec![light.clone()]));
        none_drawn(&off, "local shadows off again");
        assert_eq!(
            off.list_bytes(),
            bare,
            "round {round}: the lists, off again"
        );
        assert!(
            img == plain,
            "round {round}: off again is not the same frame"
        );
        off.set_local_shadows(LocalShadows::On);
        let img = shot(&mut off, scene(vec![light.clone()]));
        assert_eq!(off.list_bytes() * 68, bare * 72, "round {round}: on again");
        assert!(
            img == shadowed,
            "round {round}: on again is not the same frame"
        );
    }
}

// --- Casters by opacity, a masked caster, a transparent receiver ----------------------------------

#[test]
fn casters_cast_by_their_opacity_and_a_transparent_receiver_is_shadowed() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    // One wide spot straight above a floor; four cubes 2.5 m up along x (opaque, clear glass,
    // blends of alpha 0.6 and 0.35, as tests/transparency.rs `casters`), and an alpha-masked quad
    // 4 m square 3 m up, nearer the camera.
    let high = Vec3::new(0.0, 12.0, 0.0);
    let light = spot(10, high, Vec3::ZERO, 50.0, 30.0, 600.0);
    let cam = CameraState::look_at(Vec3::new(0.0, 14.0, 14.0), Vec3::new(0.0, 0.0, 3.0));
    let xs = [-6.0f32, -2.0, 2.0, 6.0];
    let grey = |alpha| look("cube", [0.7, 0.7, 0.7, alpha], true);
    let quad = Vec3::new(0.0, 3.0, 6.0);
    let scene = |light: LightView| {
        frame(
            vec![
                floor(1),
                at(2, Vec3::new(xs[0], 2.5, 0.0), Vec3::ONE, grey(1.0)),
                at(
                    3,
                    Vec3::new(xs[1], 2.5, 0.0),
                    Vec3::ONE,
                    Look {
                        transmission: Some(1.0),
                        ..grey(1.0)
                    },
                ),
                at(4, Vec3::new(xs[2], 2.5, 0.0), Vec3::ONE, grey(0.6)),
                at(5, Vec3::new(xs[3], 2.5, 0.0), Vec3::ONE, grey(0.35)),
                at(
                    6,
                    quad,
                    Vec3::new(4.0, 1.0, 4.0),
                    look(&format!("{MODEL}#holes"), [1.0; 4], true),
                ),
            ],
            vec![light],
        )
    };
    // Where the light through `p` meets the floor.
    let under = |p: Vec3| high + (p - high) * (high.y / (high.y - p.y));
    let mut r = renderer(&gpu, Antialiasing::Off, &cam);
    let shadowed = shot(&mut r, scene(light.clone()));
    assert_eq!(r.last.local_shadows, 1);
    // The floor, the opaque cube, the blend of alpha 0.6 and the quad: the glass and the blend of
    // alpha 0.35 are no casters at all (transparency.md 5).
    assert_eq!(r.draw_counts().local[0].instances(), 4);
    let plain = shot(&mut r, scene(unshadowed(&light)));
    let ambient = shot(&mut r, scene(dark(&light)));
    let is_dark = |p: Vec3, r: i32| {
        let (s, u, a) = (
            patch(&shadowed, &cam, p, r),
            patch(&plain, &cam, p, r),
            patch(&ambient, &cam, p, r),
        );
        assert!(
            luminance(u) > 1.5 * luminance(a),
            "{p}: the spot lights the floor here ({u:?} against {a:?})"
        );
        let dark = luminance(s) < 0.6 * luminance(u);
        // Shadowed all the way or not at all.
        assert_same(
            &format!("the floor at {p}"),
            s,
            if dark { a } else { u },
            2e-3,
        );
        dark
    };
    let cubes: Vec<bool> = xs
        .iter()
        .map(|&x| is_dark(under(Vec3::new(x, 2.5, 0.0)), 1))
        .collect();
    assert_eq!(
        cubes,
        [true, false, true, false],
        "shadows under the opaque cube, the glass, alpha 0.6 and alpha 0.35"
    );
    // The quad's checker: the four cells around its centre (0.5 m each) alternate, so the floor
    // under them is dark under two that touch by a corner and lit under the other two.
    let cell = |sx: f32, sz: f32| is_dark(under(quad + Vec3::new(0.25 * sx, 0.0, 0.25 * sz)), 0);
    let cells = [
        cell(-1.0, -1.0),
        cell(1.0, -1.0),
        cell(-1.0, 1.0),
        cell(1.0, 1.0),
    ];
    assert!(
        cells[0] == cells[3] && cells[1] == cells[2] && cells[0] != cells[1],
        "the masked quad casts its checker: {cells:?}"
    );

    // A transparent receiver: a blended pane (alpha 0.5, no caster) 2 m up over nothing but the
    // sky, under an opaque cube 5 m up and the spot 8 m up.
    let top = Vec3::new(0.0, 8.0, 0.0);
    let light = spot(10, top, Vec3::ZERO, 40.0, 20.0, 300.0);
    let cam = CameraState::look_at(Vec3::new(0.0, 9.0, 9.0), Vec3::new(0.0, 2.0, 0.0));
    let scene = |light: LightView| {
        frame(
            vec![
                cube(1, Vec3::new(0.0, 5.0, 0.0), 1.0, GREY),
                at(
                    2,
                    Vec3::new(0.0, 2.0, 0.0),
                    Vec3::new(6.0, 1.0, 6.0),
                    look("plane", [0.8, 0.8, 0.8, 0.5], false),
                ),
            ],
            vec![light],
        )
    };
    let mut r = renderer(&gpu, Antialiasing::Off, &cam);
    let shadowed = shot(&mut r, scene(light.clone()));
    assert_eq!((r.last.local_shadows, r.last.transparent), (1, 1));
    assert_eq!(
        r.draw_counts().local[0].instances(),
        1,
        "the cube alone casts"
    );
    let plain = shot(&mut r, scene(unshadowed(&light)));
    let ambient = shot(&mut r, scene(dark(&light)));
    let (under_cube, beside) = (Vec3::new(0.0, 2.0, 0.0), Vec3::new(2.2, 2.0, 0.0));
    assert_same(
        "the pane in the cube's shadow",
        patch(&shadowed, &cam, under_cube, 2),
        patch(&ambient, &cam, under_cube, 2),
        2e-3,
    );
    assert_same(
        "the pane beside the shadow",
        patch(&shadowed, &cam, beside, 2),
        patch(&plain, &cam, beside, 2),
        1e-3,
    );
    assert!(
        luminance(patch(&shadowed, &cam, under_cube, 2))
            < 0.8 * luminance(patch(&shadowed, &cam, beside, 2)),
        "the pane is no darker in the cube's shadow than beside it"
    );
    assert!(
        luminance(patch(&plain, &cam, under_cube, 2))
            > 1.2 * luminance(patch(&shadowed, &cam, under_cube, 2)),
        "the spot lights the pane under the cube without its shadow"
    );
}

// --- The limit ------------------------------------------------------------------------------------

/// The row's spots: one more than the local views. Where they stand along x: 5 m up, shining
/// straight down on a 1 m cube 2 m up each, their cones (2.1 m wide on the floor) 3 m apart.
const ROW_N: usize = LOCAL_VIEWS + 1;
fn row_x(i: usize) -> f32 {
    (i as f32 - (ROW_N - 1) as f32 * 0.5) * 3.0
}

/// The row's ids by place, from 11 on.
fn row_ids() -> [u64; ROW_N] {
    std::array::from_fn(|i| 11 + i as u64)
}

/// Every spot of the row shadowed but the one at `place`.
fn all_but(place: usize) -> Vec<bool> {
    (0..ROW_N).map(|i| i != place).collect()
}

/// The row's spots with `ids` (by place), and its floor and cubes.
fn row(ids: [u64; ROW_N], extra: Vec<LightView>, reversed: bool) -> RenderFrame {
    let mut v = vec![floor(1)];
    let mut lights = extra;
    for (i, &id) in ids.iter().enumerate() {
        let x = row_x(i);
        v.push(cube(2 + i as u64, Vec3::new(x, 2.0, 0.0), 1.0, GREY));
        lights.push(spot(
            id,
            Vec3::new(x, 5.0, 0.0),
            Vec3::new(x, 0.0, 0.0),
            12.0,
            8.0,
            150.0,
        ));
    }
    if reversed {
        lights.reverse();
    }
    frame(v, lights)
}

/// Which of the row's cubes shadow the floor in `img`: under each, the floor is the ambient
/// frame's (shadowed) or the unshadowed frame's.
fn row_shadows(
    what: &str,
    cam: &CameraState,
    img: &[f32],
    plain: &[f32],
    ambient: &[f32],
) -> Vec<bool> {
    (0..ROW_N)
        .map(|i| {
            let x = row_x(i);
            let p = Vec3::new(x, 0.0, 0.0);
            let (s, u, a) = (
                patch(img, cam, p, 1),
                patch(plain, cam, p, 1),
                patch(ambient, cam, p, 1),
            );
            assert!(
                luminance(u) > 1.5 * luminance(a),
                "{what}: the spot at x = {x} lights the floor under its cube"
            );
            let dark = luminance(s) < 0.6 * luminance(u);
            assert_same(
                &format!("{what}: the floor under the cube at x = {x}"),
                s,
                if dark { a } else { u },
                2e-3,
            );
            dark
        })
        .collect()
}

#[test]
fn one_spot_more_than_the_limit_is_counted() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    let ids = row_ids();
    let (first, last) = (0, ROW_N - 1);
    let views = LOCAL_VIEWS as u32;
    let dim = |f: &RenderFrame, change: fn(&LightView) -> LightView| {
        let mut f = f.clone();
        f.lights = f.lights.map(|l| l.iter().map(change).collect());
        f
    };
    // From the left: the spot at the right end is the farthest, the smallest on screen.
    let left = CameraState::look_at(Vec3::new(-14.0, 20.0, 34.0), Vec3::new(5.0, 0.0, 0.0));
    let mut r = renderer(&gpu, Antialiasing::Off, &left);
    let plain = shot(&mut r, dim(&row(ids, vec![], false), unshadowed));
    let ambient = shot(&mut r, dim(&row(ids, vec![], false), dark));
    let img = shot(&mut r, row(ids, vec![], false));
    assert_eq!(
        (r.last.local_shadows, r.last.local_shadows_dropped),
        (views, 1),
        "{ROW_N} shadowed spots in view"
    );
    assert_eq!(
        row_shadows("from the left", &left, &img, &plain, &ambient),
        all_but(last)
    );
    // The order of the lights changes nothing: the same frame to the bit (the cones are apart, so
    // no pixel sums two lights).
    let again = shot(&mut r, row(ids, vec![], true));
    assert_eq!(
        (r.last.local_shadows, r.last.local_shadows_dropped),
        (views, 1)
    );
    assert!(
        again == img,
        "the lights listed in reverse draw another frame"
    );
    // From the right the spot at the left end is the smallest.
    let right = CameraState::look_at(Vec3::new(14.0, 20.0, 34.0), Vec3::new(-5.0, 0.0, 0.0));
    let mut r = renderer(&gpu, Antialiasing::Off, &right);
    let plain = shot(&mut r, dim(&row(ids, vec![], false), unshadowed));
    let ambient = shot(&mut r, dim(&row(ids, vec![], false), dark));
    let img = shot(&mut r, row(ids, vec![], false));
    assert_eq!(r.last.local_shadows_dropped, 1);
    assert_eq!(
        row_shadows("from the right", &right, &img, &plain, &ambient),
        all_but(first)
    );
    // From the middle the two end spots are equal on screen: the lower id keeps its shadow.
    let middle = CameraState::look_at(Vec3::new(0.0, 20.0, 34.0), Vec3::ZERO);
    let mut r = renderer(&gpu, Antialiasing::Off, &middle);
    let plain = shot(&mut r, dim(&row(ids, vec![], false), unshadowed));
    let ambient = shot(&mut r, dim(&row(ids, vec![], false), dark));
    let mut swapped = ids;
    swapped.swap(first, last);
    for (ids, want) in [(ids, all_but(last)), (swapped, all_but(first))] {
        for reversed in [false, true] {
            let img = shot(&mut r, row(ids, vec![], reversed));
            assert_eq!(
                (r.last.local_shadows, r.last.local_shadows_dropped),
                (views, 1)
            );
            assert_eq!(
                row_shadows(
                    &format!("from the middle, ids {ids:?}"),
                    &middle,
                    &img,
                    &plain,
                    &ambient
                ),
                want
            );
        }
    }
    // One more shadowed spot behind the camera takes no view and is not counted.
    let behind = spot(
        99,
        Vec3::new(0.0, 5.0, 80.0),
        Vec3::new(0.0, 0.0, 80.0),
        12.0,
        8.0,
        150.0,
    );
    let img = shot(&mut r, row(ids, vec![behind.clone()], false));
    assert_eq!(
        (r.last.local_shadows, r.last.local_shadows_dropped),
        (views, 1),
        "one more spot out of view"
    );
    assert_eq!(
        row_shadows("with one more spot behind", &middle, &img, &plain, &ambient),
        all_but(last)
    );
    // With one of the row out of view instead, the ones in view all keep their shadows.
    let mut f = row(ids, vec![], false);
    if let Some(lights) = &mut f.lights {
        lights[0] = LightView {
            id: ids[0],
            ..behind
        };
    }
    let _ = shot(&mut r, f);
    assert_eq!(
        (r.last.local_shadows, r.last.local_shadows_dropped),
        (views, 0),
        "the row less one in view, one behind the camera"
    );
}

// --- The draw paths -------------------------------------------------------------------------------

#[test]
fn every_draw_path_draws_the_local_views() {
    let choice = BackendChoice::from_env();
    let Ok(full) = Gpu::headless_with(choice, Minimal::default()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    let device = |m: Minimal| Gpu::headless_with(choice, m).expect("another device");
    // Batch by batch with the base in first_instance (the browser with the feature), and WebGPU's
    // baseline (the base in a uniform at a dynamic offset).
    let per_batch = device(Minimal {
        multi_draw: true,
        ..Minimal::default()
    });
    let baseline = device(Minimal {
        first_instance: true,
        ..Minimal::default()
    });
    if !full.caps.indirect_first_instance {
        eprintln!(concat!(
            "no INDIRECT_FIRST_INSTANCE: every shot takes the baseline path; ",
            "first-instance paths not checked"
        ));
    }
    let cam = eye();
    let light = the_spot();
    let shoot = |gpu: &Gpu, culled: bool| {
        let mut r = renderer(gpu, Antialiasing::Off, &cam);
        if culled {
            // Two-phase culling and the depth prepass: the local view's draws stay the same.
            r.set_occlusion(OcclusionMode::On);
            r.set_prepass(PrepassMode::On);
            r.apply(spot_scene(light.clone(), true), 0.0);
            for _ in 0..3 {
                let _ = r.capture_rgba(0.0);
            }
        }
        let img = shot(&mut r, spot_scene(light.clone(), true));
        assert_eq!(r.last.local_shadows, 1, "{}", r.draw_path());
        let local = r.draw_counts().local[0].clone();
        assert_eq!(local.instances(), 2, "{}: {local:?}", r.draw_path());
        let plain = shot(&mut r, spot_scene(unshadowed(&light), true));
        let path = r.draw_path();
        // The shadow, and the floor beside it.
        let (dark, lit) = (patch(&img, &cam, AIM, 2), patch(&img, &cam, BESIDE, 2));
        assert!(
            luminance(patch(&plain, &cam, AIM, 2)) > 1.5 * luminance(dark),
            "{path}: no shadow under the cube"
        );
        assert_same(
            &format!("{path}: beside the shadow"),
            lit,
            patch(&plain, &cam, BESIDE, 2),
            1e-3,
        );
        (path, dark, lit, local)
    };
    let a = shoot(&full, false);
    let c = shoot(&baseline, false);
    assert_eq!(c.0, "baseline");
    for other in [c, shoot(&per_batch, false), shoot(&full, true)] {
        eprintln!("{} against {}", other.0, a.0);
        assert_same(&format!("{}: the shadow", other.0), other.1, a.1, 1e-3);
        assert_same(&format!("{}: the lit floor", other.0), other.2, a.2, 1e-3);
        assert_eq!(other.3, a.3, "{}: the local view's draws", other.0);
    }
}

// --- Levels of detail -----------------------------------------------------------------------------

#[test]
fn a_local_view_draws_the_cameras_levels() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    // A rock (radius about 1) on a floor that casts nothing, 50 m from the camera and 8 m under a
    // spot on the camera's side: the camera draws one of its coarser levels.
    let rock = Vec3::new(0.0, 1.0, 0.0);
    let cam = CameraState::look_at(Vec3::new(0.0, 30.0, 40.0), rock);
    let light = spot(10, Vec3::new(0.0, 7.0, 5.0), rock, 30.0, 20.0, 300.0);
    let scene = |light: LightView| {
        frame(
            vec![
                at(
                    1,
                    Vec3::ZERO,
                    Vec3::new(40.0, 1.0, 40.0),
                    look("plane", GREY, false),
                ),
                at(
                    2,
                    rock,
                    Vec3::ONE,
                    look(&format!("{}#rock_a", demo::LOD_MODEL), [1.0; 4], true),
                ),
            ],
            vec![light],
        )
    };
    let mut r = renderer(&gpu, Antialiasing::Off, &cam);
    r.add_model(demo::LOD_MODEL, &demo::lod_model(4));
    r.set_lod(LodMode::On);
    let shadowed = shot(&mut r, scene(light.clone()));
    assert_eq!(r.last.lod, "on");
    let counts = r.draw_counts();
    assert_eq!(counts.local[0].instances(), 1, "the rock alone casts");
    let level = (0..8)
        .find(|&l| counts.local[0].instances[l] == 1)
        .expect("the rock's level in the local view");
    eprintln!(
        "the rock: level {level} in the local view; the camera draws {:?}",
        counts.camera.instances
    );
    assert!(level > 0, "the camera draws the rock's full mesh from 50 m");
    assert_eq!(
        counts.camera.instances[level], 1,
        "the local view draws the camera's level ({counts:?})"
    );
    // The rock's lit face toward the camera is not shadowed by a finer or coarser copy of itself.
    let plain = shot(&mut r, scene(unshadowed(&light)));
    let ambient = shot(&mut r, scene(dark(&light)));
    let (s, p, a) = (
        patch(&shadowed, &cam, rock, 1),
        patch(&plain, &cam, rock, 1),
        patch(&ambient, &cam, rock, 1),
    );
    assert!(
        luminance(p) > 1.5 * luminance(a),
        "the spot lights the rock"
    );
    assert!(
        luminance(s) >= 0.98 * luminance(p),
        "the rock shadows itself: {s:?} against {p:?}"
    );
    // Levels off: the full mesh in both.
    r.set_lod(LodMode::Off);
    let _ = shot(&mut r, scene(light.clone()));
    let counts = r.draw_counts();
    assert_eq!(counts.local[0].instances[0], 1, "{counts:?}");
    assert_eq!(counts.local[0].instances(), 1, "{counts:?}");
    assert_eq!(counts.camera.instances[0], 2, "{counts:?}");
}

// --- Beside the sun -------------------------------------------------------------------------------

#[test]
fn a_spot_shadows_beside_the_sun_and_under_ray_traced_sun_shadows() {
    let choice = BackendChoice::from_env();
    let Ok(gpu) = Gpu::headless(choice) else {
        eprintln!("no GPU: skipped");
        return;
    };
    let cam = eye();
    let light = the_spot();
    // The sun slants toward +z: the cube's sun shadow falls at z from 1 to 2.6, beside the spot's
    // (around `AIM`).
    let sun = |shadows: bool| LightView {
        id: 1,
        kind: LightKindView::Directional,
        position: [0.0; 3],
        direction: Vec3::new(0.2, -1.0, 0.6).normalize().to_array(),
        color: [1.0; 3],
        intensity: 2.0,
        range: 0.0,
        inner_deg: 0.0,
        outer_deg: 0.0,
        shadows,
    };
    let scene = |sun: LightView, spot: LightView| {
        frame(vec![floor(1), cube(2, CUBE, 1.0, GREY)], vec![sun, spot])
    };
    // In the spot's shadow and the sun's light; in the sun's shadow and the spot's light.
    let (in_spot, in_sun) = (Vec3::new(1.0, 0.0, -0.3), Vec3::new(-0.4, 0.0, 1.8));
    let mut paths = vec![("cascades", gpu)];
    match Gpu::headless_ray_query(choice, true) {
        Ok(traced) if traced.caps.ray_query => paths.push(("ray traced", traced)),
        _ => eprintln!("no ray queries: ray-traced shadows skipped"),
    }
    for (path, gpu) in &paths {
        let mut r = renderer(gpu, Antialiasing::Off, &cam);
        // A ray-traced caster's bottom level joins a frame after it is asked for.
        r.apply(scene(sun(true), light.clone()), 0.0);
        for _ in 0..4 {
            let _ = r.capture_rgba(0.0);
        }
        let both = shot(&mut r, scene(sun(true), light.clone()));
        assert_eq!(r.last.local_shadows, 1, "{path}");
        let counts = r.draw_counts();
        assert_eq!(counts.local[0].instances(), 2, "{path}: {counts:?}");
        let traced = r.rt_shadow_stats().is_some();
        assert!(
            traced || *path == "cascades",
            "{path}: no ray-traced shadows"
        );
        if traced {
            assert!(
                counts.cascades.iter().all(|c| c.instances() == 0),
                "{path}: no cascade is drawn ({counts:?})"
            );
        } else {
            assert!(counts.shadow_triangles() > 0, "{path}: {counts:?}");
        }
        let no_spot = shot(&mut r, scene(sun(true), dark(&light)));
        let plain_spot = shot(&mut r, scene(sun(true), unshadowed(&light)));
        let plain_sun = shot(&mut r, scene(sun(false), light.clone()));
        // The spot's shadow: the floor loses the spot's light and keeps the sun's.
        assert_same(
            &format!("{path}: in the spot's shadow"),
            patch(&both, &cam, in_spot, 1),
            patch(&no_spot, &cam, in_spot, 1),
            2e-3,
        );
        assert!(
            luminance(patch(&plain_spot, &cam, in_spot, 1))
                > 1.2 * luminance(patch(&both, &cam, in_spot, 1)),
            "{path}: the spot casts no shadow beside the sun's"
        );
        // The sun's shadow: still there, the spot's light in it.
        assert!(
            luminance(patch(&plain_sun, &cam, in_sun, 1))
                > 1.2 * luminance(patch(&both, &cam, in_sun, 1)),
            "{path}: the sun casts no shadow"
        );
        assert_same(
            &format!("{path}: in the sun's shadow, the spot's light"),
            patch(&both, &cam, in_sun, 1),
            patch(&plain_spot, &cam, in_sun, 1),
            2e-3,
        );
        // Beside both, the frame of the unshadowed spot.
        assert_same(
            &format!("{path}: beside both shadows"),
            patch(&both, &cam, BESIDE, 2),
            patch(&plain_spot, &cam, BESIDE, 2),
            1e-3,
        );
    }
}
