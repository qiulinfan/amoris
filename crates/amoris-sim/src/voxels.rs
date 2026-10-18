//! Block volumes (docs/spec/voxels.md 2 to 4): `Voxels`, a world of cells in 16 x 16 x 16 chunks
//! with a palette of blocks, its edits and its exact grid ray cast; each cell's state byte, which a
//! liquid's level lives in, and the flow's pending cells (docs/spec/liquids.md 2 to 4). World state:
//! physics collides with it and flows its liquids, scripts read and edit it, the render extractor
//! sends its changed chunks.

use amoris_contract::{Problem, detail};
use bevy_ecs::prelude::Component;
use schemars::JsonSchema;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::json;

use crate::num::to_i32;
use crate::persisted::Persisted;

/// Cells along a chunk's edge.
pub const CHUNK: i32 = 16;
/// Cells in a chunk.
pub const CHUNK_CELLS: usize = 4096;
/// The most cells one `fill` may cover.
pub const MAX_FILL: u64 = 1 << 24;

/// A cell or a chunk key: integer coordinates in the volume's grid.
pub type Cell = [i32; 3];

/// A liquid state's level bits: 0 a source, 1 to 7 flowing (liquids.md 2).
pub const LEVEL: u8 = 0b0111;
/// A liquid state's falling bit: fed from above.
pub const FALLING: u8 = 0b1000;
/// The most liquids of a volume that flow.
pub const MAX_FLOWING: usize = 8;

/// How a block is drawn (voxels.md 2.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BlockShape {
    /// A full cell with a top, side and bottom material.
    #[default]
    Cube,
    /// Two crossed quads (plants), with `faces[0]`.
    Cross,
    /// A cell of liquid, its top lowered by an eighth, with `faces[0]`.
    Liquid,
}

/// One kind of block in a volume's palette.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Block {
    /// Unique in the palette, `^[a-z][a-z0-9_]*$`; index 0 is `air`.
    pub name: String,
    pub shape: BlockShape,
    /// Collides, and stops a ray cast that asks for solids.
    pub solid: bool,
    /// The top, side and bottom material paths of a cube; a cross or a liquid uses the first.
    pub faces: [String; 3],
    /// A liquid's flow (liquids.md 3); none: it stays still.
    pub flow: Option<Flow>,
}

/// How a liquid flows (liquids.md 3).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Flow {
    /// Cells a source reaches over flat ground, 0 to 7 (0: still).
    pub spread: u8,
    /// Ticks between flow steps.
    pub period_ticks: u32,
    /// How far, in cells, the liquid looks over the ground for a way down before it spreads.
    pub seek: u8,
    /// Two sources beside a cell over ground or a source make it a source.
    pub renews: bool,
    /// The view from inside: linear colour and fog density per metre.
    pub fog: [f64; 4],
    /// The splashes' absorption per metre, linear rgb (splashes.md 2); none: no splashes.
    /// Presentation, like `fog`.
    pub splash: Option<[f64; 3]>,
    /// The block game's cells, or the water's height field (water.md 6).
    pub model: FlowModel,
}

/// What moves a liquid (water.md 6).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FlowModel {
    /// The cellular automaton of liquids.md 4, `physics.fluids`.
    #[default]
    Cells,
    /// A `WaterRegion`'s height field: `physics.water` turns the liquid's cells into its water and
    /// clears them. Needs `spread: 0`.
    Field,
}

impl Default for Flow {
    fn default() -> Self {
        Flow {
            spread: 7,
            period_ticks: 15,
            seek: 4,
            renews: true,
            fog: [0.05, 0.16, 0.45, 0.12],
            splash: None,
            model: FlowModel::Cells,
        }
    }
}

/// A chunk's 4096 palette indices, x fastest, then z, then y. Written as bytes in the canonical
/// encoding and as base64 in JSON.
#[derive(Clone, PartialEq, Eq)]
pub struct Cells(pub Box<[u8]>);

impl std::fmt::Debug for Cells {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let filled = self.0.iter().filter(|&&c| c != 0).count();
        write!(f, "Cells({} bytes, {filled} filled)", self.0.len())
    }
}

impl Default for Cells {
    fn default() -> Self {
        Cells(vec![0; CHUNK_CELLS].into_boxed_slice())
    }
}

impl JsonSchema for Cells {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Cells".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "description": "4096 palette indices (x fastest, then z, then y), base64",
        })
    }
}

impl Serialize for Cells {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if s.is_human_readable() {
            s.serialize_str(&base64_encode(&self.0))
        } else {
            s.serialize_bytes(&self.0)
        }
    }
}

impl<'de> Deserialize<'de> for Cells {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Cells;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a chunk's cells: bytes, or base64 in text")
            }
            fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<Cells, E> {
                Ok(Cells(v.into()))
            }
            fn visit_byte_buf<E: de::Error>(self, v: Vec<u8>) -> Result<Cells, E> {
                Ok(Cells(v.into_boxed_slice()))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Cells, E> {
                base64_decode(v)
                    .map(|b| Cells(b.into_boxed_slice()))
                    .ok_or_else(|| E::custom("cells are not base64"))
            }
        }
        if d.is_human_readable() {
            d.deserialize_str(V)
        } else {
            d.deserialize_bytes(V)
        }
    }
}

/// One chunk of a volume.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Chunk {
    /// The chunk's key: the chunk holding cell `c` is `c.div_euclid(16)`.
    pub key: Cell,
    pub cells: Cells,
    /// The tick of the chunk's last edit (0: as loaded).
    #[serde(default)]
    pub edited: u64,
    /// One state byte per cell, in the cells' order (liquids.md 2); none while every byte is 0.
    #[serde(default)]
    pub states: Option<Cells>,
}

/// The flow's progress (liquids.md 4.1): the cells that may change at the next step, sorted and
/// each once, and the tick whose edits the flow has taken.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct FluidState {
    pub pending: Vec<Cell>,
    pub seen: u64,
}

/// A volume of blocks (voxels.md 2.1). Its cell `(x, y, z)` occupies `origin + cell_m * [x, x + 1)`
/// along each axis, `origin` being the entity's `Transform.position`.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Voxels {
    /// Edge of a cell, metres.
    pub cell_m: f64,
    /// Index 0 is air; at most 255 more.
    pub palette: Vec<Block>,
    /// Project-relative cells file a scene loads the cells from (voxels.md 2.3); provenance after.
    pub source: String,
    /// The chunks holding any block, in key order.
    pub chunks: Vec<Chunk>,
    /// Where its liquids' flow stands (liquids.md 4.1).
    pub fluid: FluidState,
}

impl Default for Voxels {
    fn default() -> Self {
        Voxels {
            cell_m: 1.0,
            palette: vec![Block {
                name: "air".into(),
                ..Block::default()
            }],
            source: String::new(),
            chunks: Vec::new(),
            fluid: FluidState::default(),
        }
    }
}

/// What a ray cast found (voxels.md 4).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct VoxelHit {
    pub cell: Cell,
    /// The face the ray entered by, as an axis direction; zero when it started inside the cell.
    pub normal: Cell,
    pub block: u8,
    #[serde(rename = "distanceM")]
    pub distance_m: f64,
}

impl Persisted for Voxels {
    const NAME: &'static str = "Voxels";
    const VERSION: u32 = 2;

    fn trace(
        t: &mut serde_reflection::Tracer,
        s: &serde_reflection::Samples,
    ) -> serde_reflection::Result<()> {
        t.trace_type::<BlockShape>(s)?;
        t.trace_type::<FlowModel>(s)?;
        t.trace_type::<Self>(s).map(|_| ())
    }
}

fn invalid(field: &str, message: String) -> Problem {
    Problem::new("voxels.invalid", message, detail([("field", json!(field))]))
}

/// `(chunk key, offset in the chunk)` of a cell.
pub fn locate(cell: Cell) -> (Cell, usize) {
    let key = cell.map(|c| c.div_euclid(CHUNK));
    let [x, y, z] = cell.map(|c| c.rem_euclid(CHUNK).unsigned_abs() as usize);
    (key, x + 16 * (z + 16 * y))
}

impl Voxels {
    /// Checks the palette and the chunks (voxels.md 2.2).
    pub fn validate(&self) -> Result<(), Problem> {
        if !(self.cell_m.is_finite() && self.cell_m > 0.0) {
            return Err(invalid(
                "cell_m",
                format!("cell_m must be positive, not {}", self.cell_m),
            ));
        }
        let Some(air) = self.palette.first() else {
            return Err(invalid(
                "palette",
                "the palette is empty; index 0 must be air".into(),
            ));
        };
        if air.name != "air" || air.solid {
            return Err(invalid(
                "palette.0",
                "palette index 0 must be air, not solid".into(),
            ));
        }
        if self.palette.len() > 256 {
            return Err(invalid(
                "palette",
                format!(
                    "a palette holds at most 256 blocks, not {}",
                    self.palette.len()
                ),
            ));
        }
        for (i, b) in self.palette.iter().enumerate() {
            let named = b.name.starts_with(|c: char| c.is_ascii_lowercase())
                && b.name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
            if !named {
                return Err(invalid(
                    &format!("palette.{i}.name"),
                    format!("block name {:?} must match ^[a-z][a-z0-9_]*$", b.name),
                ));
            }
            if self.palette[..i].iter().any(|o| o.name == b.name) {
                return Err(invalid(
                    &format!("palette.{i}.name"),
                    format!("block {:?} is named twice", b.name),
                ));
            }
            if let Some(f) = &b.flow {
                if b.shape != BlockShape::Liquid {
                    return Err(invalid(
                        &format!("palette.{i}.flow"),
                        format!("block {:?} flows but is not a liquid", b.name),
                    ));
                }
                if f.model == FlowModel::Field && f.spread != 0 {
                    return Err(invalid(
                        &format!("palette.{i}.flow"),
                        format!(
                            "block {:?} is a field liquid: its spread is 0 (water.md 6)",
                            b.name
                        ),
                    ));
                }
                if f.spread > LEVEL || f.period_ticks == 0 {
                    return Err(invalid(
                        &format!("palette.{i}.flow"),
                        format!(
                            "block {:?}: spread is 0 to 7, period_ticks 1 or more",
                            b.name
                        ),
                    ));
                }
            }
            let used = if b.shape == BlockShape::Cube { 3 } else { 1 };
            if i > 0 && b.faces[..used].iter().any(String::is_empty) {
                return Err(invalid(
                    &format!("palette.{i}.faces"),
                    format!("block {:?} needs {used} face material(s)", b.name),
                ));
            }
        }
        let flowing = self
            .palette
            .iter()
            .filter(|b| b.flow.is_some_and(|f| f.spread > 0));
        if flowing.count() > MAX_FLOWING {
            return Err(invalid(
                "palette",
                format!("at most {MAX_FLOWING} liquids of a volume flow"),
            ));
        }
        let blocks = self.palette.len();
        for (i, c) in self.chunks.iter().enumerate() {
            if c.states.as_ref().is_some_and(|s| s.0.len() != CHUNK_CELLS) {
                return Err(invalid(
                    &format!("chunks.{i}.states"),
                    format!("a chunk holds {CHUNK_CELLS} states"),
                ));
            }
            if c.cells.0.len() != CHUNK_CELLS {
                return Err(invalid(
                    &format!("chunks.{i}.cells"),
                    format!("a chunk holds {CHUNK_CELLS} cells, not {}", c.cells.0.len()),
                ));
            }
            if let Some(&b) = c.cells.0.iter().find(|&&b| usize::from(b) >= blocks) {
                return Err(invalid(
                    &format!("chunks.{i}.cells"),
                    format!("cell block {b} is past the palette's {blocks} blocks"),
                ));
            }
            if i > 0 && self.chunks[i - 1].key >= c.key {
                return Err(invalid(
                    &format!("chunks.{i}.key"),
                    "chunks must be in key order, each key once".into(),
                ));
            }
        }
        Ok(())
    }

    fn find(&self, key: Cell) -> Result<usize, usize> {
        self.chunks.binary_search_by(|c| c.key.cmp(&key))
    }

    /// The chunk with `key`, if it holds any block.
    pub fn chunk(&self, key: Cell) -> Option<&Chunk> {
        self.find(key).ok().map(|i| &self.chunks[i])
    }

    /// The block at `cell`: 0 for air or an absent chunk.
    pub fn get(&self, cell: Cell) -> u8 {
        let (key, at) = locate(cell);
        self.chunk(key).map_or(0, |c| c.cells.0[at])
    }

    /// The state byte of `cell` (liquids.md 2): 0 for none.
    pub fn state(&self, cell: Cell) -> u8 {
        let (key, at) = locate(cell);
        self.chunk(key)
            .and_then(|c| c.states.as_ref())
            .map_or(0, |s| s.0[at])
    }

    /// The block and state of `cell`.
    pub fn cell(&self, cell: Cell) -> (u8, u8) {
        let (key, at) = locate(cell);
        self.chunk(key).map_or((0, 0), |c| {
            (c.cells.0[at], c.states.as_ref().map_or(0, |s| s.0[at]))
        })
    }

    /// Whether `block` is a liquid on the water's height field (water.md 6).
    pub fn field_liquid(&self, block: u8) -> bool {
        self.palette
            .get(usize::from(block))
            .and_then(|b| b.flow)
            .is_some_and(|f| f.model == FlowModel::Field)
    }

    /// The flow of `block`, if it is a liquid that flows.
    pub fn flow(&self, block: u8) -> Option<Flow> {
        self.palette
            .get(usize::from(block))
            .and_then(|b| b.flow)
            .filter(|f| f.spread > 0)
    }

    /// The block's shape; air's for an index past the palette.
    pub fn shape(&self, block: u8) -> BlockShape {
        self.palette
            .get(usize::from(block))
            .map_or(BlockShape::Cube, |b| b.shape)
    }

    /// The palette index of the block named `name`.
    pub fn block(&self, name: &str) -> Option<u8> {
        self.palette
            .iter()
            .position(|b| b.name == name)
            .and_then(|i| u8::try_from(i).ok())
    }

    /// Whether the block is solid; air and indices past the palette are not.
    pub fn solid(&self, block: u8) -> bool {
        self.palette
            .get(usize::from(block))
            .is_some_and(|b| b.solid)
    }

    fn check_block(&self, block: u8) -> Result<(), Problem> {
        if usize::from(block) >= self.palette.len() {
            return Err(Problem::new(
                "voxels.range",
                format!(
                    "block {block} is past the palette's {} blocks",
                    self.palette.len()
                ),
                detail([
                    ("block", json!(block)),
                    ("palette", json!(self.palette.len())),
                ]),
            ));
        }
        Ok(())
    }

    /// Sets one cell with state 0, marking its chunk edited at `tick`; creates or removes the chunk
    /// as needed. Returns whether the cell changed.
    pub fn set(&mut self, cell: Cell, block: u8, tick: u64) -> Result<bool, Problem> {
        self.set_with_state(cell, block, 0, tick)
    }

    /// Sets one cell and its state byte (liquids.md 2); see [`Voxels::set`].
    pub fn set_with_state(
        &mut self,
        cell: Cell,
        block: u8,
        state: u8,
        tick: u64,
    ) -> Result<bool, Problem> {
        self.check_block(block)?;
        let (key, at) = locate(cell);
        let i = match self.find(key) {
            Ok(i) => i,
            Err(_) if block == 0 => return Ok(false),
            Err(i) => {
                self.chunks.insert(
                    i,
                    Chunk {
                        key,
                        cells: Cells::default(),
                        edited: tick,
                        states: None,
                    },
                );
                i
            }
        };
        let chunk = &mut self.chunks[i];
        let old = chunk.states.as_ref().map_or(0, |s| s.0[at]);
        if chunk.cells.0[at] == block && old == state {
            return Ok(false);
        }
        chunk.cells.0[at] = block;
        chunk.edited = tick;
        if state != 0 {
            chunk.states.get_or_insert_with(Cells::default).0[at] = state;
        } else if let Some(s) = chunk.states.as_mut() {
            s.0[at] = 0;
            if s.0.iter().all(|&b| b == 0) {
                chunk.states = None;
            }
        }
        if block == 0 && chunk.cells.0.iter().all(|&c| c == 0) {
            self.chunks.remove(i);
        }
        Ok(true)
    }

    /// Sets every cell of the inclusive box `lo..=hi`. Returns the cells that changed.
    pub fn fill(&mut self, lo: Cell, hi: Cell, block: u8, tick: u64) -> Result<u64, Problem> {
        self.check_block(block)?;
        let mut count: u64 = 1;
        for a in 0..3 {
            if lo[a] > hi[a] {
                return Err(Problem::new(
                    "voxels.range",
                    format!("fill's corner {lo:?} is past {hi:?}"),
                    detail([("lo", json!(lo)), ("hi", json!(hi))]),
                ));
            }
            count = count.saturating_mul(u64::from(hi[a].abs_diff(lo[a])) + 1);
        }
        if count > MAX_FILL {
            return Err(Problem::new(
                "voxels.range",
                format!("fill covers {count} cells, more than {MAX_FILL}"),
                detail([("cells", json!(count)), ("max", json!(MAX_FILL))]),
            ));
        }
        let (klo, _) = locate(lo);
        let (khi, _) = locate(hi);
        let mut changed = 0;
        for ky in klo[1]..=khi[1] {
            for kz in klo[2]..=khi[2] {
                for kx in klo[0]..=khi[0] {
                    changed += self.fill_chunk([kx, ky, kz], lo, hi, block, tick);
                }
            }
        }
        Ok(changed)
    }

    fn fill_chunk(&mut self, key: Cell, lo: Cell, hi: Cell, block: u8, tick: u64) -> u64 {
        let i = match self.find(key) {
            Ok(i) => i,
            Err(_) if block == 0 => return 0,
            Err(i) => {
                self.chunks.insert(
                    i,
                    Chunk {
                        key,
                        cells: Cells::default(),
                        edited: tick,
                        states: None,
                    },
                );
                i
            }
        };
        let base = key.map(|k| k * CHUNK);
        let local =
            |c: Cell, a: usize| (c[a] - base[a]).clamp(0, CHUNK - 1).unsigned_abs() as usize;
        let from = [0, 1, 2].map(|a| local(lo, a));
        let to = [0, 1, 2].map(|a| local(hi, a));
        let chunk = &mut self.chunks[i];
        let cells = &mut chunk.cells.0;
        let mut states = chunk.states.as_mut().map(|s| &mut s.0);
        let mut changed = 0;
        for y in from[1]..=to[1] {
            for z in from[2]..=to[2] {
                for x in from[0]..=to[0] {
                    let at = x + 16 * (z + 16 * y);
                    let stated = states.as_ref().is_some_and(|s| s[at] != 0);
                    if cells[at] != block || stated {
                        cells[at] = block;
                        if let Some(s) = states.as_mut() {
                            s[at] = 0;
                        }
                        changed += 1;
                    }
                }
            }
        }
        if chunk
            .states
            .as_ref()
            .is_some_and(|s| s.0.iter().all(|&b| b == 0))
        {
            chunk.states = None;
        }
        if changed > 0 {
            self.chunks[i].edited = tick;
        }
        if block == 0 && self.chunks[i].cells.0.iter().all(|&c| c == 0) {
            self.chunks.remove(i);
        }
        changed
    }

    /// The first non-air cell (`solid_only`: the first solid one) a ray from `from` along `dir`
    /// (world metres; `dir` need not be unit) enters within `max_m`, walking the grid exactly
    /// (Amanatides and Woo). `origin` is the volume's position. `None` for a zero direction.
    pub fn raycast(
        &self,
        origin: [f64; 3],
        from: [f64; 3],
        dir: [f64; 3],
        max_m: f64,
        solid_only: bool,
    ) -> Option<VoxelHit> {
        let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
        if !(len.is_finite() && len > 0.0 && max_m.is_finite() && max_m >= 0.0) {
            return None;
        }
        let d = dir.map(|c| c / len);
        let p = [0, 1, 2].map(|a| (from[a] - origin[a]) / self.cell_m);
        let mut cell = [0; 3];
        for a in 0..3 {
            cell[a] = to_i32(p[a].floor()).ok()?;
        }
        let hits = |b: u8| b != 0 && (!solid_only || self.solid(b));
        let first = self.get(cell);
        if hits(first) {
            return Some(VoxelHit {
                cell,
                normal: [0; 3],
                block: first,
                distance_m: 0.0,
            });
        }
        let max_t = max_m / self.cell_m;
        let mut step = [0i32; 3];
        let mut t_max = [f64::INFINITY; 3];
        let mut t_delta = [f64::INFINITY; 3];
        for a in 0..3 {
            if d[a] > 0.0 {
                step[a] = 1;
                t_max[a] = (f64::from(cell[a]) + 1.0 - p[a]) / d[a];
                t_delta[a] = 1.0 / d[a];
            } else if d[a] < 0.0 {
                step[a] = -1;
                t_max[a] = (p[a] - f64::from(cell[a])) / -d[a];
                t_delta[a] = -1.0 / d[a];
            }
        }
        loop {
            let a = if t_max[0] <= t_max[1] && t_max[0] <= t_max[2] {
                0
            } else if t_max[1] <= t_max[2] {
                1
            } else {
                2
            };
            let t = t_max[a];
            if t > max_t {
                return None;
            }
            cell[a] = cell[a].checked_add(step[a])?;
            t_max[a] += t_delta[a];
            let block = self.get(cell);
            if hits(block) {
                let mut normal = [0; 3];
                normal[a] = -step[a];
                return Some(VoxelHit {
                    cell,
                    normal,
                    block,
                    distance_m: t * self.cell_m,
                });
            }
        }
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(char::from(B64[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Decodes standard base64 with padding; `None` for anything else.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let s = text.as_bytes();
    if !s.len().is_multiple_of(4) {
        return None;
    }
    let value = |c: u8| {
        B64.iter()
            .position(|&b| b == c)
            .and_then(|v| u32::try_from(v).ok())
    };
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    for (q, quad) in s.chunks(4).enumerate() {
        let last = q + 1 == s.len() / 4;
        let pad = quad.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for &c in &quad[..4 - pad] {
            n = (n << 6) | value(c)?;
        }
        n <<= 6 * pad;
        out.extend_from_slice(&n.to_be_bytes()[1..4 - pad]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume() -> Voxels {
        let mut v = Voxels::default();
        for (name, solid) in [("stone", true), ("grass", false)] {
            v.palette.push(Block {
                name: name.into(),
                shape: if solid {
                    BlockShape::Cube
                } else {
                    BlockShape::Cross
                },
                solid,
                faces: ["m#a".into(), "m#b".into(), "m#c".into()],
                flow: None,
            });
        }
        v
    }

    #[test]
    fn states_are_kept_beside_cells_and_dropped_when_all_zero() {
        let mut v = volume();
        v.palette.push(Block {
            name: "water".into(),
            shape: BlockShape::Liquid,
            faces: ["w".into(), String::new(), String::new()],
            flow: Some(Flow::default()),
            ..Block::default()
        });
        v.validate().unwrap();
        assert!(v.set_with_state([1, 2, 3], 3, 5, 7).unwrap());
        assert_eq!(v.cell([1, 2, 3]), (3, 5));
        assert!(v.chunks[0].states.is_some());
        assert!(v.set_with_state([1, 2, 3], 3, 5 | FALLING, 8).unwrap());
        assert_eq!(v.state([1, 2, 3]) & FALLING, FALLING);
        // A block set by hand is a source: state 0, and the chunk's states go.
        assert!(v.set([1, 2, 3], 3, 9).unwrap());
        assert_eq!(v.cell([1, 2, 3]), (3, 0));
        assert!(v.chunks[0].states.is_none());
        v.set_with_state([0, 0, 0], 3, 4, 10).unwrap();
        v.fill([0, 0, 0], [1, 1, 1], 1, 11).unwrap();
        assert_eq!(v.state([0, 0, 0]), 0, "a fill writes state 0");
        assert!(v.chunks[0].states.is_none());
        assert_eq!(v.flow(3).unwrap().spread, 7);
        assert!(v.flow(1).is_none());
        v.palette[1].flow = Some(Flow::default());
        assert!(v.validate().is_err(), "only liquids flow");
    }

    #[test]
    fn cells_are_found_in_chunks_by_floor_division() {
        assert_eq!(locate([0, 0, 0]), ([0, 0, 0], 0));
        assert_eq!(locate([15, 0, 0]), ([0, 0, 0], 15));
        assert_eq!(locate([0, 0, 1]), ([0, 0, 0], 16));
        assert_eq!(locate([0, 1, 0]), ([0, 0, 0], 256));
        assert_eq!(locate([-1, -16, -17]), ([-1, -1, -2], 15 + 16 * 15));
    }

    #[test]
    fn set_creates_and_removes_chunks_and_marks_edits() {
        let mut v = volume();
        assert_eq!(v.get([3, -2, 40]), 0);
        assert!(
            !v.set([3, -2, 40], 0, 5).unwrap(),
            "air into an absent chunk is no change"
        );
        assert!(v.chunks.is_empty());
        assert!(v.set([3, -2, 40], 1, 7).unwrap());
        assert!(v.set([-5, 0, 0], 2, 8).unwrap());
        assert_eq!(v.get([3, -2, 40]), 1);
        assert_eq!(v.chunks.len(), 2);
        assert!(v.chunks[0].key < v.chunks[1].key);
        assert_eq!(v.chunk([0, -1, 2]).unwrap().edited, 7);
        assert!(!v.set([3, -2, 40], 1, 9).unwrap());
        assert_eq!(v.chunk([0, -1, 2]).unwrap().edited, 7, "no change, no edit");
        assert!(v.set([3, -2, 40], 0, 10).unwrap());
        assert!(v.chunk([0, -1, 2]).is_none(), "an emptied chunk is removed");
        assert!(v.set([0, 0, 0], 3, 1).is_err());
        v.validate().unwrap();
    }

    #[test]
    fn fill_covers_the_inclusive_box_across_chunks() {
        let mut v = volume();
        assert_eq!(v.fill([-2, 0, -2], [17, 1, 1], 1, 3).unwrap(), 20 * 2 * 4);
        assert_eq!(
            v.chunks.len(),
            6,
            "x spans chunks -1, 0 and 1; z spans -1 and 0"
        );
        assert_eq!(v.get([-2, 0, -2]), 1);
        assert_eq!(v.get([17, 1, 1]), 1);
        assert_eq!(v.get([18, 1, 1]), 0);
        assert_eq!(v.get([0, 2, 0]), 0);
        assert_eq!(v.fill([-2, 0, -2], [17, 1, 1], 0, 4).unwrap(), 160);
        assert!(v.chunks.is_empty());
        assert!(v.fill([0, 0, 0], [-1, 0, 0], 1, 5).is_err());
        assert!(v.fill([0, 0, 0], [4096, 4096, 0], 1, 5).is_err());
        v.validate().unwrap();
    }

    #[test]
    fn rays_find_the_first_cell_and_the_face_entered() {
        let mut v = volume();
        v.set([5, 0, 0], 1, 1).unwrap();
        v.set([2, 0, 0], 2, 1).unwrap();
        let o = [0.0; 3];
        let hit = v
            .raycast(o, [0.5, 0.5, 0.5], [1.0, 0.0, 0.0], 10.0, false)
            .unwrap();
        assert_eq!(
            (hit.cell, hit.normal, hit.block),
            ([2, 0, 0], [-1, 0, 0], 2)
        );
        assert_eq!(hit.distance_m, 1.5);
        let solid = v
            .raycast(o, [0.5, 0.5, 0.5], [2.0, 0.0, 0.0], 10.0, true)
            .unwrap();
        assert_eq!((solid.cell, solid.block), ([5, 0, 0], 1));
        assert!(
            v.raycast(o, [0.5, 0.5, 0.5], [1.0, 0.0, 0.0], 4.0, true)
                .is_none()
        );
        // From above, falling onto the top face; a volume moved by its origin.
        // The volume at x = 10: its cell 5 spans x 15 to 16.
        let down = v.raycast(
            [10.0, 0.0, 0.0],
            [15.5, 3.0, 0.5],
            [0.0, -1.0, 0.0],
            5.0,
            true,
        );
        let down = down.unwrap();
        assert_eq!(
            (down.cell, down.normal, down.distance_m),
            ([5, 0, 0], [0, 1, 0], 2.0)
        );
        assert!(
            v.raycast(
                [10.0, 0.0, 0.0],
                [5.5, 3.0, 0.5],
                [0.0, -1.0, 0.0],
                5.0,
                true
            )
            .is_none()
        );
        // Diagonal, negative cells.
        v.set([-3, -3, -3], 1, 1).unwrap();
        let diag = v
            .raycast(o, [0.5, 0.5, 0.5], [-1.0, -1.0, -1.0], 10.0, true)
            .unwrap();
        assert_eq!(diag.cell, [-3, -3, -3]);
        // Inside a cell: a hit at once, with no face.
        let inside = v
            .raycast(o, [5.2, 0.4, 0.9], [0.0, 1.0, 0.0], 1.0, false)
            .unwrap();
        assert_eq!((inside.normal, inside.distance_m), ([0; 3], 0.0));
        assert!(v.raycast(o, [0.0; 3], [0.0; 3], 1.0, false).is_none());
    }

    #[test]
    fn validation_names_the_field() {
        let mut v = volume();
        v.palette[0].solid = true;
        assert_eq!(v.validate().unwrap_err().code, "voxels.invalid");
        let mut v = volume();
        v.palette[1].name = "Stone".into();
        assert!(v.validate().is_err());
        let mut v = volume();
        v.palette.push(v.palette[1].clone());
        assert!(v.validate().is_err());
        let mut v = volume();
        v.set([0, 0, 0], 2, 1).unwrap();
        v.palette.pop();
        assert!(v.validate().is_err(), "a cell past the palette");
        let mut v = volume();
        v.palette[1].faces[2].clear();
        assert!(v.validate().is_err());
    }

    #[test]
    fn cells_are_base64_in_json() {
        for bytes in [
            &b""[..],
            b"a",
            b"ab",
            b"abc",
            b"abcd",
            &[0, 255, 7, 128, 64],
        ] {
            assert_eq!(base64_decode(&base64_encode(bytes)).unwrap(), bytes);
        }
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert!(base64_decode("TW=u").is_none());
        let mut v = volume();
        v.set([1, 2, 3], 1, 4).unwrap();
        let text = serde_json::to_string(&v).unwrap();
        let back: Voxels = serde_json::from_str(&text).unwrap();
        assert_eq!(back, v);
    }
}
