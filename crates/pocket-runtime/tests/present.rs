//! The render feed's extraction (pocket-runtime's `Extractor`; host-protocol.md 5): a viewport
//! that applies every frame it is pushed holds, after each tick and each edit of the sailing game,
//! the scene a new subscriber's full frame describes.
mod common;

use std::collections::BTreeMap;

use pocket_assets::Feed;
use pocket_assets::frame::{InstanceUpdate, RenderFrame};
use pocket_link::Source;
use pocket_runtime::{Command, Extractor, Game};
use serde_json::{Value, json};

/// A viewport's scene: what the frames applied in order leave.
#[derive(Default, Debug, PartialEq)]
struct Scene {
    instances: BTreeMap<u64, InstanceUpdate>,
    rest: RenderFrame,
}

impl Scene {
    fn apply(&mut self, f: RenderFrame) {
        if f.reset {
            self.instances.clear();
            self.rest = RenderFrame::default();
        }
        for id in &f.removed {
            self.instances.remove(id);
        }
        for u in &f.instances {
            let cur = self
                .instances
                .entry(u.id)
                .or_insert_with(|| InstanceUpdate {
                    id: u.id,
                    pose: None,
                    look: None,
                    anim: None,
                });
            if u.pose.is_some() {
                cur.pose = u.pose;
            }
            if u.look.is_some() {
                cur.look = u.look.clone();
            }
            if u.anim.is_some() {
                cur.anim = u.anim.clone();
            }
        }
        let r = &mut self.rest;
        r.tick = f.tick;
        r.t_s = f.t_s;
        r.dt_s = f.dt_s;
        macro_rules! take {
            ($($field:ident),*) => {$(if f.$field.is_some() { r.$field = f.$field.clone(); })*};
        }
        take!(
            lights,
            cameras,
            environment,
            sea,
            splats,
            ui,
            audio,
            emitters
        );
    }

    fn of(full: RenderFrame) -> Scene {
        assert!(full.reset, "a new subscriber's first frame is a reset");
        let mut s = Scene::default();
        s.apply(full);
        s
    }
}

fn edit(g: &mut Game, seq: u64, ops: Value) {
    g.apply(&Command::new(
        Source::Editor,
        seq,
        "world.edit",
        json!({"ops": ops, "label": "test"}),
    ))
    .unwrap_or_else(|e| panic!("{e:#?}"));
}

fn ops(t: u64) -> Option<Value> {
    let x = f64::from(u32::try_from(t % 40).unwrap_or(0)) / 8.0;
    match t % 8 {
        1 => Some(
            json!([{"spawn": {"name": format!("Buoy{t}"), "components": {
            "Transform": {"position": [x, 0.5, -x]},
            "Model": {"mesh": "builtin:sphere"}}}}]),
        ),
        2 => Some(
            json!([{"set": {"entity": "Sun", "component": "Light", "value": {"intensity": x}}}]),
        ),
        3 => Some(
            json!([{"set": {"entity": "Crate3", "component": "Model", "value": {"color": [x / 5.0, 0.2, 0.3, 1.0]}}}]),
        ),
        4 => Some(
            json!([{"set": {"entity": "Crate4", "component": "Light", "value": {"intensity": x}}}]),
        ),
        5 => Some(json!([{"remove": {"entity": "Crate4", "component": "Light"}}])),
        6 => Some(json!([{"destroy": {"entity": format!("Buoy{}", t - 5)}}])),
        7 => Some(
            json!([{"set": {"entity": "Sky", "component": "Environment", "value": {"ambient": x / 10.0}}}]),
        ),
        _ => None,
    }
}

#[test]
fn applied_deltas_hold_the_full_frames_scene() {
    common::big_stack(|| {
        let mut g = Game::new(common::sailing(), 4).unwrap_or_else(|e| panic!("{e:#?}"));
        let feed = Feed::new();
        let viewport = feed.subscribe();
        let mut x = Extractor::new();
        let mut scene = Scene::default();
        let mut check = |g: &mut Game, scene: &mut Scene, at: &str| {
            // A new subscriber gets the full frame at the same extraction as the delta.
            let fresh = feed.subscribe();
            g.present(&mut x, &feed);
            scene.apply(viewport.take());
            let full = Scene::of(fresh.take());
            fresh.close();
            assert_eq!(*scene, full, "the viewport's scene differs {at}");
        };
        check(&mut g, &mut scene, "at the start");
        let mut seq = 0;
        let mut most = 0;
        for _ in 0..120 {
            let t = g.tick().0 + 1;
            if let Some(o) = ops(t) {
                seq += 1;
                edit(&mut g, seq, o);
                check(
                    &mut g,
                    &mut scene,
                    &format!("after the edit before tick {t}"),
                );
            }
            g.step().unwrap_or_else(|e| panic!("{e:#?}"));
            check(&mut g, &mut scene, &format!("after tick {t}"));
            most = most.max(scene.instances.len());
        }
        // The buoys came and went.
        assert!(most > 5, "at most {most} instances");
    });
}
