//! Versioned CPU-owned description of a reconstructible resident water field. Packed words are
//! the exact GPU f32 bits, not decimalized floats. No device handle enters this representation.
use amoris_sim::ContentHash;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const MAX_STEPS: usize = 64;
/// Explicit probes and current-pose buoyancy points share one bounded per-region readback.
pub const MAX_SAMPLES: usize = 4096;
/// One transient resident execution batch, bounded by regions and logical column count.
/// The graphics cache may split it further to respect retained allocation capacity.
pub const MAX_BATCH_REGIONS: usize = 16;
pub const MAX_BATCH_COLUMNS: usize = 4 * 512 * 512;

/// Runtime replay identity for the actual numerical kernels/dispatch implementation. This is
/// not a file-audit inventory: a changed solver must not reuse an old execution environment.
/// v8 uses limited linear depth flux and reconstructed donor-volume budgets. Changing it
/// here also changes the feed fingerprint, so a previous viewport refuses incompatible programs.
pub fn execution_key(parts: &[&[u8]]) -> String {
    let mut key = blake3::Hasher::new_derive_key("Amoris resident water execution v8");
    for part in parts {
        key.update(&(part.len() as u64).to_le_bytes());
        key.update(part);
    }
    key.finalize().to_hex().to_string()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FieldCheckpoint {
    pub size: [u32; 2],
    pub datum: f64,
    /// Relative bed, depth and minus-face velocities, row-major, as exact f32 words.
    pub columns: Vec<[u32; 4]>,
    /// East faces (one per z), then north faces (one per x), as exact f32 words.
    pub far_faces: Vec<u32>,
}
impl FieldCheckpoint {
    pub fn new(size: [u32; 2], datum: f64, columns: Vec<[u32; 4]>) -> Self {
        Self {
            size,
            datum,
            columns,
            far_faces: vec![0; (size[0] + size[1]) as usize],
        }
    }
    /// Padded storage words. Column data stays contiguous; only the small face tail is copied.
    pub fn packed_faces(&self) -> Vec<u32> {
        let mut faces = self.far_faces.clone();
        faces.resize(faces.len().div_ceil(4) * 4, 0);
        faces
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.size.iter().any(|&n| !(2..=512).contains(&n))
            || !self.datum.is_finite()
            || self.columns.len() != self.size[0] as usize * self.size[1] as usize
            || self.far_faces.len() != (self.size[0] + self.size[1]) as usize
            || self
                .far_faces
                .iter()
                .any(|v| !f32::from_bits(*v).is_finite())
        {
            return Err("water checkpoint dimensions, datum or column count are invalid".into());
        }
        for words in &self.columns {
            let c = words.map(f32::from_bits);
            if c.iter().any(|v| !v.is_finite()) || c[1] < 0.0 {
                return Err("water checkpoint has invalid values".into());
            }
        }
        Ok(())
    }
    /// The same unkeyed byte hash the GPU produces. Header identity is separate so the digest
    /// can be checked against the actual storage buffer without transferring it every tick.
    pub fn digest(&self) -> [u8; 32] {
        // One contiguous input lets BLAKE3 hash several chunks together with SIMD. Feeding one
        // 1 KiB chunk at a time prevents that batching and inflated periodic checkpoint cost.
        #[cfg(target_endian = "little")]
        {
            let mut h = blake3::Hasher::new();
            h.update(bytemuck::cast_slice(&self.columns));
            h.update(bytemuck::cast_slice(&self.packed_faces()));
            *h.finalize().as_bytes()
        }
        #[cfg(target_endian = "big")]
        {
            let mut h = blake3::Hasher::new();
            let mut block = [0u8; 1024];
            for columns in self.columns.chunks(64) {
                for (i, word) in columns.iter().flatten().enumerate() {
                    block[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
                }
                h.update(&block[..columns.len() * 16]);
            }
            for word in self.packed_faces() {
                h.update(&word.to_le_bytes());
            }
            *h.finalize().as_bytes()
        }
    }
    pub fn key(&self) -> ContentHash {
        self.identity().0
    }
    /// Compute the byte digest once when both the field digest and checkpoint identity are used.
    pub fn identity(&self) -> (ContentHash, [u8; 32]) {
        let digest = self.digest();
        let mut h = ContentHash::hasher("Amoris resident water checkpoint v2");
        for n in self.size {
            h.update(&n.to_le_bytes());
        }
        h.update(&self.datum.to_bits().to_le_bytes());
        h.update(&digest);
        (h.finish(), digest)
    }
}

/// Immutable checkpoint data whose CPU validity and content identity were computed at entry.
/// This proves payload validity/identity, not that a solver executed the right program. A host
/// still compares the digest with its independent GPU receipt and validates the recipe.
///
/// Payload mutation requires extracting/building raw data and checking a new object:
/// ```compile_fail
/// use amoris_assets::water_field::FieldCheckpoint;
/// let mut c=FieldCheckpoint::new([2,2],0.0,vec![[0;4];4]).checked().unwrap();
/// c.columns[0][1]=1.0f32.to_bits();
/// ```
#[derive(Clone, Debug)]
pub struct CheckedCheckpoint {
    raw: FieldCheckpoint,
    key: ContentHash,
    digest: [u8; 32],
}
impl CheckedCheckpoint {
    pub fn new(raw: FieldCheckpoint) -> Result<Self, String> {
        raw.validate()?;
        let (key, digest) = raw.identity();
        Ok(Self { raw, key, digest })
    }
    pub fn as_raw(&self) -> &FieldCheckpoint {
        &self.raw
    }
    pub fn into_raw(self) -> FieldCheckpoint {
        self.raw
    }
    pub fn key(&self) -> ContentHash {
        self.key
    }
    pub fn digest(&self) -> [u8; 32] {
        self.digest
    }
    pub fn identity(&self) -> (ContentHash, [u8; 32]) {
        (self.key, self.digest)
    }
}
impl std::ops::Deref for CheckedCheckpoint {
    type Target = FieldCheckpoint;
    fn deref(&self) -> &Self::Target {
        &self.raw
    }
}
impl PartialEq for CheckedCheckpoint {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}
impl Serialize for CheckedCheckpoint {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.raw.serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for CheckedCheckpoint {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(FieldCheckpoint::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl FieldCheckpoint {
    pub fn checked(self) -> Result<CheckedCheckpoint, String> {
        CheckedCheckpoint::new(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ColumnDelta {
    pub column: u32,
    pub depth_m: f64,
}

/// Horizontal impulse into water, kg m/s. Position is in local grid coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PointImpulse {
    pub at: [f64; 2],
    pub impulse: [f64; 2],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub enum FieldBoundary {
    #[default]
    Wall,
    Inflow {
        m3s_per_m: f64,
    },
    Outflow,
    /// Outward-only discharge against a receiving surface, relative to checkpoint datum.
    Stage {
        surface_relative_m: f64,
    },
    /// Uniform reservoir state at one side; outward velocity is positive in the side's normal.
    /// Incoming characteristics are prescribed, outgoing characteristics come from the field.
    Radiation {
        surface_relative_m: f64,
        outward_velocity_mps: f64,
    },
    /// Spatial wave forcing from the FieldStep's required sea descriptor.
    Sea,
}
impl FieldBoundary {
    pub fn validate(self) -> Result<(), String> {
        if let Self::Inflow { m3s_per_m: q } = self
            && (!q.is_finite() || q < 0.0 || q > f64::from(f32::MAX))
        {
            return Err("water boundary discharge is invalid".into());
        }
        if let Self::Stage {
            surface_relative_m: h,
        } = self
            && (!h.is_finite() || h.abs() > f64::from(f32::MAX))
        {
            return Err("water outlet stage is invalid".into());
        }
        if let Self::Radiation {
            surface_relative_m: h,
            outward_velocity_mps: u,
        } = self
            && [h, u]
                .iter()
                .any(|v| !v.is_finite() || v.abs() > f64::from(f32::MAX))
        {
            return Err("water radiation boundary is invalid".into());
        }
        Ok(())
    }
}

pub const MAX_SEA_WAVES: usize = 16;

/// Deep-water phase along the boundary, with shallow-water incoming impedance evaluated on GPU.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FieldSeaWave {
    pub amplitude_m: f64,
    pub wave_vector_rad_m: [f64; 2],
    pub angular_frequency_rad_s: f64,
    /// Phase at the region origin and start of this recorded step, reduced to [-pi, pi].
    pub phase_at_start_rad: f64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FieldSea {
    pub level_relative_m: f64,
    pub waves: Vec<FieldSeaWave>,
}
impl FieldSea {
    pub fn validate(&self) -> Result<(), String> {
        if !self.level_relative_m.is_finite()
            || self.level_relative_m.abs() > f64::from(f32::MAX)
            || self.waves.len() > MAX_SEA_WAVES
        {
            return Err("resident sea level/wave count is invalid".into());
        }
        for w in &self.waves {
            if [
                w.amplitude_m,
                w.wave_vector_rad_m[0],
                w.wave_vector_rad_m[1],
                w.angular_frequency_rad_s,
                w.phase_at_start_rad,
            ]
            .iter()
            .any(|v| !v.is_finite() || v.abs() > f64::from(f32::MAX))
                || w.angular_frequency_rad_s < f64::from(f32::MIN_POSITIVE)
                || w.phase_at_start_rad.abs() > std::f64::consts::PI + 1e-12
            {
                return Err("resident sea wave is invalid".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FieldStep {
    pub boundaries: [FieldBoundary; 4],
    pub sea: Option<FieldSea>,
    pub cell_m: f64,
    pub substep_dt: f64,
    pub substeps: u32,
    pub friction: f64,
    pub dry_below: f64,
    /// Ordered depth edits before the first substep; each removal clamps to available water.
    pub edits: Vec<ColumnDelta>,
    /// Applied before depth edits/transport; reactions queued after buoyancy enter next tick.
    pub impulses: Vec<PointImpulse>,
}
impl FieldStep {
    pub fn validate(&self, columns: usize) -> Result<(), String> {
        for boundary in self.boundaries {
            boundary.validate()?;
        }
        if self.boundaries.contains(&FieldBoundary::Sea) != self.sea.is_some() {
            return Err(
                "resident sea controls require exactly one matching wave descriptor".into(),
            );
        }
        if let Some(sea) = &self.sea {
            sea.validate()?;
        }

        if !(0.25..=4.0).contains(&self.cell_m)
            || !self.substep_dt.is_finite()
            || self.substep_dt <= 0.0
            || self.substep_dt < f64::from(f32::MIN_POSITIVE)
            || self.substep_dt > 1.0
            || !(1..=8).contains(&self.substeps)
            || !(0.0..=10.0).contains(&self.friction)
            || !(0.0..=0.05).contains(&self.dry_below)
            || self.edits.iter().any(|e| {
                e.column as usize >= columns
                    || !e.depth_m.is_finite()
                    || e.depth_m.abs() > f64::from(f32::MAX)
            })
            || self.impulses.len() > MAX_SAMPLES
            || self.impulses.iter().any(|p| {
                p.at.iter()
                    .chain(&p.impulse)
                    .any(|v| !v.is_finite() || v.abs() > f64::from(f32::MAX))
            })
        {
            return Err("resident water step or depth edit is invalid".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FieldRecipe {
    checkpoint: ContentHash,
    #[serde(serialize_with = "serialize_steps")]
    #[schemars(with = "Vec<FieldStep>")]
    steps: std::sync::Arc<Vec<std::sync::Arc<FieldStep>>>,
    pub digest: [u8; 32],
    pub environment: String,
    #[serde(skip)]
    #[schemars(skip)]
    heads: std::sync::Arc<Vec<ContentHash>>,
}
fn serialize_steps<S: serde::Serializer>(
    steps: &[std::sync::Arc<FieldStep>],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeSeq;
    let mut seq = serializer.serialize_seq(Some(steps.len()))?;
    for step in steps {
        seq.serialize_element(step.as_ref())?;
    }
    seq.end()
}
impl<'de> Deserialize<'de> for FieldRecipe {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename = "FieldRecipe", deny_unknown_fields)]
        struct Input {
            checkpoint: ContentHash,
            steps: Vec<FieldStep>,
            digest: [u8; 32],
            environment: String,
        }
        let input = Input::deserialize(deserializer)?;
        if input.steps.len() > MAX_STEPS {
            return Err(serde::de::Error::custom(
                "resident water program exceeds replay bound",
            ));
        }
        Ok(Self::new(
            input.checkpoint,
            input.steps,
            input.digest,
            input.environment,
        ))
    }
}
impl FieldRecipe {
    pub fn new(
        checkpoint: ContentHash,
        steps: Vec<FieldStep>,
        digest: [u8; 32],
        environment: String,
    ) -> Self {
        let mut recipe = Self {
            checkpoint,
            steps: std::sync::Arc::new(Vec::with_capacity(steps.len())),
            digest,
            environment,
            heads: std::sync::Arc::new(vec![Self::base_key(checkpoint)]),
        };
        for step in steps {
            recipe.push_step(step);
        }
        recipe
    }
    pub fn checkpoint(&self) -> ContentHash {
        self.checkpoint
    }
    pub fn steps(&self) -> &[std::sync::Arc<FieldStep>] {
        &self.steps
    }
    pub fn push_step(&mut self, step: FieldStep) {
        let head = Self::extend_key(self.program_key(), &step);
        std::sync::Arc::make_mut(&mut self.steps).push(std::sync::Arc::new(step));
        std::sync::Arc::make_mut(&mut self.heads).push(head);
    }
    pub fn reset(&mut self, checkpoint: ContentHash, digest: [u8; 32]) {
        self.checkpoint = checkpoint;
        self.digest = digest;
        self.steps = std::sync::Arc::new(vec![]);
        self.heads = std::sync::Arc::new(vec![Self::base_key(checkpoint)]);
    }
    pub fn validate(&self, checkpoint: &FieldCheckpoint) -> Result<(), String> {
        checkpoint.validate()?;
        let (key, digest) = checkpoint.identity();
        self.validate_identity(key, digest, checkpoint.columns.len())
    }
    /// A checked payload is immutable; its cached identity remains tied to these exact bytes.
    pub fn validate_checked(&self, checkpoint: &CheckedCheckpoint) -> Result<(), String> {
        self.validate_identity(
            checkpoint.key(),
            checkpoint.digest(),
            checkpoint.columns.len(),
        )
    }
    fn validate_identity(
        &self,
        key: ContentHash,
        digest: [u8; 32],
        columns: usize,
    ) -> Result<(), String> {
        if self.checkpoint != key || self.steps.len() > MAX_STEPS {
            return Err(
                "resident water recipe has the wrong checkpoint or exceeds its replay bound".into(),
            );
        }
        for step in self.steps.iter() {
            step.validate(columns)?;
        }
        if self.steps.is_empty() && self.digest != digest {
            return Err("resident water checkpoint digest differs from its recipe".into());
        }
        Ok(())
    }
    /// Program identity excludes returned digest/environment. A cache must additionally compare
    /// those, so a bad field cannot authenticate itself merely by receiving the right commands.
    pub fn program_key(&self) -> ContentHash {
        *self.heads.last().expect("resident water checkpoint head")
    }
    pub fn prefix_key(&self, count: usize) -> Option<ContentHash> {
        self.heads.get(count).copied()
    }
    pub fn base_key(checkpoint: ContentHash) -> ContentHash {
        ContentHash::derive("Amoris resident water program base v2", &checkpoint.0)
    }
    pub fn extend_key(previous: ContentHash, step: &FieldStep) -> ContentHash {
        // Hash each bounded step in one contiguous slice: feeding BLAKE3 one f64 at a time
        // disables its bulk path and is much slower for interaction-heavy programs.
        let bytes = bincode2::serde::encode_to_vec(
            step,
            bincode2::config::standard().with_fixed_int_encoding(),
        )
        .expect("resident water step serialization");
        let mut key = ContentHash::hasher("Amoris resident water program step v2");
        key.update(&previous.0).update(&bytes);
        key.finish()
    }
    /// Presentation only. The world retains the complete program; this update extends a named
    /// previously published prefix, or replaces it from the checkpoint when start is zero.
    pub fn patch(&self, start: usize) -> FieldPatch {
        let base = self.prefix_key(start).expect("resident patch prefix");
        self.patch_with_base(start, base)
    }
    pub fn patch_after(&self, previous: Option<(usize, ContentHash)>) -> FieldPatch {
        let matched =
            previous.and_then(|(n, key)| self.prefix_key(n).filter(|p| *p == key).map(|p| (n, p)));
        let (start, base) = matched.unwrap_or_else(|| (0, Self::base_key(self.checkpoint)));
        self.patch_with_base(start, base)
    }
    fn patch_with_base(&self, start: usize, base: ContentHash) -> FieldPatch {
        let steps = self.steps[start..].iter().map(|s| (**s).clone()).collect();
        let program = self.program_key();
        FieldPatch {
            checkpoint: self.checkpoint,
            start: u32::try_from(start).expect("bounded program"),
            base,
            steps,
            program,
            digest: self.digest,
            environment: self.environment.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldPatch {
    pub checkpoint: ContentHash,
    pub start: u32,
    pub base: ContentHash,
    pub steps: Vec<FieldStep>,
    pub program: ContentHash,
    pub digest: [u8; 32],
    pub environment: String,
}
impl FieldPatch {
    pub fn end(&self) -> usize {
        self.start as usize + self.steps.len()
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.start as usize > MAX_STEPS || self.steps.len() > MAX_STEPS || self.end() > MAX_STEPS
        {
            return Err("resident visual patch exceeds the replay bound".into());
        }
        if self.start == 0 && self.base != FieldRecipe::base_key(self.checkpoint) {
            return Err("resident visual replacement has the wrong checkpoint prefix".into());
        }
        if self.steps.iter().fold(self.base, FieldRecipe::extend_key) != self.program {
            return Err("resident visual patch program identity differs from its steps".into());
        }
        Ok(())
    }
    pub fn check_append(&self, next: &Self) -> Result<(), String> {
        next.validate()?;
        if next.start == 0 {
            return Ok(());
        }
        self.validate()?;
        if self.checkpoint != next.checkpoint
            || self.end() != next.start as usize
            || self.program != next.base
            || self.environment != next.environment
        {
            return Err("resident visual patch does not extend the pending prefix".into());
        }
        if self.start as usize + self.steps.len() + next.steps.len() > MAX_STEPS {
            return Err("merged resident visual patch exceeds the replay bound".into());
        }
        Ok(())
    }
    pub fn append(&mut self, next: Self) -> Result<(), String> {
        self.check_append(&next)?;
        self.append_validated(next);
        Ok(())
    }
    pub(crate) fn append_validated(&mut self, next: Self) {
        if next.start == 0 {
            *self = next;
            return;
        }
        self.steps.extend(next.steps);
        self.program = next.program;
        self.digest = next.digest;
        self.environment = next.environment;
    }
    pub fn apply(&self, previous: Option<&FieldRecipe>) -> Result<FieldRecipe, String> {
        self.validate()?;
        let mut recipe = if self.start == 0 {
            FieldRecipe::new(
                self.checkpoint,
                vec![],
                self.digest,
                self.environment.clone(),
            )
        } else {
            let old = previous.ok_or("resident visual patch has no cached base program")?;
            if old.checkpoint != self.checkpoint
                || old.steps.len() != self.start as usize
                || old.program_key() != self.base
                || old.environment != self.environment
            {
                return Err("resident visual patch base program is missing or different".into());
            }
            old.clone()
        };
        for step in &self.steps {
            recipe.push_step(step.clone());
        }
        recipe.digest = self.digest;
        recipe.environment.clone_from(&self.environment);
        // validate() already checked extension identity; the retained prefix was checked above.
        Ok(recipe)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checkpoint_hash_preserves_bits_header_and_bounded_program() {
        let mut c = FieldCheckpoint::new(
            [4, 4],
            1_000_000.0,
            vec![[0.0f32.to_bits(), 1.0f32.to_bits(), 0, 0]; 16],
        );
        c.validate().unwrap();
        let key = c.key();
        let digest = c.digest();
        c.datum += 0.125;
        assert_ne!(key, c.key());
        assert_eq!(digest, c.digest());
        c.columns[0][2] = (-0.0f32).to_bits();
        c.validate().unwrap();
        assert_ne!(digest, c.digest());
        let mut r = FieldRecipe::new(c.key(), vec![], c.digest(), "test".into());
        r.validate(&c).unwrap();
        r = FieldRecipe::new(
            c.key(),
            vec![
                FieldStep {
                    sea: None,
                    boundaries: [crate::water_field::FieldBoundary::Wall; 4],
                    cell_m: 1.0,
                    substep_dt: 1.0 / 120.0,
                    substeps: 2,
                    friction: 0.02,
                    dry_below: 0.0,
                    edits: vec![],
                    impulses: vec![],
                };
                MAX_STEPS + 1
            ],
            c.digest(),
            "test".into(),
        );
        assert!(r.validate(&c).is_err());
    }
    #[test]
    fn complete_faces_are_required_finite_and_part_of_state_identity() {
        let mut c = FieldCheckpoint::new([3, 2], 0.0, vec![[0, 1.0f32.to_bits(), 0, 0]; 6]);
        c.validate().unwrap();
        let digest = c.digest();
        let key = c.key();
        c.far_faces[0] = (-0.0f32).to_bits();
        assert_ne!(c.digest(), digest);
        assert_ne!(c.key(), key);
        assert_eq!(c.packed_faces().len(), 8);
        c.far_faces[0] = f32::NAN.to_bits();
        assert!(c.validate().is_err());
        c.far_faces.clear();
        assert!(c.validate().is_err());
    }
    #[test]
    fn execution_identity_and_required_impulses_do_not_silently_reuse_old_programs() {
        assert_ne!(
            super::execution_key(&[b"ab".as_slice(), b"c".as_slice()]),
            super::execution_key(&[b"a".as_slice(), b"bc".as_slice()])
        );
        assert_ne!(
            super::execution_key(&[b"kernel 1"]),
            super::execution_key(&[b"kernel 2"])
        );
        let old = serde_json::json!({"cell_m":1.0,"substep_dt":0.01,"substeps":1,"friction":0.0,"dry_below":0.0,"edits":[]});
        assert!(serde_json::from_value::<super::FieldStep>(old.clone()).is_err());
        let mut current = old;
        current["impulses"] = serde_json::json!([]);
        assert!(serde_json::from_value::<super::FieldStep>(current.clone()).is_err());
        current["boundaries"] = serde_json::json!(["Wall", "Wall", "Wall", "Wall"]);
        assert!(serde_json::from_value::<super::FieldStep>(current).is_ok());
    }
    #[test]
    fn characteristic_boundary_controls_are_finite_signed_and_round_trip() {
        use super::FieldBoundary;
        let boundary = FieldBoundary::Radiation {
            surface_relative_m: -12.0,
            outward_velocity_mps: -0.5,
        };
        boundary.validate().unwrap();
        let json = serde_json::to_string(&boundary).unwrap();
        assert_eq!(
            serde_json::from_str::<FieldBoundary>(&json).unwrap(),
            boundary
        );
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX] {
            assert!(
                FieldBoundary::Radiation {
                    surface_relative_m: value,
                    outward_velocity_mps: 0.0
                }
                .validate()
                .is_err()
            );
            assert!(
                FieldBoundary::Radiation {
                    surface_relative_m: 0.0,
                    outward_velocity_mps: value
                }
                .validate()
                .is_err()
            );
        }
    }
    #[test]
    fn sea_steps_require_matching_finite_bounded_descriptors_and_hash_their_phase() {
        use super::{
            FieldBoundary, FieldCheckpoint, FieldRecipe, FieldSea, FieldSeaWave, FieldStep,
            MAX_SEA_WAVES,
        };
        let cp = FieldCheckpoint::new([16, 16], 0.0, vec![[0, 1.0f32.to_bits(), 0, 0]; 256]);
        let mut step = FieldStep {
            boundaries: [FieldBoundary::Sea; 4],
            sea: None,
            cell_m: 1.0,
            substep_dt: 0.01,
            substeps: 1,
            friction: 0.0,
            dry_below: 0.0,
            edits: vec![],
            impulses: vec![],
        };
        assert!(step.validate(256).is_err());
        let wave = FieldSeaWave {
            amplitude_m: 0.01,
            wave_vector_rad_m: [0.1, 0.0],
            angular_frequency_rad_s: 1.0,
            phase_at_start_rad: 0.0,
        };
        step.sea = Some(FieldSea {
            level_relative_m: 1.0,
            waves: vec![wave],
        });
        step.validate(256).unwrap();
        let a = FieldRecipe::new(cp.key(), vec![step.clone()], cp.digest(), String::new());
        step.sea.as_mut().unwrap().waves[0].phase_at_start_rad = 0.1;
        let b = FieldRecipe::new(cp.key(), vec![step.clone()], cp.digest(), String::new());
        assert_ne!(a.program_key(), b.program_key());
        step.sea.as_mut().unwrap().waves = vec![wave; MAX_SEA_WAVES + 1];
        assert!(step.validate(256).is_err());
        step.sea.as_mut().unwrap().waves = vec![wave];
        step.boundaries = [FieldBoundary::Wall; 4];
        assert!(step.validate(256).is_err());
    }
}
