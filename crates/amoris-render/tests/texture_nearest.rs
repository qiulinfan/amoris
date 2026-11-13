//! Pixel art keeps its pixels (docs/spec/voxels.md 7.2): an image whose glTF sampler magnifies
//! with `NEAREST` is imported marked (`ImageData::nearest`) and reaches its texture layer by point
//! sampling, where any other image is stretched by a linear sampler. A quad textured with 16 x 16
//! art stands under an orthographic camera, close up (two of the art's texels across the frame:
//! the layer's first level), at a middle distance and from afar (the mip chain). The marked art
//! draws, byte for byte, the frames of the art repeated 64 times a texel to the layer's size by
//! the test itself, the image such a layer must hold; the same art unmarked draws another, blurred
//! frame; and a model that holds the art both ways, as the importer gives an image sampled both
//! ways, draws each material with its own filter. Skips when there is no GPU.

use amoris_assets::frame::{
    EnvironmentView, InstanceUpdate, LightKindView, LightView, Look, Pose, RenderFrame,
};
use amoris_assets::mesh::{ImageData, MaterialData, ModelAsset, NodeData};
use amoris_assets::primitives::primitive;
use amoris_render::materials::TEX_SIZE;
use amoris_render::{Antialiasing, BackendChoice, CameraState, Gpu, Gtao, Renderer};
use glam::{Quat, Vec3};

const W: u32 = 256;
const H: u32 = 256;
const MODEL: &str = "test/pixels.glb";
/// The art's side in texels, and the quad's in metres: a texel is a metre.
const ART: u32 = 16;
/// The views by their height in metres: two texels fill the frame; the quad is 26 pixels wide;
/// the quad is 6 pixels wide.
const VIEWS: [(&str, f32); 3] = [("close", 2.0), ("middle", 160.0), ("far", 640.0)];

/// A checker of light and dark texels, each with a tint of its own: neighbours differ strongly,
/// and no two texels are equal, so a texel taken from the wrong place shows.
fn art(x: u32, y: u32) -> [u8; 4] {
    let (r, g) = (x as u8 * 4, y as u8 * 4);
    if (x + y).is_multiple_of(2) {
        [255 - r, 255 - g, 200, 255]
    } else {
        [r, g, 40, 255]
    }
}

/// The art `scale` times as large, every texel repeated.
fn image(scale: u32, nearest: bool) -> ImageData {
    let size = ART * scale;
    let mut rgba8 = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            rgba8.extend_from_slice(&art(x / scale, y / scale));
        }
    }
    ImageData {
        levels: None,
        name: "art".into(),
        width: size,
        height: size,
        rgba8,
        srgb: true,
        nearest,
    }
}

/// A model of one quad (the plane primitive) per image, named `names[i]`, its base color image
/// `i`.
fn model(images: Vec<ImageData>, names: &[&str]) -> ModelAsset {
    let mut asset = ModelAsset {
        images,
        ..ModelAsset::default()
    };
    for (i, name) in names.iter().enumerate() {
        let mut mesh = primitive("plane").expect("the plane primitive");
        mesh.name = (*name).into();
        mesh.material = Some(i);
        asset.meshes.push(mesh);
        asset.materials.push(MaterialData {
            name: (*name).into(),
            roughness: 1.0,
            base_color_texture: Some(i),
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

/// The quad `name` of the model, 16 m square and facing the camera (+z), lit from behind it.
fn scene(name: &str) -> RenderFrame {
    RenderFrame {
        tick: 1,
        dt_s: 1.0 / 60.0,
        reset: true,
        instances: vec![InstanceUpdate {
            id: 1,
            pose: Some(Pose {
                position: [0.0; 3],
                // The plane faces +y; a quarter turn about x faces it +z.
                rotation: Quat::from_rotation_x(std::f32::consts::FRAC_PI_2).to_array(),
                scale: [ART as f32, 1.0, ART as f32],
            }),
            look: Some(Look {
                mesh: format!("{MODEL}#{name}"),
                material: String::new(),
                color: [1.0; 4],
                metallic: 0.0,
                roughness: 1.0,
                transmission: None,
                ior: None,
                emissive: [0.0; 3],
                cast_shadows: false,
                visible: true,
            }),
            anim: None,
        }],
        lights: Some(vec![LightView {
            id: 0,
            kind: LightKindView::Directional,
            position: [0.0; 3],
            direction: Vec3::new(0.3, -0.4, -1.0).normalize().to_array(),
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

/// The quad `name` of `asset` drawn by a fresh renderer from each of [`VIEWS`].
fn frames(gpu: &Gpu, asset: &ModelAsset, name: &str) -> Vec<Vec<u8>> {
    let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
    r.set_antialiasing(Antialiasing::Msaa);
    r.set_gtao(Gtao::Off);
    r.add_model(MODEL, asset);
    r.apply(scene(name), 0.0);
    VIEWS
        .iter()
        .map(|&(_, height)| {
            // Over the corner the art's texels (5, 4), (6, 4), (5, 5) and (6, 5) share.
            let centre = Vec3::new(-2.0, 3.0, 0.0);
            let mut camera = CameraState::look_at(centre + Vec3::Z * 10.0, centre);
            camera.ortho_height = Some(height);
            r.set_camera_override(Some(camera));
            r.capture_rgba(0.0).2
        })
        .collect()
}

/// The share of pixels of which a color channel differs by more than `over`, and the largest
/// difference.
fn differing(a: &[u8], b: &[u8], over: u8) -> (f64, u8) {
    assert_eq!(a.len(), b.len());
    let (mut n, mut max) = (0u32, 0u8);
    for (p, q) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0) {
        let d = (0..3).map(|k| p[k].abs_diff(q[k])).max().unwrap_or(0);
        max = max.max(d);
        n += u32::from(d > over);
    }
    (f64::from(n) / f64::from(W * H), max)
}

#[test]
fn a_marked_image_keeps_its_pixels() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    let scale = TEX_SIZE / ART;
    assert_eq!(scale * ART, TEX_SIZE, "the art's side divides the layer's");
    // What blockcraft shipped before the importer read samplers: the art at the layer's size.
    let repeat = frames(&gpu, &model(vec![image(scale, false)], &["quad"]), "quad");
    let marked = frames(&gpu, &model(vec![image(1, true)], &["quad"]), "quad");
    let smooth = frames(&gpu, &model(vec![image(1, false)], &["quad"]), "quad");
    // An image sampled both ways, as the importer hands it over: the file's image unmarked, its
    // marked copy after it, a material on each.
    let both = model(vec![image(1, false), image(1, true)], &["smooth", "pixel"]);
    let both_pixel = frames(&gpu, &both, "pixel");
    let both_smooth = frames(&gpu, &both, "smooth");
    for (v, (view, _)) in VIEWS.iter().enumerate() {
        // The quad is in the frame: the frames are not one flat color.
        let pixels = repeat[v].as_chunks::<4>().0;
        assert!(
            pixels.iter().any(|p| *p != pixels[0]),
            "{view}: the frame is flat"
        );
        let (share, max) = differing(&marked[v], &repeat[v], 0);
        eprintln!(
            "{view}: marked against the repeated art: {:.2}% of the pixels differ, at most {max}",
            share * 100.0
        );
        assert!(
            marked[v] == repeat[v],
            "{view}: the marked art differs from the art repeated to the layer's size on \
             {:.2}% of the pixels, by up to {max}",
            share * 100.0
        );
        assert!(
            both_pixel[v] == repeat[v],
            "{view}: the marked copy of an image sampled both ways differs from the repeated art"
        );
        assert!(
            both_smooth[v] == smooth[v],
            "{view}: the unmarked image beside its marked copy differs from the unmarked art"
        );
    }
    // Close up the unmarked art is a blur of its texels: far from the marked art's frame.
    let (share, max) = differing(&smooth[0], &marked[0], 8);
    eprintln!(
        "close: unmarked against marked: {:.1}% of the pixels differ by more than 8, at most {max}",
        share * 100.0
    );
    assert!(
        share > 0.25,
        "close: the unmarked art differs from the marked art by more than 8 on only {:.1}% of \
         the pixels (at most {max}): the mark changes nothing",
        share * 100.0
    );
    // And the marked art is flat inside a texel: the frame's four quadrants are the four texels
    // around its centre, and away from their edges each holds one color to within the lighting's
    // slope, where the unmarked art runs from one texel's color to the next.
    let spread = |img: &[u8]| {
        let mut worst = 0u8;
        for (qx, qy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let at = |x: u32, y: u32| {
                let i = (((qy * H / 2 + y) * W + qx * W / 2 + x) * 4) as usize;
                [img[i], img[i + 1], img[i + 2]]
            };
            let middle = at(W / 4, H / 4);
            for y in W / 16..H / 2 - H / 16 {
                for x in W / 16..W / 2 - W / 16 {
                    let p = at(x, y);
                    worst = worst.max((0..3).map(|k| p[k].abs_diff(middle[k])).max().unwrap_or(0));
                }
            }
        }
        worst
    };
    let (flat, blurred) = (spread(&marked[0]), spread(&smooth[0]));
    eprintln!("close: a texel's colors spread by {flat} marked, by {blurred} unmarked");
    assert!(
        flat <= 4,
        "close: a texel of the marked art spreads over {flat} levels"
    );
    assert!(
        blurred > 64,
        "close: a texel of the unmarked art spreads over only {blurred} levels"
    );
}
