//! Block volumes in the renderer (docs/spec/voxels.md 7.2): a volume's chunks draw, the id pass
//! names the volume's entity for them, and after blocks are broken and placed, chunk by chunk and
//! across chunk borders, the kept renderer (its chunk meshes replaced in the mesh pool, freed bytes
//! reused) draws what a fresh renderer given the final cells draws. Skips when there is no GPU.

use amoris_assets::frame::{
    BlockLook, ChunkUpdate, EnvironmentView, LightKindView, LightView, RenderFrame, VoxelUpdate,
};
use amoris_render::{Antialiasing, BackendChoice, CameraState, Gpu, Gtao, Renderer};
use glam::Vec3;

const W: u32 = 320;
const H: u32 = 180;
const VOLUME: u64 = 42;

fn gpu() -> Option<Gpu> {
    let gpu = Gpu::headless(BackendChoice::from_env()).ok();
    if gpu.is_none() {
        eprintln!("no GPU: skipped");
    }
    gpu
}

fn renderer(gpu: &Gpu) -> Renderer {
    let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
    r.set_antialiasing(Antialiasing::Msaa);
    r.set_gtao(Gtao::Off);
    r.set_camera_override(Some(CameraState::look_at(
        Vec3::new(8.0, 14.0, 30.0),
        Vec3::new(8.0, 2.0, 0.0),
    )));
    r
}

/// Inline-material blocks (no asset to load): a cube, and a plant.
fn blocks() -> Vec<BlockLook> {
    vec![
        BlockLook::default(),
        BlockLook {
            shape: 0,
            faces: [String::new(), String::new(), String::new()],
            fog: None,
            splash: None,
        },
        BlockLook {
            shape: 1,
            faces: [String::new(), String::new(), String::new()],
            fog: None,
            splash: None,
        },
    ]
}

/// Cells as a map from chunk key to 4096 indices.
type Cells = std::collections::BTreeMap<[i32; 3], Vec<u8>>;

fn set(cells: &mut Cells, c: [i32; 3], b: u8) {
    let key = c.map(|x| x.div_euclid(16));
    let [x, y, z] = c.map(|x| x.rem_euclid(16) as usize);
    cells.entry(key).or_insert_with(|| vec![0; 4096])[x + 16 * (z + 16 * y)] = b;
}

fn ground() -> Cells {
    let mut cells = Cells::new();
    for x in -4..20 {
        for z in -12..4 {
            for y in 0..3 {
                set(&mut cells, [x, y, z], 1);
            }
        }
    }
    set(&mut cells, [3, 3, 0], 2);
    cells
}

fn update(cells: &Cells, keys: &[[i32; 3]], with_blocks: bool) -> VoxelUpdate {
    VoxelUpdate {
        id: VOLUME,
        origin: [0.0, 0.0, 0.0],
        cell_m: 1.0,
        blocks: with_blocks.then(blocks),
        chunks: keys
            .iter()
            .map(|k| ChunkUpdate {
                key: *k,
                cells: cells.get(k).filter(|c| c.iter().any(|&b| b != 0)).cloned(),
                states: None,
            })
            .collect(),
    }
}

fn scene(tick: u64, voxels: Vec<VoxelUpdate>) -> RenderFrame {
    let mut f = RenderFrame {
        tick,
        t_s: tick as f64 / 60.0,
        dt_s: 1.0 / 60.0,
        reset: tick == 1,
        voxels,
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

fn shoot(r: &mut Renderer, t: f64) -> (Vec<u8>, Vec<(u64, f32)>) {
    r.request_visible();
    let mut visible = None;
    for _ in 0..30 {
        let _ = r.capture_rgba(t);
        if let Some(v) = r.take_visible() {
            visible = Some(v);
            break;
        }
    }
    (r.capture_rgba(t).2, visible.expect("the id pass answers"))
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

#[test]
fn edited_volumes_draw_as_a_fresh_renderer_draws_them() {
    let Some(gpu) = gpu() else { return };
    let mut cells = ground();
    let all: Vec<[i32; 3]> = cells.keys().copied().collect();
    let mut kept = renderer(&gpu);
    kept.apply(scene(1, vec![update(&cells, &all, true)]), 0.0);
    let (first, seen) = shoot(&mut kept, 0.0);
    let share = seen.iter().find(|v| v.0 == VOLUME).map_or(0.0, |v| v.1);
    assert!(
        share > 0.03,
        "the volume's entity covers {share} of the view: {seen:?}"
    );

    // Break a pit across the chunk border at x = 16, remove a whole chunk's blocks, place a tower.
    let mut tick = 1;
    let mut edits: Vec<Vec<[i32; 3]>> = Vec::new();
    for x in 14..18 {
        for y in 1..3 {
            set(&mut cells, [x, y, -2], 0);
        }
    }
    edits.push(vec![[0, 0, -1], [1, 0, -1]]);
    for x in -4..0 {
        for z in -12..4 {
            for y in 0..3 {
                set(&mut cells, [x, y, z], 0);
            }
        }
    }
    edits.push(vec![[-1, 0, -1], [-1, 0, 0]]);
    for y in 3..9 {
        set(&mut cells, [8, y, 1], 1);
    }
    edits.push(vec![[0, 0, 0]]);
    for keys in edits {
        tick += 1;
        kept.apply(scene(tick, vec![update(&cells, &keys, false)]), 0.0);
        let _ = kept.capture_rgba(0.0);
    }
    let (after, _) = shoot(&mut kept, 0.0);
    assert!(differing(&first, &after) > 0.01, "the edits show");

    let mut fresh = renderer(&gpu);
    let all: Vec<[i32; 3]> = cells.keys().copied().collect();
    fresh.apply(scene(1, vec![update(&cells, &all, true)]), 0.0);
    let (want, _) = shoot(&mut fresh, 0.0);
    if let Some(dir) = std::env::var_os("VOXELS_DUMP") {
        let dir = std::path::PathBuf::from(dir);
        std::fs::write(dir.join("kept.rgba"), &after).unwrap();
        std::fs::write(dir.join("fresh.rgba"), &want).unwrap();
    }
    let d = differing(&after, &want);
    eprintln!("kept against fresh: {:.5} of the pixels differ", d);
    assert!(
        d < 0.0005,
        "the kept renderer draws the edited volume as a fresh one: {d}"
    );
}

#[test]
fn under_a_liquid_the_view_is_its_fog() {
    let Some(gpu) = gpu() else { return };
    let fog = [0.02, 0.10, 0.45, 0.3];
    let mut looks = blocks();
    looks.push(BlockLook {
        shape: 2,
        faces: [String::new(), String::new(), String::new()],
        fog: Some(fog),
        splash: None,
    });
    let mut cells = Cells::new();
    for x in -8..8 {
        for z in -8..8 {
            set(&mut cells, [x, 0, z], 1);
            for y in 1..6 {
                set(&mut cells, [x, y, z], 3);
            }
        }
    }
    let keys: Vec<[i32; 3]> = cells.keys().copied().collect();
    let mut u = update(&cells, &keys, true);
    u.blocks = Some(looks);
    let mut r = renderer(&gpu);
    r.set_camera_override(Some(CameraState::look_at(
        Vec3::new(0.5, 3.0, 0.5),
        Vec3::new(6.0, 3.5, -6.0),
    )));
    r.apply(scene(1, vec![u.clone()]), 0.0);
    let (under, _) = shoot(&mut r, 0.0);
    // Above the water the same scene has no such fog.
    let mut above = renderer(&gpu);
    above.set_camera_override(Some(CameraState::look_at(
        Vec3::new(0.5, 9.0, 0.5),
        Vec3::new(6.0, 9.5, -6.0),
    )));
    above.apply(scene(1, vec![u]), 0.0);
    let (dry, _) = shoot(&mut above, 0.0);
    let mean = |img: &[u8]| {
        let px = img.as_chunks::<4>().0;
        let n = px.len() as f64;
        [0, 1, 2].map(|c| px.iter().map(|p| f64::from(p[c])).sum::<f64>() / n)
    };
    let (u, d) = (mean(&under), mean(&dry));
    eprintln!("mean colour under {u:?}, above {d:?}");
    assert!(
        u[2] > 1.5 * u[0] && u[2] > u[1],
        "blue under the liquid: {u:?}"
    );
    assert!(differing(&under, &dry) > 0.5, "the fog changes the view");
}
