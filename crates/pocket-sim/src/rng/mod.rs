//! Random numbers (docs/spec/rng.md): PCG32 streams derived, during each tick, from the world seed,
//! the tick and a key naming who draws. Between ticks the only random state is the world seed, so
//! fork, restore and replay reproduce every draw by construction and a script reload leaves no
//! generator behind.

mod draw;
mod streams;
mod table;

pub use draw::Stream;
pub use table::{RngMark, RngTable, StreamKey, StreamKind};

use bevy_ecs::prelude::Resource;
use pocket_contract::{Problem, detail};
use schemars::JsonSchema;
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::entity::EntityId;
use crate::time::Tick;

/// PCG-XSH-RR with 64-bit state and 32-bit output (O'Neill 2014), exactly as the reference
/// `pcg-c-basic`'s `pcg32_srandom_r` and `pcg32_random_r` (rng.md 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pcg32 {
    state: u64,
    inc: u64,
}

const PCG_MULT: u64 = 6_364_136_223_846_793_005;

impl Pcg32 {
    /// `pcg32_srandom_r(initstate, initseq)`.
    pub fn new(initstate: u64, initseq: u64) -> Pcg32 {
        let mut r = Pcg32 {
            state: 0,
            inc: (initseq << 1) | 1,
        };
        r.next_u32();
        r.state = r.state.wrapping_add(initstate);
        r.next_u32();
        r
    }

    /// `pcg32_random_r`.
    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(PCG_MULT).wrapping_add(self.inc);
        #[allow(clippy::cast_possible_truncation)] // the XSH-RR output takes the low 32 bits
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        #[allow(clippy::cast_possible_truncation)] // old >> 59 is below 32
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }
}

const GOLDEN: u64 = 0x9e37_79b9_7f4a_7c15;

/// SplitMix64's output function (Stafford's "Mix13" constants, as Vigna's `splitmix64.c`).
pub fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The derived seed of a key's words (rng.md 5.1).
pub fn derive(seed: u64, words: &[u64]) -> u64 {
    let mut d = Deriver::new(seed);
    for &w in words {
        d.word(w);
    }
    d.finish()
}

/// [`derive`] fed word by word, so a key needs no buffer.
pub struct Deriver(u64);

impl Deriver {
    pub fn new(seed: u64) -> Deriver {
        Deriver(mix64(seed.wrapping_add(GOLDEN)))
    }

    pub fn word(&mut self, w: u64) {
        self.0 = mix64(self.0.wrapping_add(GOLDEN) ^ w);
    }

    /// A part's tag and value words.
    pub fn part(&mut self, p: &KeyPart<'_>) {
        let [tag, value] = p.words();
        self.word(tag);
        self.word(value);
    }

    pub fn finish(&self) -> u64 {
        self.0
    }
}

/// FNV-1a, 64 bits, over bytes.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    fnv1a64_extend(0xcbf2_9ce4_8422_2325, bytes)
}

/// FNV-1a continued from `h` over more bytes.
pub fn fnv1a64_extend(mut h: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// A stream's generator from its derived seed: both the state and the sequence come from it.
pub fn stream_from_seed(derived: u64) -> Pcg32 {
    Pcg32::new(derived, mix64(derived ^ GOLDEN))
}

/// The largest integer a key part, a seed or an `int` bound may be: 2^53 - 1, exact in a double.
pub const MAX_SAFE: u64 = (1 << 53) - 1;

/// One part of a stream key (rng.md 5.2), encoded as a tag word and a value word so parts of
/// different kinds never alias (entity 17 is not the integer 17).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyPart<'a> {
    Tick(Tick),
    System(&'a str),
    Entity(EntityId),
    /// An integer in ±(2^53 - 1).
    Int(i64),
    Str(&'a str),
}

impl KeyPart<'_> {
    /// The part's two words.
    pub fn words(&self) -> [u64; 2] {
        match *self {
            KeyPart::Tick(t) => [1, t.0],
            KeyPart::System(k) => [2, fnv1a64(k.as_bytes())],
            KeyPart::Entity(e) => [3, e.get()],
            #[allow(clippy::cast_sign_loss)] // two's complement in a u64, as rng.md 5.2 says
            KeyPart::Int(i) => [4, i as u64],
            KeyPart::Str(s) => [5, fnv1a64(s.as_bytes())],
        }
    }
}

/// Checks a part: an integer must lie within ±(2^53 - 1), else `rng.key_invalid`.
pub fn check_part(p: &KeyPart<'_>, index: usize) -> Result<(), Problem> {
    match p {
        KeyPart::Int(i) if i.unsigned_abs() > MAX_SAFE => Err(key_invalid(&i.to_string(), index)),
        _ => Ok(()),
    }
}

/// The words of a key, with the parts checked.
pub fn key_words(parts: &[KeyPart<'_>]) -> Result<Vec<u64>, Problem> {
    let mut words = Vec::with_capacity(parts.len() * 2);
    for (index, p) in parts.iter().enumerate() {
        check_part(p, index)?;
        words.extend_from_slice(&p.words());
    }
    Ok(words)
}

/// The world seed: the RNG's whole state between ticks, hashed and snapshotted with the world
/// (rng.md 5.4). At most 2^53 - 1, so it crosses JSON and TypeScript exactly; decoding refuses
/// more with `rng.seed_invalid`'s message.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct WorldSeed(pub u64);

impl<'de> Deserialize<'de> for WorldSeed {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<WorldSeed, D::Error> {
        /// The encoded shape, `WorldSeed`'s own.
        #[derive(Deserialize)]
        #[serde(rename = "WorldSeed")]
        struct Repr(u64);
        let Repr(seed) = Repr::deserialize(d)?;
        WorldSeed::new(seed).map_err(de::Error::custom)
    }
}

impl WorldSeed {
    /// A seed, or `rng.seed_invalid` outside 0..=2^53 - 1.
    pub fn new(seed: u64) -> Result<WorldSeed, Problem> {
        if seed > MAX_SAFE {
            return Err(Problem::new(
                "rng.seed_invalid",
                format!("A world seed must be at most 2^53 - 1 (9007199254740991); got {seed}."),
                detail([("seed", json!(seed))]),
            ));
        }
        Ok(WorldSeed(seed))
    }
}

/// `rng.key_invalid {part, index}`.
pub fn key_invalid(part: &str, index: usize) -> Problem {
    Problem::new(
        "rng.key_invalid",
        format!(
            "Key part {index} ({part}) is not a valid key part: integers must lie within \
             ±(2^53 - 1), entities must be valid ids."
        ),
        detail([("part", json!(part)), ("index", json!(index))]),
    )
}

/// `rng.outside_tick {tick}`: streams exist only during a tick (rng.md 5.3).
pub fn outside_tick(tick: Tick) -> Problem {
    Problem::new(
        "rng.outside_tick",
        format!(
            "Random numbers are drawn only during a tick; the world is at the boundary after tick \
             {}. Set state that a system rolls in the next tick.",
            tick.0
        ),
        detail([("tick", json!(tick.0))]),
    )
}

/// `rng.stream_expired {key}`: a handle used after the invocation that obtained it returned.
pub fn stream_expired(key: &str) -> Problem {
    Problem::new(
        "rng.stream_expired",
        format!("The random stream '{key}' belonged to an earlier call; ask for it again."),
        detail([("key", json!(key))]),
    )
}

/// The rng.md reference vectors (4 and 5): the web test `rng.vectors`.
pub fn check_vectors() -> Result<(), String> {
    vectors::check()
}

mod vectors;
