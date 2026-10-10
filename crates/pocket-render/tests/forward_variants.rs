//! The forward pass's four pipeline variants (renderer.rs `FORWARD_ENTRIES`: opaque, alpha-masked,
//! double-sided, both) draw what their materials ask for. Under an orthographic camera stand eight
//! quads, one per variant facing the camera and one per variant facing away: a single-sided quad's
//! back is culled, a double-sided quad's back is drawn, and an alpha-masked quad's checker of
//! cut-out holes shows the sky behind about half of it. Each quad's pixels are compared with the sky
//! drawn alone. The pipelines `Renderer::new` builds and those a change of anti-aliasing rebuilds
//! are checked; both assemble the variants from two concurrent tasks. Skips when there is no GPU.

use glam::{Quat, Vec3};
use pocket_assets::frame::{
    EnvironmentView, InstanceUpdate, LightKindView, LightView, Look, Pose, RenderFrame,
};
use pocket_assets::mesh::{AlphaMode, ImageData, MaterialData, ModelAsset, NodeData};
use pocket_assets::primitives::primitive;
use pocket_render::{Antialiasing, BackendChoice, CameraState, Gpu, Renderer};

const W: u32 = 320;
const H: u32 = 160;
/// The view's height in metres (20 pixels a metre).
const ORTHO: f32 = 8.0;
const MODEL: &str = "test/quads.glb";
/// The materials by variant (bit 1 alpha-masked, bit 2 double-sided, as materials.rs `variant`).
const VARIANTS: [&str; 4] = ["opaque", "masked", "double", "masked_double"];
/// The quads' centres along x, by variant; the row facing the camera stands at y = 2, the row
/// facing away at y = -2. A quad is 3 m square.
const COLUMNS: [f32; 4] = [-6.0, -2.0, 2.0, 6.0];

/// A unit quad per variant (the plane primitive, facing +y), orange; the alpha-masked ones cut by
/// a checker of 8 x 8 holes in their texture's alpha.
fn model() -> ModelAsset {
    let size = 64u32;
    let mut rgba8 = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let on = ((x / 8) + (y / 8)) % 2 == 0;
            rgba8.extend_from_slice(&[255, 255, 255, if on { 255 } else { 0 }]);
        }
    }
    let mut asset = ModelAsset {
        images: vec![ImageData {
            name: "holes".into(),
            width: size,
            height: size,
            rgba8,
            srgb: true,
        }],
        ..ModelAsset::default()
    };
    for (i, name) in VARIANTS.into_iter().enumerate() {
        let masked = i & 1 != 0;
        let mut mesh = primitive("plane").expect("the plane primitive");
        mesh.name = name.into();
        mesh.material = Some(i);
        asset.meshes.push(mesh);
        asset.materials.push(MaterialData {
            name: name.into(),
            base_color: [1.0, 0.35, 0.05, 1.0],
            roughness: 0.6,
            base_color_texture: masked.then_some(0),
            alpha_mode: if masked {
                AlphaMode::Mask
            } else {
                AlphaMode::Opaque
            },
            double_sided: i & 2 != 0,
            ..MaterialData::default()
        });
        asset.nodes.push(NodeData {
            name: name.into(),
            mesh: i,
            transform: glam::Mat4::IDENTITY.to_cols_array(),
            skin: None,
        });
    }
    asset
}

/// A blue sky lit by a sun behind the camera; with `quads` the eight quads.
fn scene(quads: bool) -> RenderFrame {
    let mut instances = Vec::new();
    if quads {
        // Facing the camera (+z) at y = 2, facing away at y = -2.
        for (row, sign) in [1.0f32, -1.0].into_iter().enumerate() {
            for (v, name) in VARIANTS.iter().enumerate() {
                instances.push(InstanceUpdate {
                    id: 1 + (row * 4 + v) as u64,
                    pose: Some(Pose {
                        position: [COLUMNS[v], sign * 2.0, 0.0],
                        // The plane faces +y; a quarter turn about x faces it +z, the other way
                        // -z.
                        rotation: Quat::from_rotation_x(sign * std::f32::consts::FRAC_PI_2)
                            .to_array(),
                        scale: [3.0, 1.0, 3.0],
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
                });
            }
        }
    }
    RenderFrame {
        tick: 1,
        dt_s: 1.0 / 60.0,
        reset: true,
        instances,
        lights: Some(vec![LightView {
            id: 0,
            kind: LightKindView::Directional,
            position: [0.0; 3],
            direction: Vec3::new(0.3, -0.4, -1.0).normalize().to_array(),
            color: [1.0; 3],
            intensity: 4.0,
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
        }),
        ..RenderFrame::default()
    }
}

/// The share of the pixels inside the middle 2 m of the quad at (`x`, `y`) that differ from the
/// sky behind it.
fn covered(img: &[f32], sky: &[f32], x: f32, y: f32) -> f64 {
    let px = W as f32 / (ORTHO * W as f32 / H as f32);
    let (cx, cy) = (W as f32 * 0.5 + x * px, H as f32 * 0.5 - y * px);
    let (mut n, mut differ) = (0u32, 0u32);
    for py in (cy - px) as u32..(cy + px) as u32 {
        for qx in (cx - px) as u32..(cx + px) as u32 {
            let i = ((py * W + qx) * 4) as usize;
            n += 1;
            if (0..3).any(|k| (img[i + k] - sky[i + k]).abs() > 0.05) {
                differ += 1;
            }
        }
    }
    f64::from(differ) / f64::from(n)
}

fn check(r: &mut Renderer, what: &str) {
    let mut camera = CameraState::look_at(Vec3::new(0.0, 0.0, 10.0), Vec3::ZERO);
    camera.ortho_height = Some(ORTHO);
    r.set_camera_override(Some(camera));
    r.apply(scene(false), 0.0);
    let sky = r.capture_hdr(0.0).2;
    r.apply(scene(true), 0.0);
    let img = r.capture_hdr(0.0).2;
    for (v, name) in VARIANTS.iter().enumerate() {
        let (masked, double) = (v & 1 != 0, v & 2 != 0);
        for (facing, y) in [("front", 2.0), ("back", -2.0)] {
            let c = covered(&img, &sky, COLUMNS[v], y);
            eprintln!(
                "{what}: {name} seen from the {facing}: {:.0}% covered",
                c * 100.0
            );
            let drawn = facing == "front" || double;
            let expected = if !drawn {
                0.0..0.01
            } else if masked {
                0.25..0.75
            } else {
                0.99..1.01
            };
            assert!(
                expected.contains(&c),
                "{what}: the {name} quad seen from the {facing} covers {:.1}% of its middle, \
                 expected {:.0} to {:.0}%",
                c * 100.0,
                expected.start * 100.0,
                expected.end * 100.0
            );
        }
    }
}

#[test]
fn every_forward_variant_draws_its_material() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    // The pipelines Renderer::new builds.
    let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
    r.add_model(MODEL, &model());
    let aa = r.antialiasing();
    check(&mut r, &format!("{aa:?} from start-up"));
    // Those a change of the opaque pass's format rebuilds (renderer.rs `reformat`).
    let other = if aa == Antialiasing::Msaa {
        Antialiasing::Off
    } else {
        Antialiasing::Msaa
    };
    r.set_antialiasing(other);
    check(&mut r, &format!("{other:?} rebuilt"));
}
