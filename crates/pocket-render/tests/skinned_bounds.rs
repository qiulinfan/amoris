//! A skinned part's occlusion box holds every pose its frustum sphere does (docs/spec/occlusion.md
//! 5). samples/anim's hero is skinned on the CPU in its rest pose, through its clips, and with
//! each upper arm swung 90 degrees in four directions (pointing, reaching); wherever a pose stays
//! inside the grown sphere frustum culling assumes, every vertex must be inside the box the late
//! occlusion test uses. The bind pose's box grown per axis, the box this replaced, is left by the
//! swung arms along the body's thin axis.

use std::path::Path;

use glam::{Mat4, Quat, Vec3};
use pocket_assets::animation::{global_pose, joint_matrices};
use pocket_assets::mesh::ModelAsset;
use pocket_render::meshes::{MeshInfo, SKINNED_GROW, dynamic_bounds};

/// The skinned vertices of node `n` (in its mesh's space) with `asset`'s clip `clip` at `time`, or
/// its rest pose.
fn skinned(asset: &ModelAsset, n: usize, clip: Option<usize>, time: f32) -> Vec<Vec3> {
    let node = &asset.nodes[n];
    let mesh = &asset.meshes[node.mesh];
    let (Some(skin), Some(w)) = (node.skin, &mesh.skin) else {
        return Vec::new();
    };
    let pose = global_pose(asset, clip.map(|c| &asset.animations[c]), time);
    let joints = joint_matrices(asset, skin, &pose, Mat4::from_cols_array(&node.transform));
    mesh.vertices
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let p = Vec3::from(v.position);
            (0..4)
                .map(|k| joints[w.joints[i][k] as usize].transform_point3(p) * w.weights[i][k])
                .sum()
        })
        .collect()
}

#[test]
fn skinned_occlusion_boxes_hold_every_pose_their_spheres_do() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/anim/models/hero.glb");
    let asset = pocket_assets::import::import_gltf(&path).expect("samples/anim's hero");
    let mut poses = vec![("rest".to_owned(), asset.clone(), None, 0.0)];
    for (c, clip) in asset.animations.iter().enumerate() {
        for k in 0..8 {
            let time = clip.duration * k as f32 / 8.0;
            let name = format!("{} at {time:.2} s", clip.name);
            poses.push((name, asset.clone(), Some(c), time));
        }
    }
    let arms: Vec<usize> = (0..asset.skeleton.len())
        .filter(|&i| {
            let n = asset.skeleton[i].name.to_ascii_lowercase();
            n.ends_with("leftarm") || n.ends_with("rightarm")
        })
        .collect();
    assert_eq!(arms.len(), 2, "two upper arms in {:?}", asset.skeleton);
    for &arm in &arms {
        for (axis, sign) in [
            (Vec3::X, 1.0),
            (Vec3::X, -1.0),
            (Vec3::Z, 1.0),
            (Vec3::Z, -1.0),
        ] {
            let mut a = asset.clone();
            let r = Quat::from_array(a.skeleton[arm].rotation);
            let swing = Quat::from_axis_angle(axis, sign * std::f32::consts::FRAC_PI_2);
            a.skeleton[arm].rotation = (r * swing).to_array();
            let name = format!("{} swung {sign:+} about {axis}", asset.skeleton[arm].name);
            poses.push((name, a, None, 0.0));
        }
    }

    let mut checked = 0;
    let mut outside_old_box = 0;
    for n in 0..asset.nodes.len() {
        let node = &asset.nodes[n];
        let mesh = &asset.meshes[node.mesh];
        if node.skin.is_none() || mesh.skin.is_none() {
            continue;
        }
        let b = &mesh.bounds;
        let src = MeshInfo {
            center: b.center,
            radius: b.radius,
            box_center: b.center,
            box_half: std::array::from_fn(|i| (b.max[i] - b.min[i]) * 0.5),
            ..MeshInfo::default()
        };
        let d = dynamic_bounds(&src, SKINNED_GROW);
        let (c, half) = (Vec3::from(d.box_center), Vec3::from(d.box_half));
        let old_half = Vec3::from(src.box_half) * SKINNED_GROW;
        for (name, a, clip, time) in &poses {
            let vs = skinned(a, n, *clip, *time);
            let worst = vs
                .iter()
                .map(|p| (*p - Vec3::from(d.center)).length() / d.radius)
                .fold(0.0, f32::max);
            if worst > 1.0 {
                eprintln!("{}: {name} leaves the grown sphere ({worst:.2})", mesh.name);
                continue;
            }
            for p in &vs {
                let off = (*p - c).abs();
                assert!(
                    off.cmple(half).all(),
                    "{}: {name}: a vertex {off} from the centre, outside the box {half}",
                    mesh.name
                );
            }
            if vs.iter().any(|p| !(*p - c).abs().cmple(old_half).all()) {
                outside_old_box += 1;
            }
            checked += 1;
        }
    }
    eprintln!("{checked} poses checked, {outside_old_box} outside the bind box grown per axis");
    assert!(checked > 0);
    // The swung arms are the case the per-axis box missed.
    assert!(outside_old_box > 0);
}
