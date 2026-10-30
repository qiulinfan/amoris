//! Skeletons and clips as the simulation keeps them (animation.md 2): every node of a skinned file
//! with its rest transform, and clips as keyed tracks by joint name, bound to a target skeleton by
//! name with the root's and pelvis's translation scaled to the target (retargeting).

use serde::{Deserialize, Serialize};

use super::math::{self, Q, V3, Xform};

/// One node of a skeleton file, in the file's order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Joint {
    pub name: String,
    pub parent: Option<u32>,
    pub rest: Xform,
}

/// Every node of a file (bones are nodes), in the file's order: the order the renderer draws the
/// file's skeleton in, so a pose's index is a node's.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Skeleton {
    pub joints: Vec<Joint>,
    /// Joints in an order where a parent comes before its children.
    pub order: Vec<u32>,
}

impl Skeleton {
    pub fn new(joints: Vec<Joint>) -> Skeleton {
        let mut order = Vec::with_capacity(joints.len());
        let mut placed = vec![false; joints.len()];
        // Repeated passes keep the file's order among siblings; a cycle (a broken file) is cut.
        for _ in 0..joints.len() {
            let before = order.len();
            for (i, j) in joints.iter().enumerate() {
                let ready = j
                    .parent
                    .is_none_or(|p| (p as usize) >= joints.len() || placed[p as usize]);
                if !placed[i] && ready {
                    placed[i] = true;
                    order.push(i as u32);
                }
            }
            if order.len() == before {
                break;
            }
        }
        for (i, p) in placed.iter().enumerate() {
            if !p {
                order.push(i as u32);
            }
        }
        Skeleton { joints, order }
    }

    pub fn find(&self, name: &str) -> Option<usize> {
        self.joints.iter().position(|j| j.name == name)
    }

    pub fn rest(&self) -> Vec<Xform> {
        self.joints.iter().map(|j| j.rest).collect()
    }

    /// Every joint's transform in the file's space, from local ones.
    pub fn globals(&self, locals: &[Xform]) -> Vec<Xform> {
        let mut g = locals.to_vec();
        for &i in &self.order {
            let i = i as usize;
            if let Some(p) = self.joints[i].parent.map(|p| p as usize)
                && p < g.len()
                && p != i
            {
                g[i] = g[p].mul(&locals[i]);
            }
        }
        g
    }

    /// One joint's transform in the file's space (its chain of parents only).
    pub fn global(&self, locals: &[Xform], joint: usize) -> Xform {
        let mut x = locals[joint];
        let mut at = self.joints[joint].parent;
        let mut steps = 0;
        while let Some(p) = at.map(|p| p as usize) {
            if p >= locals.len() || steps > self.joints.len() {
                break;
            }
            x = locals[p].mul(&x);
            at = self.joints[p].parent;
            steps += 1;
        }
        x
    }
}

/// Keys of one property: times, and 3 (translation, scale) or 4 (rotation) values each.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Keys {
    pub times: Vec<f32>,
    pub values: Vec<f32>,
    /// Hold each key until the next (glTF STEP).
    #[serde(default)]
    pub step: bool,
}

impl Keys {
    pub fn is_empty(&self) -> bool {
        self.times.is_empty()
    }

    /// The keys around `t` and the share of the way from the first to the second.
    fn at(&self, t: f32) -> (usize, usize, f32) {
        let n = self.times.len();
        if t <= self.times[0] || n == 1 {
            return (0, 0, 0.0);
        }
        if t >= self.times[n - 1] {
            return (n - 1, n - 1, 0.0);
        }
        let i = self
            .times
            .partition_point(|&x| x <= t)
            .saturating_sub(1)
            .min(n - 2);
        let (t0, t1) = (self.times[i], self.times[i + 1]);
        if self.step || t1 <= t0 {
            return (i, i, 0.0);
        }
        (i, i + 1, (t - t0) / (t1 - t0))
    }

    fn v3(&self, k: usize) -> V3 {
        [
            self.values[3 * k],
            self.values[3 * k + 1],
            self.values[3 * k + 2],
        ]
    }

    fn q(&self, k: usize) -> Q {
        [
            self.values[4 * k],
            self.values[4 * k + 1],
            self.values[4 * k + 2],
            self.values[4 * k + 3],
        ]
    }

    pub fn sample3(&self, t: f32) -> V3 {
        let (a, b, u) = self.at(t);
        math::lerp(self.v3(a), self.v3(b), u)
    }

    pub fn sample_q(&self, t: f32) -> Q {
        let (a, b, u) = self.at(t);
        math::nlerp(self.q(a), self.q(b), u)
    }
}

/// One joint's keys in a clip.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub joint: String,
    pub t: Keys,
    pub r: Keys,
    pub s: Keys,
}

/// A clip as made on its source skeleton.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub name: String,
    pub duration: f32,
    pub tracks: Vec<Track>,
}

/// A clip file: the source skeleton the clips were made on (for retargeting) and its clips.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ClipSet {
    pub skeleton: Skeleton,
    pub clips: Vec<Clip>,
}

/// How a bound track's translation is taken.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Translate {
    /// The target's rest translation (the clip's would change the bone's length).
    Rest,
    /// The clip's, scaled by the target's size over the source's.
    Scaled(f32),
}

/// A clip bound to a target skeleton: per track the target joint and how it translates.
#[derive(Clone, Debug)]
pub struct Bound {
    pub name: String,
    pub duration: f32,
    tracks: Vec<(usize, usize, Translate)>,
    /// The clip's index in its set.
    clip: usize,
    set: usize,
}

impl Bound {
    /// Binds `clip` of `set` (the set's index `set_index`) to `target`: tracks whose joint the
    /// target lacks are left out; `translate` names the joints whose clip translation is kept,
    /// scaled by the ratio of the target's `scale_by` joint's rest offset to the source's.
    pub fn new(
        target: &Skeleton,
        set: &ClipSet,
        set_index: usize,
        clip: usize,
        translate: &[String],
        scale_by: &str,
    ) -> Bound {
        let ratio = match (target.find(scale_by), set.skeleton.find(scale_by)) {
            (Some(a), Some(b)) => {
                let (la, lb) = (
                    math::length(target.joints[a].rest.t),
                    math::length(set.skeleton.joints[b].rest.t),
                );
                if lb > 0.0 { la / lb } else { 1.0 }
            }
            _ => 1.0,
        };
        let c = &set.clips[clip];
        let tracks = c
            .tracks
            .iter()
            .enumerate()
            .filter_map(|(k, tr)| {
                let j = target.find(&tr.joint)?;
                let mode = if translate.contains(&tr.joint) {
                    Translate::Scaled(ratio)
                } else {
                    Translate::Rest
                };
                Some((j, k, mode))
            })
            .collect();
        Bound {
            name: c.name.clone(),
            duration: c.duration,
            tracks,
            clip,
            set: set_index,
        }
    }

    /// The clip at `t` seconds (held at its ends) written over `pose` (the target's rest for
    /// joints it does not key).
    pub fn sample(&self, sets: &[ClipSet], t: f32, pose: &mut [Xform]) {
        let clip = &sets[self.set].clips[self.clip];
        for &(j, k, mode) in &self.tracks {
            let tr = &clip.tracks[k];
            let x = &mut pose[j];
            if !tr.r.is_empty() {
                x.r = tr.r.sample_q(t);
            }
            if !tr.s.is_empty() {
                x.s = tr.s.sample3(t);
            }
            if let Translate::Scaled(k) = mode
                && !tr.t.is_empty()
            {
                x.t = math::scale(tr.t.sample3(t), k);
            }
        }
    }

    /// The clip's translation of `joint` at `t`, in the target's scale, if it keys and keeps it.
    pub fn translation(&self, sets: &[ClipSet], joint: usize, t: f32) -> Option<V3> {
        let clip = &sets[self.set].clips[self.clip];
        self.tracks.iter().find_map(|&(j, k, mode)| match mode {
            Translate::Scaled(s) if j == joint && !clip.tracks[k].t.is_empty() => {
                Some(math::scale(clip.tracks[k].t.sample3(t), s))
            }
            _ => None,
        })
    }

    /// The clip's rotation of `joint` at `t`, if it keys it.
    pub fn rotation(&self, sets: &[ClipSet], joint: usize, t: f32) -> Option<Q> {
        let clip = &sets[self.set].clips[self.clip];
        self.tracks.iter().find_map(|&(j, k, _)| {
            (j == joint && !clip.tracks[k].r.is_empty()).then(|| clip.tracks[k].r.sample_q(t))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joint(name: &str, parent: Option<u32>, t: V3) -> Joint {
        Joint {
            name: name.into(),
            parent,
            rest: Xform {
                t,
                ..Xform::IDENTITY
            },
        }
    }

    fn keys(times: &[f32], values: &[f32]) -> Keys {
        Keys {
            times: times.to_vec(),
            values: values.to_vec(),
            step: false,
        }
    }

    #[test]
    fn a_clip_keeps_the_targets_bone_lengths_and_scales_its_pelvis() {
        // The source is a tall body, the target a short one with a longer forearm.
        let source = Skeleton::new(vec![
            joint("root", None, [0.0; 3]),
            joint("pelvis", Some(0), [0.0, 1.0, 0.0]),
            joint("arm", Some(1), [0.5, 0.0, 0.0]),
        ]);
        let target = Skeleton::new(vec![
            joint("arm", Some(2), [0.7, 0.0, 0.0]),
            joint("root", None, [0.0; 3]),
            joint("pelvis", Some(1), [0.0, 0.8, 0.0]),
        ]);
        let set = ClipSet {
            skeleton: source,
            clips: vec![Clip {
                name: "bob".into(),
                duration: 1.0,
                tracks: vec![
                    Track {
                        joint: "pelvis".into(),
                        t: keys(&[0.0, 1.0], &[0.0, 1.0, 0.0, 0.0, 0.5, 0.0]),
                        r: Keys::default(),
                        s: Keys::default(),
                    },
                    Track {
                        joint: "arm".into(),
                        t: keys(&[0.0], &[9.0, 9.0, 9.0]),
                        r: keys(&[0.0], &[0.0, 0.0, 0.0, 1.0]),
                        s: Keys::default(),
                    },
                ],
            }],
        };
        let sets = [set];
        let b = Bound::new(
            &target,
            &sets[0],
            0,
            0,
            &["root".into(), "pelvis".into()],
            "pelvis",
        );
        let mut pose = target.rest();
        b.sample(&sets, 1.0, &mut pose);
        assert_eq!(
            pose[0].t,
            [0.7, 0.0, 0.0],
            "the arm keeps the target's length"
        );
        assert!(
            (pose[2].t[1] - 0.4).abs() < 1e-6,
            "the pelvis drops by the target's share: {:?}",
            pose[2].t
        );
        // Parents before children, whatever the file's order.
        assert_eq!(target.order, vec![1, 2, 0]);
        let g = target.globals(&pose);
        assert!(
            (g[0].t[1] - 0.4).abs() < 1e-6 && (g[0].t[0] - 0.7).abs() < 1e-6,
            "{:?}",
            g[0].t
        );
        assert_eq!(target.global(&pose, 0), g[0]);
    }
}
