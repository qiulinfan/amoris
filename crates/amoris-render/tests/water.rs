//! Water regions in the renderer (docs/spec/water.md 9): a region's columns draw Smooth and Blocky,
//! a renderer fed only the tiles that changed draws what a fresh renderer given every tile draws,
//! and a camera under a region's surface sees the region's fog. Skips when there is no GPU.

use amoris_assets::frame::{
    BlockLook, ChunkUpdate, EnvironmentView, LightKindView, LightView, RenderFrame, VoxelUpdate,
    WaterLookView, WaterTileView, WaterUpdate,
};
use amoris_render::{Antialiasing, BackendChoice, CameraState, Gpu, Gtao, Renderer};
use glam::Vec3;

const W: u32 = 320;
const H: u32 = 180;
const REGION: u64 = 7;

fn gpu() -> Option<Gpu> {
    let gpu = Gpu::headless(BackendChoice::from_env()).ok();
    if gpu.is_none() {
        eprintln!("no GPU: skipped");
    }
    gpu
}

fn renderer(gpu: &Gpu, eye: Vec3, at: Vec3) -> Renderer {
    let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
    r.set_antialiasing(Antialiasing::Msaa);
    r.set_gtao(Gtao::Off);
    r.set_camera_override(Some(CameraState::look_at(eye, at)));
    r
}

/// Ground of 1 m blocks, its top at 2 m, with a pit 8 x 8 blocks whose floor's top is at 1 m.
fn ground() -> VoxelUpdate {
    let mut cells = vec![0u8; 4096];
    for x in 0..16 {
        for z in 0..16 {
            let top = if (4..12).contains(&x) && (4..12).contains(&z) {
                1
            } else {
                2
            };
            for y in 0..top {
                cells[x + 16 * (z + 16 * y)] = 1;
            }
        }
    }
    VoxelUpdate {
        id: 1,
        origin: [0.0; 3],
        cell_m: 1.0,
        blocks: Some(vec![
            BlockLook::default(),
            BlockLook {
                shape: 0,
                faces: [String::new(), String::new(), String::new()],
                fog: None,
                splash: None,
            },
        ]),
        chunks: vec![ChunkUpdate {
            key: [0, 0, 0],
            cells: Some(cells),
            states: None,
        }],
    }
}

/// A region of 0.5 m columns over the ground (32 x 32, two columns a block): the pit holds water
/// in two levels, 1.9 m over its first half and 1.3 m over its second, running toward +x.
fn column(i: u32, k: u32) -> [f32; 4] {
    let (bx, bz) = (i / 2, k / 2);
    let pit = (4..12).contains(&bx) && (4..12).contains(&bz);
    if !pit {
        return [2.0, 0.0, 0.0, 0.0];
    }
    let surface = if bx < 8 { 1.9 } else { 1.3 };
    [surface, surface - 1.0, 0.8, 0.0]
}

fn tile(key: [u32; 2], f: fn(u32, u32) -> [f32; 4]) -> WaterTileView {
    WaterTileView {
        key,
        columns: (0..256)
            .map(|c| f(key[0] * 16 + c % 16, key[1] * 16 + c / 16))
            .collect(),
    }
}

fn region(blocky: bool, keys: &[[u32; 2]], f: fn(u32, u32) -> [f32; 4]) -> WaterUpdate {
    WaterUpdate {
        id: REGION,
        origin: [0.0, 0.0],
        cell_m: 0.5,
        size: [32, 32],
        look: Some(WaterLookView {
            fog: [0.02, 0.1, 0.45, 0.3],
            absorb: [3.0, 1.2, 0.6],
            scatter: [0.02; 3],
            blocky,
            per: 2,
        }),
        tiles: keys.iter().map(|k| tile(*k, f)).collect(),
    }
}

const ALL: [[u32; 2]; 4] = [[0, 0], [1, 0], [0, 1], [1, 1]];

fn scene(tick: u64, water: Vec<WaterUpdate>, with_ground: bool) -> RenderFrame {
    let mut f = RenderFrame {
        tick,
        t_s: tick as f64 / 60.0,
        dt_s: 1.0 / 60.0,
        reset: tick == 1,
        water,
        voxels: if with_ground { vec![ground()] } else { vec![] },
        ..RenderFrame::default()
    };
    if f.reset {
        f.lights = Some(vec![LightView {
            id: 0,
            kind: LightKindView::Directional,
            position: [0.0; 3],
            direction: Vec3::new(-0.4, -1.0, -0.5).normalize().to_array(),
            color: [1.0, 0.96, 0.9],
            intensity: 6.0,
            range: 0.0,
            inner_deg: 0.0,
            outer_deg: 0.0,
            shadows: true,
        }]);
        f.cameras = Some(vec![]);
        f.environment = Some(EnvironmentView {
            sky: 1,
            sky_color: [0.3, 0.5, 0.8],
            ambient: 1.0,
            baked_gi: String::new(),
            neural_gi: String::new(),
            gi_intensity: 1.0,
            fog_density: 0.0,
            fog_color: [0.6, 0.7, 0.8],
            exposure_ev: 0.0,
            bloom: 0.0,
        });
    }
    f
}

fn shoot(r: &mut Renderer) -> Vec<u8> {
    for _ in 0..4 {
        let _ = r.capture_rgba(0.0);
    }
    r.capture_rgba(0.0).2
}

fn differing(a: &[u8], b: &[u8]) -> f64 {
    let n = a
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0)
        .filter(|(p, q)| (0..3).any(|i| p[i].abs_diff(q[i]) > 8))
        .count();
    n as f64 / (a.len() / 4) as f64
}

fn dump(name: &str, img: &[u8]) {
    if let Some(dir) = std::env::var_os("WATER_DUMP") {
        let path = std::path::PathBuf::from(dir).join(format!("{name}.rgba"));
        std::fs::write(path, img).unwrap();
    }
}

const EYE: Vec3 = Vec3::new(11.0, 5.0, 13.0);
const AT: Vec3 = Vec3::new(8.0, 1.5, 8.0);

#[test]
fn a_region_draws_smooth_and_blocky() {
    let Some(gpu) = gpu() else { return };
    let mut dry = renderer(&gpu, EYE, AT);
    dry.apply(scene(1, vec![], true), 0.0);
    let ground = shoot(&mut dry);
    let mut smooth = renderer(&gpu, EYE, AT);
    smooth.apply(scene(1, vec![region(false, &ALL, column)], true), 0.0);
    let s = shoot(&mut smooth);
    let mut blocky = renderer(&gpu, EYE, AT);
    blocky.apply(scene(1, vec![region(true, &ALL, column)], true), 0.0);
    let b = shoot(&mut blocky);
    dump("ground", &ground);
    dump("smooth", &s);
    dump("blocky", &b);
    let (ds, db, sb) = (
        differing(&ground, &s),
        differing(&ground, &b),
        differing(&s, &b),
    );
    eprintln!(
        "water over the ground: smooth {ds:.4}, blocky {db:.4}; smooth against blocky {sb:.4}"
    );
    assert!(ds > 0.05, "the smooth water shows: {ds}");
    assert!(db > 0.05, "the blocky water shows: {db}");
    // The step between the two levels is a slope in one and a wall in the other.
    assert!(sb > 0.002, "the styles differ at the step: {sb}");
}

#[test]
fn tiles_fed_one_by_one_draw_as_a_fresh_renderer_draws_them() {
    let Some(gpu) = gpu() else { return };
    // Every tile dry at first, then the tiles filled one frame at a time.
    fn empty(i: u32, k: u32) -> [f32; 4] {
        let c = column(i, k);
        [c[0] - c[1], 0.0, 0.0, 0.0]
    }
    for blocky in [false, true] {
        let mut kept = renderer(&gpu, EYE, AT);
        kept.apply(scene(1, vec![region(blocky, &ALL, empty)], true), 0.0);
        let _ = shoot(&mut kept);
        for (n, key) in ALL.iter().enumerate() {
            let mut u = region(blocky, &[*key], column);
            u.look = None;
            kept.apply(scene(2 + n as u64, vec![u], false), 0.0);
            let _ = kept.capture_rgba(0.0);
        }
        let after = shoot(&mut kept);
        let mut fresh = renderer(&gpu, EYE, AT);
        fresh.apply(scene(1, vec![region(blocky, &ALL, column)], true), 0.0);
        let want = shoot(&mut fresh);
        let d = differing(&after, &want);
        eprintln!("blocky {blocky}: kept against fresh, {d:.5} of the pixels differ");
        assert!(d < 0.0005, "tile updates draw the final water: {d}");
    }
}

#[test]
fn under_a_regions_surface_the_view_is_its_fog() {
    let Some(gpu) = gpu() else { return };
    let mut under = renderer(&gpu, Vec3::new(6.0, 1.3, 6.0), Vec3::new(10.0, 1.3, 9.0));
    under.apply(scene(1, vec![region(false, &ALL, column)], true), 0.0);
    let u = shoot(&mut under);
    let mut above = renderer(&gpu, Vec3::new(6.0, 2.6, 6.0), Vec3::new(10.0, 2.6, 9.0));
    above.apply(scene(1, vec![region(false, &ALL, column)], true), 0.0);
    let a = shoot(&mut above);
    dump("under", &u);
    let mean = |img: &[u8]| {
        let px = img.as_chunks::<4>().0;
        let n = px.len() as f64;
        [0, 1, 2].map(|c| px.iter().map(|p| f64::from(p[c])).sum::<f64>() / n)
    };
    let (mu, ma) = (mean(&u), mean(&a));
    eprintln!("mean colour under {mu:?}, above {ma:?}");
    assert!(
        mu[2] > 1.5 * mu[0] && mu[2] > mu[1],
        "blue under the water: {mu:?}"
    );
    assert!(differing(&u, &a) > 0.5, "the fog changes the view");
}
