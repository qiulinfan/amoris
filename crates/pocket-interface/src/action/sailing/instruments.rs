//! The skipper's instruments as shared/contract/sailing.md defines them (The skipper's
//! perception, Instruments), computed from the boat's own components and the wind: what a seat
//! knows of its own boat without seeing anything. Perception evaluates instruments through its
//! declarations; these functions are the sailing game's definitions of them, and [`BoatView`] is
//! an actor view built on them for the action layer's own tests and fixtures, knowing the entities
//! its caller lists and nothing else.

use bevy_ecs::prelude::World;
use pocket_physics::geom;
use pocket_physics::{Boat, Transform, Trim, Velocity, Wind};
use pocket_sim::{EntityId, EntityIndex, SimClock, Tick};
use serde_json::{Value, json};

use crate::action::affordance::relative;
use crate::action::view::{ActorView, Known, KnownEvent, Visibility};

/// The angle of `deg` in [0, 360).
pub fn bearing(deg: f64) -> f64 {
    let mut b = deg % 360.0;
    if b < 0.0 {
        b += 360.0;
    }
    if b >= 360.0 {
        b -= 360.0;
    }
    b
}

/// The world's wind: the first entity with a `Wind`, in `EntityId` order.
pub fn wind(world: &World) -> Option<Wind> {
    let index = world.get_resource::<EntityIndex>()?;
    index
        .iter()
        .find_map(|(_, e)| world.get::<Wind>(e).copied())
}

/// `point_of_sail` by `abs(twa_deg)`.
pub fn point_of_sail(twa_deg: f64) -> &'static str {
    let a = twa_deg.abs();
    if a < 45.0 {
        "in_irons"
    } else if a < 60.0 {
        "close_hauled"
    } else if a < 80.0 {
        "close_reach"
    } else if a < 100.0 {
        "beam_reach"
    } else if a < 150.0 {
        "broad_reach"
    } else {
        "running"
    }
}

fn trim_name(t: Trim) -> &'static str {
    match t {
        Trim::Furled => "furled",
        Trim::Luffing => "luffing",
        Trim::Good => "good",
        Trim::Overtrimmed => "overtrimmed",
    }
}

/// The instrument `name` of the boat on `body`, as a JSON value (sailing.md's table).
pub fn instrument(world: &World, body: EntityId, name: &str) -> Option<Value> {
    let e = pocket_sim::entity::entity(world, body)?;
    let boat = world.get::<Boat>(e)?;
    let tf = world.get::<Transform>(e)?;
    let heading = bearing(boat.heading_deg);
    let w = wind(world);
    let twa = w.map(|w| relative(bearing(w.from_deg), heading));
    Some(match name {
        "heading_deg" => json!(heading),
        "course_deg" => {
            let v = world.get::<Velocity>(e).map_or([0.0; 3], |v| v.linear);
            json!(geom::bearing_deg(v[0], v[2]))
        }
        "speed_mps" => json!(boat.speed),
        "wind_from_deg" => json!(bearing(w?.from_deg)),
        "wind_mps" => {
            let t = world.resource::<SimClock>().time();
            json!(geom::length(w?.at(tf.position, t)))
        }
        "twa_deg" => json!(twa?),
        "awa_deg" => json!(boat.awa_deg),
        "aws_mps" => json!(boat.aws),
        "point_of_sail" => json!(point_of_sail(twa?)),
        "tack" => json!(if twa? > 0.0 { "starboard" } else { "port" }),
        "hoist" => json!(boat.hoist_now),
        "sheet" => json!(boat.sheet_now),
        "trim" => json!(trim_name(boat.trim)),
        "drive" => json!(boat.drive),
        "rudder" => json!(boat.rudder_now),
        "heel_deg" => json!(boat.heel_deg),
        "pos_m" => json!({"x": tf.position[0], "y": tf.position[1], "z": tf.position[2]}),
        "afloat" => json!(boat.afloat),
        _ => return None,
    })
}

/// The horizontal range and bearing from `from` to `to` (perception.md, Geometry).
pub fn range_bearing(from: [f64; 3], to: [f64; 3]) -> (f64, f64) {
    let (dx, dz) = (to[0] - from[0], to[2] - from[2]);
    (
        pocket_sim::math::sqrt(dx * dx + dz * dz),
        geom::bearing_deg(dx, dz),
    )
}

/// An entity a [`BoatView`] knows: as seen (live position), or as charted at a fixed position.
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    pub id: EntityId,
    pub kind: String,
    /// `None`: seen, at its live position; `Some`: charted there.
    pub chart_m: Option<[f64; 3]>,
    pub facts: Vec<(String, Value)>,
}

/// An actor view over a boat: the sailing instruments of its body, and the entities listed.
pub struct BoatView<'w> {
    pub world: &'w World,
    pub body: EntityId,
    pub listed: Vec<Listed>,
}

impl BoatView<'_> {
    fn pos(&self) -> [f64; 3] {
        pocket_sim::entity::entity(self.world, self.body)
            .and_then(|e| self.world.get::<Transform>(e))
            .map_or([0.0; 3], |t| t.position)
    }

    fn known_of(&self, l: &Listed) -> Option<Known> {
        let e = pocket_sim::entity::entity(self.world, l.id);
        let (at, visibility) = match l.chart_m {
            Some(c) => (c, Visibility::Charted),
            None => (self.world.get::<Transform>(e?)?.position, Visibility::Seen),
        };
        let (range_m, bearing_deg) = range_bearing(self.pos(), at);
        Some(Known {
            id: l.id,
            name: e
                .and_then(|e| self.world.get::<pocket_sim::Name>(e))
                .map(|n| n.as_str().to_owned()),
            kind: l.kind.clone(),
            visibility,
            pos_m: at,
            range_m,
            bearing_deg,
            facts: l.facts.clone(),
        })
    }
}

impl ActorView for BoatView<'_> {
    fn tick(&self) -> Tick {
        self.world.resource::<SimClock>().tick
    }

    fn body(&self) -> EntityId {
        self.body
    }

    fn instrument(&self, name: &str) -> Option<Value> {
        instrument(self.world, self.body, name)
    }

    fn known(&self, id: EntityId) -> Option<Known> {
        self.listed
            .iter()
            .find(|l| l.id == id)
            .and_then(|l| self.known_of(l))
    }

    fn all_known(&self) -> Vec<Known> {
        let mut out: Vec<Known> = self
            .listed
            .iter()
            .filter_map(|l| self.known_of(l))
            .collect();
        out.sort_by_key(|k| k.id);
        out
    }

    fn events_since(&self, _seq: u64) -> Vec<KnownEvent> {
        Vec::new()
    }
}
