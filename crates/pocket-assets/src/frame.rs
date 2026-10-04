//! The render feed (charter 4.4; host-protocol.md 5): what changed in the visual state of the
//! world after a tick, for the renderers. The game thread extracts a [`RenderFrame`] from the
//! components that changed (change detection, presentation only, never read by the simulation)
//! and pushes it to every subscriber's [`Mailbox`]; a renderer takes the merged pending frame when
//! it draws. A static world costs nothing per tick, and a renderer that falls behind receives one
//! merged frame, never a backlog.
//!
//! Values are `f32`: the feed is presentation, and positions are relative to the world origin.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// An entity's pose for drawing: position, unit quaternion `[x, y, z, w]`, scale.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pose {
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

impl Default for Pose {
    fn default() -> Self {
        Pose {
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }
}

/// How an instance is drawn: the `Model` component in `f32`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Look {
    pub mesh: String,
    pub material: String,
    pub color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: [f32; 3],
    pub cast_shadows: bool,
    pub visible: bool,
}

/// A skeletal animation's state for drawing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnimView {
    pub clip: String,
    /// Seconds into the clip at this tick.
    pub time: f32,
    /// Seconds of clip per second of simulation (for interpolation between ticks).
    pub rate: f32,
    pub looped: bool,
}

/// One drawn entity's change: its pose, its look, its animation, or any of these.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstanceUpdate {
    pub id: u64,
    pub pose: Option<Pose>,
    pub look: Option<Look>,
    #[serde(default)]
    pub anim: Option<AnimView>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LightKindView {
    Directional,
    Point,
    Spot,
}

/// A light, posed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LightView {
    pub id: u64,
    pub kind: LightKindView,
    pub position: [f32; 3],
    /// The direction it shines (the entity's -z).
    pub direction: [f32; 3],
    pub color: [f32; 3],
    pub intensity: f32,
    pub range: f32,
    pub inner_deg: f32,
    pub outer_deg: f32,
    pub shadows: bool,
}

/// A camera, posed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CameraView {
    pub id: u64,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub fov_deg: f32,
    pub near: f32,
    pub far: f32,
    pub active: bool,
    pub exposure_ev: f32,
}

/// The environment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentView {
    /// 0 atmosphere, 1 color.
    pub sky: u32,
    pub sky_color: [f32; 3],
    pub ambient: f32,
    pub fog_density: f32,
    pub fog_color: [f32; 3],
    pub exposure_ev: f32,
    pub bloom: f32,
}

/// One wave of the sea, as pocket-physics computes it (a sine on the surface, deep-water speed).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WaveView {
    pub length: f32,
    pub height: f32,
    pub toward_deg: f32,
    pub phase: f32,
}

/// The sea the boats float on, drawn to the horizon from the same waves buoyancy uses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeaView {
    pub level: f32,
    pub waves: Vec<WaveView>,
}

/// A splat cloud, posed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SplatView {
    pub id: u64,
    pub asset: String,
    pub pose: Pose,
    pub visible: bool,
}

/// One UI element to draw this frame (text or bar), anchored on the screen or at a world point.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiView {
    pub id: u64,
    /// The text; empty for a bar.
    pub text: String,
    /// Bar fill 0..1 (bars only).
    pub fill: f32,
    pub bar: bool,
    pub size: [f32; 2],
    pub color: [f32; 4],
    pub back: [f32; 4],
    /// 0 top-left .. 8 bottom-right (row-major 3x3), 9: at `world`.
    pub anchor: u32,
    pub offset: [f32; 2],
    pub world: [f32; 3],
}

/// A particle emitter, posed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EmitterView {
    pub id: u64,
    pub position: [f32; 3],
    /// The entity's +y (the emission axis).
    pub up: [f32; 3],
    pub rate: f32,
    pub burst: u32,
    pub burst_id: u32,
    pub emitting: bool,
    pub lifetime: f32,
    pub speed: f32,
    pub spread_deg: f32,
    pub acceleration: [f32; 3],
    pub drag: f32,
    pub size: [f32; 2],
    pub color_start: [f32; 4],
    pub color_end: [f32; 4],
    pub radius: f32,
}

/// A sound source, posed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioView {
    pub id: u64,
    pub clip: String,
    pub volume: f32,
    pub pitch: f32,
    pub looped: bool,
    pub playing: bool,
    pub spatial: bool,
    pub position: [f32; 3],
}

/// What changed in the visual state since the subscriber's last frame.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RenderFrame {
    /// The tick this state is the end of.
    pub tick: u64,
    /// Simulated seconds at that tick.
    pub t_s: f64,
    /// The simulation's tick length, seconds (for interpolation).
    pub dt_s: f64,
    /// Everything below replaces the renderer's scene instead of updating it.
    pub reset: bool,
    pub instances: Vec<InstanceUpdate>,
    pub removed: Vec<u64>,
    /// The full list, when any light changed.
    pub lights: Option<Vec<LightView>>,
    /// The full list, when any camera changed.
    pub cameras: Option<Vec<CameraView>>,
    pub environment: Option<EnvironmentView>,
    /// `Some(None)`: the sea was removed.
    pub sea: Option<Option<SeaView>>,
    /// The full list, when any splat changed.
    pub splats: Option<Vec<SplatView>>,
    /// The full list of UI elements, when any changed (or an entity carrying one moved).
    #[serde(default)]
    pub ui: Option<Vec<UiView>>,
    /// The full list of sound sources, when any changed or moved.
    #[serde(default)]
    pub audio: Option<Vec<AudioView>>,
    /// The full list of particle emitters, when any changed or moved.
    #[serde(default)]
    pub emitters: Option<Vec<EmitterView>>,
}

impl RenderFrame {
    /// Whether it carries nothing but the clock.
    pub fn is_empty(&self) -> bool {
        !self.reset
            && self.instances.is_empty()
            && self.removed.is_empty()
            && self.lights.is_none()
            && self.cameras.is_none()
            && self.environment.is_none()
            && self.sea.is_none()
            && self.splats.is_none()
            && self.ui.is_none()
            && self.audio.is_none()
            && self.emitters.is_none()
    }

    /// Folds `next` (a later frame) into this one, as if both had been applied in order.
    pub fn merge(&mut self, next: RenderFrame) {
        self.tick = next.tick;
        self.t_s = next.t_s;
        self.dt_s = next.dt_s;
        if next.reset {
            *self = next;
            return;
        }
        if !next.removed.is_empty() {
            let gone: std::collections::HashSet<u64> = next.removed.iter().copied().collect();
            self.instances.retain(|u| !gone.contains(&u.id));
            self.removed.extend(next.removed);
        }
        if !next.instances.is_empty() {
            if self.instances.is_empty() {
                self.instances = next.instances;
            } else {
                let mut at: HashMap<u64, usize> = self
                    .instances
                    .iter()
                    .enumerate()
                    .map(|(i, u)| (u.id, i))
                    .collect();
                for u in next.instances {
                    // An entity removed and then spawned again under the same id cannot happen:
                    // ids are never reused (simulation.md 7).
                    match at.get(&u.id) {
                        Some(&i) => {
                            let cur = &mut self.instances[i];
                            if u.pose.is_some() {
                                cur.pose = u.pose;
                            }
                            if u.look.is_some() {
                                cur.look = u.look;
                            }
                            if u.anim.is_some() {
                                cur.anim = u.anim;
                            }
                        }
                        None => {
                            at.insert(u.id, self.instances.len());
                            self.instances.push(u);
                        }
                    }
                }
            }
        }
        if next.lights.is_some() {
            self.lights = next.lights;
        }
        if next.cameras.is_some() {
            self.cameras = next.cameras;
        }
        if next.environment.is_some() {
            self.environment = next.environment;
        }
        if next.sea.is_some() {
            self.sea = next.sea;
        }
        if next.splats.is_some() {
            self.splats = next.splats;
        }
        if next.ui.is_some() {
            self.ui = next.ui;
        }
        if next.audio.is_some() {
            self.audio = next.audio;
        }
        if next.emitters.is_some() {
            self.emitters = next.emitters;
        }
    }

    /// The bytes the editor viewport receives over `/render` (host-protocol.md 5).
    pub fn encode(&self) -> Vec<u8> {
        bincode2::serde::encode_to_vec(self, bincode2::config::standard()).unwrap_or_default()
    }

    pub fn decode(bytes: &[u8]) -> Option<RenderFrame> {
        bincode2::serde::decode_from_slice(bytes, bincode2::config::standard())
            .ok()
            .map(|(f, _)| f)
    }
}

/// One subscriber's pending frame.
#[derive(Default)]
pub struct Mailbox {
    pending: Mutex<RenderFrame>,
    /// The subscriber has nothing yet (or lost its scene): the next extraction sends it a reset.
    wants_reset: AtomicBool,
    closed: AtomicBool,
}

impl Mailbox {
    /// The frame merged from everything pushed since the last take.
    pub fn take(&self) -> RenderFrame {
        std::mem::take(&mut *self.pending.lock().unwrap_or_else(|p| p.into_inner()))
    }

    /// Asks for a full frame at the next extraction.
    pub fn request_reset(&self) {
        self.wants_reset.store(true, Ordering::Release);
    }

    /// Stops deliveries; the feed forgets the mailbox.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    fn push(&self, frame: RenderFrame) {
        let mut p = self.pending.lock().unwrap_or_else(|p| p.into_inner());
        if p.is_empty() && !p.reset {
            *p = frame;
        } else {
            p.merge(frame);
        }
    }
}

/// The game's render feed: its subscribers.
#[derive(Clone, Default)]
pub struct Feed {
    subs: Arc<Mutex<Vec<Arc<Mailbox>>>>,
}

impl Feed {
    pub fn new() -> Feed {
        Feed::default()
    }

    /// A new subscriber; its first frame is a reset.
    pub fn subscribe(&self) -> Arc<Mailbox> {
        let m = Arc::new(Mailbox::default());
        m.request_reset();
        self.subs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(m.clone());
        m
    }

    /// Whether anyone listens.
    pub fn has_subscribers(&self) -> bool {
        let mut subs = self.subs.lock().unwrap_or_else(|p| p.into_inner());
        subs.retain(|m| !m.closed.load(Ordering::Acquire));
        !subs.is_empty()
    }

    /// Whether a subscriber waits for a full frame.
    pub fn wants_reset(&self) -> bool {
        self.subs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .any(|m| m.wants_reset.load(Ordering::Acquire))
    }

    /// Delivers a tick's frames: `delta` to subscribers that have a scene, `full` (when made) to
    /// those that asked for a reset. Every subscriber needs a reset after `reset_all`.
    pub fn publish(&self, delta: RenderFrame, full: Option<RenderFrame>) {
        let subs = self.subs.lock().unwrap_or_else(|p| p.into_inner());
        for m in subs.iter() {
            if m.closed.load(Ordering::Acquire) {
                continue;
            }
            if m.wants_reset.load(Ordering::Acquire) {
                if let Some(f) = &full {
                    m.wants_reset.store(false, Ordering::Release);
                    m.push(f.clone());
                }
            } else if !delta.is_empty() || delta.reset {
                m.push(delta.clone());
            } else {
                // Keep the clock moving for interpolation even when nothing changed.
                let mut p = m.pending.lock().unwrap_or_else(|p| p.into_inner());
                p.tick = delta.tick;
                p.t_s = delta.t_s;
                p.dt_s = delta.dt_s;
            }
        }
    }

    /// Every subscriber's scene is stale (a restore, Play or Stop replaced the world).
    pub fn reset_all(&self) {
        for m in self.subs.lock().unwrap_or_else(|p| p.into_inner()).iter() {
            m.request_reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn up(id: u64, x: f32) -> InstanceUpdate {
        InstanceUpdate {
            id,
            pose: Some(Pose {
                position: [x, 0.0, 0.0],
                ..Pose::default()
            }),
            look: None,
            anim: None,
        }
    }

    #[test]
    fn merge_keeps_the_latest_pose_and_drops_removed() {
        let mut a = RenderFrame {
            instances: vec![up(1, 1.0), up(2, 2.0)],
            ..RenderFrame::default()
        };
        a.merge(RenderFrame {
            tick: 2,
            instances: vec![up(1, 5.0), up(3, 3.0)],
            removed: vec![2],
            ..RenderFrame::default()
        });
        assert_eq!(a.tick, 2);
        assert_eq!(a.instances.len(), 2);
        assert_eq!(a.instances[0].pose.unwrap().position[0], 5.0);
        assert_eq!(a.removed, vec![2]);
    }

    #[test]
    fn a_new_subscriber_gets_the_full_frame_then_deltas() {
        let feed = Feed::new();
        let m = feed.subscribe();
        assert!(feed.wants_reset());
        let full = RenderFrame {
            reset: true,
            instances: vec![up(1, 1.0)],
            ..RenderFrame::default()
        };
        feed.publish(RenderFrame::default(), Some(full));
        let f = m.take();
        assert!(f.reset && f.instances.len() == 1);
        feed.publish(
            RenderFrame {
                instances: vec![up(1, 2.0)],
                ..RenderFrame::default()
            },
            None,
        );
        let f = m.take();
        assert!(!f.reset && f.instances[0].pose.unwrap().position[0] == 2.0);
        let bytes = f.encode();
        assert_eq!(RenderFrame::decode(&bytes), Some(f));
    }
}
