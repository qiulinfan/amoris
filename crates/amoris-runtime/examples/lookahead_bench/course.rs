//! The sailing course's seeded variants for the lookahead bench and `tests/lookahead.rs`
//! (docs/spec/player.md 8.1; docs/bench/player.md 5). They are the engine's own (first written on
//! Pioneer), not the shared benchmark's seeds: the wind follows tasks.md 6.2's formula, drawn with
//! SplitMix64 rather than Python's `random.Random`; the crate placement is this bench's; and a
//! developer's `world.edit` applies the variant at tick 0 rather than the benchmark's
//! `new --params` and setup step.
//! The course's start, marks and Isle are read from the loaded game, so the variants follow the
//! scene.
#![allow(dead_code)] // each including crate uses part of it

use amoris_link::Source;
use amoris_runtime::{Command, Game};
use amoris_sim::math;
use serde_json::{Value, json};

/// A seed's uniform draws in [0, 1) (SplitMix64).
struct Draws(u64);

impl Draws {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        amoris_sim::Tick(z >> 11).to_f64() / amoris_sim::Tick(1 << 53).to_f64()
    }
}

/// The course's crates, in the order a variant places them.
pub const CRATES: [&str; 4] = ["Crate1", "Crate2", "Crate3", "Crate4"];
/// The marks, in the order they are rounded.
const MARKS: [&str; 3] = ["Mark1", "Mark2", "Mark3"];
/// How many times a crate's place is drawn before the variant gives up.
const DRAWS_PER_CRATE: usize = 17;

fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    math::hypot(a.0 - b.0, a.1 - b.1)
}

fn dev(g: &mut Game, name: &str, params: Value) -> Value {
    g.apply(&Command::new(Source::Developer(0), 1, name, params))
        .unwrap_or_else(|e| panic!("{name}: {e:#?}"))
}

/// An entity's place (x, z) as a developer reads it.
fn place(g: &mut Game, entity: &str) -> (f64, f64) {
    let got = dev(
        g,
        "world.get",
        json!({"entity": entity, "components": ["Transform"]}),
    );
    let p = &got["components"]["Transform"]["position"];
    match (p[0].as_f64(), p[2].as_f64()) {
        (Some(x), Some(z)) => (x, z),
        _ => panic!("{entity} has no position: {got:#}"),
    }
}

/// The course as a variant places things on it, read from the game before its first tick: the
/// Sloop's start and the marks in order (the course's path), and the Isle's centre, x and z in
/// metres.
pub struct Course {
    path: Vec<(f64, f64)>,
    isle: (f64, f64),
}

impl Course {
    pub fn read(g: &mut Game) -> Course {
        let mut path = vec![place(g, "Sloop")];
        path.extend(MARKS.iter().map(|m| place(g, m)));
        Course {
            path,
            isle: place(g, "Isle"),
        }
    }

    /// The point at `u` (0 to 1) of the course's length from the start through the marks, moved
    /// `off` metres to its left.
    fn along(&self, u: f64, off: f64) -> (f64, f64) {
        let legs: Vec<f64> = self.path.windows(2).map(|w| dist(w[0], w[1])).collect();
        let mut s = u * legs.iter().sum::<f64>();
        for (i, len) in legs.iter().enumerate() {
            if s <= *len || i == legs.len() - 1 {
                let (a, b) = (self.path[i], self.path[i + 1]);
                let t = math::min(s / len, 1.0);
                let (dx, dz) = ((b.0 - a.0) / len, (b.1 - a.1) / len);
                // Left of the direction of travel in the contract frame (x east, z south): (dz, -dx).
                return (a.0 + dx * len * t + dz * off, a.1 + dz * len * t - dx * off);
            }
            s -= len;
        }
        self.path[0]
    }

    /// The `world.edit` parameters of the variant for `seed`, sent by a developer before the first
    /// tick: the wind from `270 + 40 (r - 0.5)` at `5 + 2 r` m/s (tasks.md 6.2's formula), and
    /// each crate, in order, at a point a fraction `r` of the course's length from the start
    /// through the marks, `80 (r - 0.5)` metres to its side, drawn again (with the next two draws)
    /// when within 25 m of the Isle's centre or 30 m of the start; a crate with no place after
    /// [`DRAWS_PER_CRATE`] draws fails the variant. Seed 0 is the course as shipped (no edits).
    /// `omniscient` sets the Sloop's `Observer.omniscient`, which binds the seat to the reserved
    /// `omniscient_player` profile (perception.md, The omniscient view).
    pub fn variant(&self, seed: u64, omniscient: bool) -> Option<Value> {
        let mut ops = Vec::new();
        if seed != 0 {
            let mut r = Draws(seed);
            let from = 270.0 + 40.0 * (r.next() - 0.5);
            let speed = 5.0 + 2.0 * r.next();
            ops.push(json!({"set": {"entity": "Breeze", "component": "Wind",
                "value": {"from_deg": from, "speed": speed}}}));
            for name in CRATES {
                let at = (0..DRAWS_PER_CRATE)
                    .map(|_| self.along(r.next(), 80.0 * (r.next() - 0.5)))
                    .find(|at| dist(*at, self.isle) >= 25.0 && dist(*at, self.path[0]) >= 30.0)
                    .unwrap_or_else(|| panic!("seed {seed}: no place for {name}"));
                ops.push(json!({"set": {"entity": name, "component": "Transform",
                    "value": {"position": [at.0, 0.0, at.1]}}}));
            }
        }
        if omniscient {
            ops.push(json!({"set": {"entity": "Sloop", "component": "Observer",
                "value": {"omniscient": true}}}));
        }
        (!ops.is_empty()).then(|| json!({"ops": ops, "label": format!("course variant {seed}")}))
    }
}

/// A game of `setup` at tick 0 with the variant for `seed` applied.
pub fn game(setup: std::sync::Arc<amoris_runtime::GameSetup>, seed: u64, omniscient: bool) -> Game {
    let mut g = Game::new(setup, 1).unwrap_or_else(|e| panic!("{e:#?}"));
    if let Some(edit) = Course::read(&mut g).variant(seed, omniscient) {
        dev(&mut g, "world.edit", edit);
    }
    g
}
