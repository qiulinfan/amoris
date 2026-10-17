//! Evaluating a skeletal animation: a clip sampled at a time gives every node's local pose, the
//! hierarchy gives their global matrices, and a skin turns those into the joint matrices a vertex
//! shader (or a skinning pass) multiplies vertices by. Presentation math in `f32`; the simulation
//! only advances the clip's time (`Animator`).

use glam::{Mat4, Quat, Vec3, Vec4};

use crate::mesh::{AnimationClip, Channel, ChannelPath, Interpolation, ModelAsset};

/// The clip of `asset` named `name` (or numbered, `"2"`).
pub fn find_clip<'a>(asset: &'a ModelAsset, name: &str) -> Option<&'a AnimationClip> {
    asset
        .animations
        .iter()
        .find(|c| c.name == name)
        .or_else(|| {
            name.parse::<usize>()
                .ok()
                .and_then(|i| asset.animations.get(i))
        })
        .or_else(|| name.is_empty().then(|| asset.animations.first()).flatten())
}

/// `time` brought into the clip: wrapped when looping, held at the ends otherwise.
pub fn clip_time(clip: &AnimationClip, time: f32, looped: bool) -> f32 {
    if clip.duration <= 0.0 {
        return 0.0;
    }
    if looped {
        time.rem_euclid(clip.duration)
    } else {
        time.clamp(0.0, clip.duration)
    }
}

fn sample(ch: &Channel, t: f32) -> Vec4 {
    let n = ch.times.len();
    if n == 0 {
        return Vec4::ZERO;
    }
    let value = |i: usize| -> Vec4 {
        let k = if ch.interpolation == Interpolation::Cubic {
            i * 3 + 1
        } else {
            i
        };
        Vec4::from(ch.values.get(k).copied().unwrap_or([0.0; 4]))
    };
    if t <= ch.times[0] {
        return value(0);
    }
    if t >= ch.times[n - 1] {
        return value(n - 1);
    }
    let i = ch
        .times
        .partition_point(|&x| x <= t)
        .saturating_sub(1)
        .min(n - 2);
    let (t0, t1) = (ch.times[i], ch.times[i + 1]);
    let u = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
    match ch.interpolation {
        Interpolation::Step => value(i),
        Interpolation::Linear => {
            let (a, b) = (value(i), value(i + 1));
            if ch.path == ChannelPath::Rotation {
                let qa = Quat::from_vec4(a);
                let qb = Quat::from_vec4(b);
                Vec4::from(qa.slerp(qb, u))
            } else {
                a.lerp(b, u)
            }
        }
        Interpolation::Cubic => {
            // Hermite with the glTF tangents scaled by the key interval.
            let d = t1 - t0;
            let p0 = value(i);
            let m0 = Vec4::from(ch.values[i * 3 + 2]) * d;
            let p1 = value(i + 1);
            let m1 = Vec4::from(ch.values[(i + 1) * 3]) * d;
            let (u2, u3) = (u * u, u * u * u);
            let r = p0 * (2.0 * u3 - 3.0 * u2 + 1.0)
                + m0 * (u3 - 2.0 * u2 + u)
                + p1 * (-2.0 * u3 + 3.0 * u2)
                + m1 * (u3 - u2);
            if ch.path == ChannelPath::Rotation {
                Vec4::from(Quat::from_vec4(r).normalize())
            } else {
                r
            }
        }
    }
}

/// Global matrices of every skeleton node with `clip` at `time` (the rest pose without a clip).
pub fn global_pose(asset: &ModelAsset, clip: Option<&AnimationClip>, time: f32) -> Vec<Mat4> {
    let mut t: Vec<Vec3> = asset
        .skeleton
        .iter()
        .map(|n| Vec3::from(n.translation))
        .collect();
    let mut r: Vec<Quat> = asset
        .skeleton
        .iter()
        .map(|n| Quat::from_array(n.rotation))
        .collect();
    let mut s: Vec<Vec3> = asset.skeleton.iter().map(|n| Vec3::from(n.scale)).collect();
    if let Some(clip) = clip {
        for ch in &clip.channels {
            if ch.node >= t.len() {
                continue;
            }
            let v = sample(ch, time);
            match ch.path {
                ChannelPath::Translation => t[ch.node] = v.truncate(),
                ChannelPath::Rotation => r[ch.node] = Quat::from_vec4(v).normalize(),
                ChannelPath::Scale => s[ch.node] = v.truncate(),
            }
        }
    }
    let mut global: Vec<Option<Mat4>> = vec![None; asset.skeleton.len()];
    fn resolve(
        i: usize,
        asset: &ModelAsset,
        local: &dyn Fn(usize) -> Mat4,
        global: &mut Vec<Option<Mat4>>,
    ) -> Mat4 {
        if let Some(m) = global[i] {
            return m;
        }
        let m = match asset.skeleton[i].parent {
            Some(p) if p != i => resolve(p, asset, local, global) * local(i),
            _ => local(i),
        };
        global[i] = Some(m);
        m
    }
    let local = |i: usize| Mat4::from_scale_rotation_translation(s[i], r[i], t[i]);
    (0..asset.skeleton.len())
        .map(|i| resolve(i, asset, &local, &mut global))
        .collect()
}

/// The joint matrices of skin `skin` for a pose (`global_pose`), relative to the mesh's node
/// (`mesh_global`, the node the skinned mesh hangs from: its inverse cancels the node's own
/// transform, as glTF requires).
pub fn joint_matrices(
    asset: &ModelAsset,
    skin: usize,
    pose: &[Mat4],
    mesh_global: Mat4,
) -> Vec<Mat4> {
    let Some(sk) = asset.skins.get(skin) else {
        return Vec::new();
    };
    let inv_mesh = mesh_global.inverse();
    sk.joints
        .iter()
        .enumerate()
        .map(|(k, &node)| {
            let ib = sk
                .inverse_bind
                .get(k)
                .map_or(Mat4::IDENTITY, Mat4::from_cols_array);
            inv_mesh * pose.get(node).copied().unwrap_or(Mat4::IDENTITY) * ib
        })
        .collect()
}
