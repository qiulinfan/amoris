//! Offline world-space diffuse GI probes. This asset contains radiance, before the Lambert
//! convolution; the renderer multiplies SH bands 0, 1 and 2 by 1, 2/3 and 1/4 respectively to
//! reconstruct diffuse outgoing radiance. Probe indices are x-fast: `x + nx * (y + ny * z)`.
//!
//! The directional distance moments use row-major octahedral texels. They record the distance to
//! the first geometry hit, capped at `max_distance`, independently of radiance path bounces.

use pocket_sim::math::max_f32;
use serde::{Deserialize, Serialize};

pub const BAKED_GI_FORMAT: &str = "amoris-baked-gi-v1";
pub const MAX_PROBES: usize = 65_536;
pub const MAX_DISTANCE_RESOLUTION: u32 = 32;
pub const MAX_DISTANCE_MOMENTS: usize = 8_388_608;
pub const MAX_JSON_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_RAYS_PER_PROBE: u32 = 1_048_576;
pub const MAX_BOUNCES: u32 = 16;
pub const NEURAL_GI_FORMAT: &str = "amoris-neural-gi-v1";
pub const MAX_NEURAL_JSON_BYTES: usize = 1024 * 1024;
/// The fixed 6 -> 32 -> 32 -> 3 network stores 1,312 weights and 67 biases.
pub const NEURAL_PARAMETER_COUNT: usize = 1379;

/// An offline approximation of the baked diffuse field, not an online neural radiance cache.
///
/// Input is `[2*(world-input_min)/input_extent-1, unit_normal]`. The three output-major dense
/// layers have shapes `[32,6]`, `[32,32]`, `[3,32]`; hidden layers use ReLU and the final layer is
/// linear, with the renderer clamping RGB to zero. Evaluate only inside the closed half-cell
/// AABB `[input_min, input_min + input_extent]`, and apply `Environment.gi_intensity` afterward.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NeuralGi {
    pub format: String,
    pub origin: [f32; 3],
    pub spacing: [f32; 3],
    pub dimensions: [u32; 3],
    pub input_min: [f32; 3],
    pub input_extent: [f32; 3],
    #[serde(rename = "in")]
    pub input_size: u32,
    #[serde(rename = "hidden")]
    pub hidden_size: u32,
    #[serde(rename = "output")]
    pub output_size: u32,
    pub layers: Vec<NeuralLayer>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NeuralLayer {
    /// Output-neuron/input-neuron row-major; no transposition at GPU upload.
    pub weights: Vec<Vec<f32>>,
    pub bias: Vec<f32>,
}

impl NeuralGi {
    pub fn from_json(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_NEURAL_JSON_BYTES {
            return Err(format!(
                "neural GI JSON is {} bytes; maximum is {MAX_NEURAL_JSON_BYTES}",
                bytes.len()
            ));
        }
        let network: Self = serde_json::from_slice(bytes)
            .map_err(|error| format!("invalid neural GI JSON: {error}"))?;
        network.validate()?;
        Ok(network)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        use std::io::Read;
        let file = std::fs::File::open(path)
            .map_err(|error| format!("cannot open neural GI {}: {error}", path.display()))?;
        let mut bytes = Vec::new();
        file.take(MAX_NEURAL_JSON_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("cannot read neural GI {}: {error}", path.display()))?;
        Self::from_json(&bytes)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.format != NEURAL_GI_FORMAT {
            return Err(format!(
                "unsupported neural GI format; expected {NEURAL_GI_FORMAT}"
            ));
        }
        if (self.input_size, self.hidden_size, self.output_size) != (6, 32, 3)
            || self.layers.len() != 3
        {
            return Err("neural GI must contain exactly a 6 -> 32 -> 32 -> 3 network".into());
        }
        let count = self
            .dimensions
            .iter()
            .try_fold(1usize, |count, &dimension| {
                if dimension == 0 {
                    return Err("neural GI dimensions must be positive".to_string());
                }
                count
                    .checked_mul(dimension as usize)
                    .ok_or_else(|| "neural GI dimensions overflow".to_string())
            })?;
        if count > MAX_PROBES {
            return Err(format!("neural GI grid exceeds {MAX_PROBES} probes"));
        }
        for axis in 0..3 {
            if !self.origin[axis].is_finite()
                || !self.spacing[axis].is_finite()
                || self.spacing[axis] <= 0.0
                || !self.input_min[axis].is_finite()
                || !self.input_extent[axis].is_finite()
                || self.input_extent[axis] <= 0.0
                || !(self.input_min[axis] + self.input_extent[axis]).is_finite()
            {
                return Err(format!(
                    "neural GI axis {axis} has invalid or non-finite bounds"
                ));
            }
            let minimum = self.origin[axis] - 0.5 * self.spacing[axis];
            let extent = self.spacing[axis] * self.dimensions[axis] as f32;
            if self.input_min[axis] != minimum || self.input_extent[axis] != extent {
                return Err(format!(
                    "neural GI axis {axis} normalization must match the half-cell bounds"
                ));
            }
        }
        let mut scalars = 0usize;
        for (index, (layer, (outputs, inputs))) in self
            .layers
            .iter()
            .zip([(32, 6), (32, 32), (3, 32)])
            .enumerate()
        {
            if layer.weights.len() != outputs || layer.bias.len() != outputs {
                return Err(format!(
                    "neural GI layer {index} requires {outputs} output rows and biases"
                ));
            }
            for (row, weights) in layer.weights.iter().enumerate() {
                if weights.len() != inputs || !weights.iter().all(|value| value.is_finite()) {
                    return Err(format!(
                        "neural GI layer {index} row {row} requires {inputs} finite weights"
                    ));
                }
                scalars += weights.len();
            }
            if !layer.bias.iter().all(|value| value.is_finite()) {
                return Err(format!("neural GI layer {index} has non-finite biases"));
            }
            scalars += layer.bias.len();
        }
        if scalars != NEURAL_PARAMETER_COUNT {
            return Err(format!(
                "neural GI requires exactly {NEURAL_PARAMETER_COUNT} scalar parameters"
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BakedGi {
    pub format: String,
    pub origin: [f32; 3],
    pub spacing: [f32; 3],
    pub dimensions: [u32; 3],
    pub probes: Vec<BakedProbe>,
    pub distance_resolution: u32,
    pub max_distance: f32,
    pub rays_per_probe: u32,
    pub bounces: u32,
    /// Fingerprint of the geometry, materials, lights and bake settings used by the baker.
    pub scene_signature: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BakedProbe {
    /// RGB real SH integrals, in band-major order. Coefficients may be negative. The basis is
    /// `[0.282095, 0.488603*y, 0.488603*z, 0.488603*x, 1.092548*x*y, 1.092548*y*z,
    /// 0.315392*(3*z*z-1), 1.092548*x*z, 0.546274*(x*x-y*y)]` for unit direction `[x,y,z]`.
    pub radiance_sh: [[f32; 3]; 9],
    /// `[E[distance], E[distance squared]]` per octahedral texel.
    pub distance_moments: Vec<[f32; 2]>,
}

impl BakedGi {
    /// Strictly decodes and validates a bounded JSON asset on native or web platforms.
    pub fn from_json(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_JSON_BYTES {
            return Err(format!(
                "baked GI JSON is {} bytes; maximum is {MAX_JSON_BYTES}",
                bytes.len()
            ));
        }
        let gi: Self = serde_json::from_slice(bytes)
            .map_err(|error| format!("invalid baked GI JSON: {error}"))?;
        gi.validate()?;
        Ok(gi)
    }

    /// Loads at most the documented JSON capacity; no GPU dependency is required.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        use std::io::Read;
        let file = std::fs::File::open(path)
            .map_err(|error| format!("cannot open baked GI {}: {error}", path.display()))?;
        let mut bytes = Vec::new();
        file.take(MAX_JSON_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("cannot read baked GI {}: {error}", path.display()))?;
        Self::from_json(&bytes)
    }

    /// Rejects malformed, non-finite and over-capacity assets before GPU allocation.
    pub fn validate(&self) -> Result<(), String> {
        if self.format != BAKED_GI_FORMAT {
            return Err(format!(
                "unsupported baked GI format {:?}; expected {BAKED_GI_FORMAT}",
                self.format
            ));
        }
        if self.scene_signature.is_empty() || self.scene_signature.len() > 256 {
            return Err("baked GI scene_signature must contain 1..256 bytes".into());
        }
        let count = self.probe_count()?;
        if self.probes.len() != count {
            return Err(format!(
                "baked GI dimensions require {count} probes, found {}",
                self.probes.len()
            ));
        }
        for axis in 0..3 {
            if !self.origin[axis].is_finite() {
                return Err(format!("baked GI origin[{axis}] must be finite"));
            }
            if !self.spacing[axis].is_finite() || self.spacing[axis] <= 0.0 {
                return Err(format!(
                    "baked GI spacing[{axis}] must be finite and positive"
                ));
            }
            let end = self.origin[axis] + self.spacing[axis] * (self.dimensions[axis] - 1) as f32;
            if !end.is_finite() {
                return Err(format!("baked GI axis {axis} has a non-finite extent"));
            }
        }
        if !(1..=MAX_DISTANCE_RESOLUTION).contains(&self.distance_resolution) {
            return Err(format!(
                "baked GI distance_resolution must be 1..{MAX_DISTANCE_RESOLUTION}"
            ));
        }
        let texels = (self.distance_resolution as usize).pow(2);
        let total = count
            .checked_mul(texels)
            .ok_or("baked GI distance moment count overflows")?;
        if total > MAX_DISTANCE_MOMENTS {
            return Err(format!(
                "baked GI requires {total} distance moments; maximum is {MAX_DISTANCE_MOMENTS}"
            ));
        }
        if !self.max_distance.is_finite()
            || self.max_distance <= 0.0
            || !(self.max_distance * self.max_distance).is_finite()
        {
            return Err("baked GI max_distance and its square must be finite and positive".into());
        }
        if !(1..=MAX_RAYS_PER_PROBE).contains(&self.rays_per_probe) {
            return Err(format!(
                "baked GI rays_per_probe must be 1..{MAX_RAYS_PER_PROBE}"
            ));
        }
        if !(1..=MAX_BOUNCES).contains(&self.bounces) {
            return Err(format!("baked GI bounces must be 1..{MAX_BOUNCES}"));
        }
        let max_squared = self.max_distance * self.max_distance;
        for (index, probe) in self.probes.iter().enumerate() {
            if !probe
                .radiance_sh
                .iter()
                .flatten()
                .all(|value| value.is_finite())
            {
                return Err(format!("baked GI probe {index} has non-finite radiance SH"));
            }
            if probe.distance_moments.len() != texels {
                return Err(format!(
                    "baked GI probe {index} requires {texels} distance moments, found {}",
                    probe.distance_moments.len()
                ));
            }
            for (texel, &[mean, second]) in probe.distance_moments.iter().enumerate() {
                // F32 accumulation may round E[d²] below E[d]² by a few ULPs.
                let tolerance = 2e-5 * max_f32(max_f32(second, mean * mean), 1.0);
                if !mean.is_finite()
                    || !second.is_finite()
                    || mean < 0.0
                    || second < 0.0
                    || mean > self.max_distance
                    || second > max_squared + tolerance
                    || second + tolerance < mean * mean
                {
                    return Err(format!(
                        "baked GI probe {index} texel {texel} has invalid distance moments"
                    ));
                }
            }
        }
        Ok(())
    }

    /// The grid capacity, checked independently of probe allocation.
    pub fn probe_count(&self) -> Result<usize, String> {
        let mut count = 1usize;
        for (axis, &dimension) in self.dimensions.iter().enumerate() {
            if dimension == 0 {
                return Err(format!("baked GI dimensions[{axis}] must be positive"));
            }
            count = count
                .checked_mul(dimension as usize)
                .ok_or("baked GI probe count overflows")?;
        }
        if count > MAX_PROBES {
            return Err(format!(
                "baked GI requires {count} probes; maximum is {MAX_PROBES}"
            ));
        }
        Ok(count)
    }

    pub fn probe_index(&self, cell: [u32; 3]) -> Option<usize> {
        if cell
            .iter()
            .zip(self.dimensions)
            .any(|(&value, size)| value >= size)
        {
            return None;
        }
        let [x, y, z] = cell.map(|value| value as usize);
        let [nx, ny, _] = self.dimensions.map(|value| value as usize);
        nx.checked_mul(ny.checked_mul(z)?.checked_add(y)?)?
            .checked_add(x)
    }

    pub fn probe_position(&self, cell: [u32; 3]) -> Option<[f32; 3]> {
        self.probe_index(cell)?;
        Some(std::array::from_fn(|axis| {
            self.origin[axis] + self.spacing[axis] * cell[axis] as f32
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn neural_fixture() -> NeuralGi {
        NeuralGi {
            format: NEURAL_GI_FORMAT.into(),
            origin: [-1.0, 0.5, -2.0],
            spacing: [2.0, 1.0, 0.5],
            dimensions: [2, 2, 2],
            input_min: [-2.0, 0.0, -2.25],
            input_extent: [4.0, 2.0, 1.0],
            input_size: 6,
            hidden_size: 32,
            output_size: 3,
            layers: [(32, 6), (32, 32), (3, 32)]
                .map(|(outputs, inputs)| NeuralLayer {
                    weights: vec![vec![-0.1; inputs]; outputs],
                    bias: vec![0.1; outputs],
                })
                .to_vec(),
        }
    }

    #[test]
    fn neural_json_matches_trainer_fields_and_keeps_signed_weights() {
        let network = neural_fixture();
        let bytes = serde_json::to_vec(&network).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["in"], 6);
        assert_eq!(json["hidden"], 32);
        assert_eq!(json["output"], 3);
        assert!(json.get("input_size").is_none());
        assert_eq!(NeuralGi::from_json(&bytes), Ok(network));
    }

    #[test]
    fn neural_json_rejects_unknown_layer_fields_and_oversized_input() {
        let mut json = serde_json::to_value(neural_fixture()).unwrap();
        json["layers"][0]["activation"] = serde_json::json!("relu");
        assert!(
            NeuralGi::from_json(&serde_json::to_vec(&json).unwrap())
                .unwrap_err()
                .contains("unknown field `activation`")
        );
        assert!(
            NeuralGi::from_json(&vec![b' '; MAX_NEURAL_JSON_BYTES + 1])
                .unwrap_err()
                .contains("maximum")
        );
    }

    #[test]
    fn neural_rejects_malformed_or_nonfinite_layers() {
        let mut network = neural_fixture();
        network.layers[0].weights[0].pop();
        assert!(network.validate().unwrap_err().contains("6 finite weights"));
        network = neural_fixture();
        network.layers[2].bias[0] = f32::NAN;
        assert!(
            network
                .validate()
                .unwrap_err()
                .contains("non-finite biases")
        );
        network = neural_fixture();
        network.layers[1].weights[0][0] = f32::INFINITY;
        assert!(network.validate().unwrap_err().contains("finite weights"));
        network = neural_fixture();
        network.hidden_size = 33;
        assert!(
            network
                .validate()
                .unwrap_err()
                .contains("6 -> 32 -> 32 -> 3")
        );
    }

    #[test]
    fn neural_rejects_invalid_normalization_and_grid_capacity() {
        let mut network = neural_fixture();
        network.input_min[0] = -1.0;
        assert!(network.validate().unwrap_err().contains("half-cell bounds"));
        network = neural_fixture();
        network.input_extent[0] = 0.0;
        assert!(network.validate().unwrap_err().contains("bounds"));
        network = neural_fixture();
        network.origin[0] = f32::INFINITY;
        assert!(network.validate().unwrap_err().contains("bounds"));
        network = neural_fixture();
        network.dimensions = [u32::MAX; 3];
        assert!(network.validate().unwrap_err().contains("overflow"));
    }

    fn fixture() -> BakedGi {
        BakedGi {
            format: BAKED_GI_FORMAT.into(),
            origin: [-1.0, 0.5, -2.0],
            spacing: [2.0, 1.0, 0.5],
            dimensions: [2, 2, 2],
            probes: vec![
                BakedProbe {
                    radiance_sh: [[0.0; 3]; 9],
                    distance_moments: vec![[4.0, 16.0]; 4],
                };
                8
            ],
            distance_resolution: 2,
            max_distance: 4.0,
            rays_per_probe: 256,
            bounces: 2,
            scene_signature: "fixture".into(),
        }
    }

    #[test]
    fn json_round_trip_keeps_grid_order_and_negative_sh() {
        let mut gi = fixture();
        gi.probes[0].radiance_sh[3] = [-2.0, 1.5, 3.0];
        let bytes = serde_json::to_vec(&gi).unwrap();
        assert_eq!(BakedGi::from_json(&bytes), Ok(gi.clone()));
        assert_eq!(gi.probe_index([1, 0, 0]), Some(1));
        assert_eq!(gi.probe_index([0, 1, 0]), Some(2));
        assert_eq!(gi.probe_index([0, 0, 1]), Some(4));
        assert_eq!(gi.probe_index([1, 1, 1]), Some(7));
        assert_eq!(gi.probe_position([1, 1, 1]), Some([1.0, 1.5, -1.5]));
        assert_eq!(gi.probe_index([2, 0, 0]), None);
    }

    #[test]
    fn rejects_unknown_json_fields_at_asset_and_probe_levels() {
        let mut json = serde_json::to_value(fixture()).unwrap();
        json["probes"][0]["radiance"] = serde_json::json!([1.0, 0.0, 0.0]);
        let error = BakedGi::from_json(&serde_json::to_vec(&json).unwrap()).unwrap_err();
        assert!(error.contains("unknown field `radiance`"), "{error}");
        json["probes"][0]
            .as_object_mut()
            .unwrap()
            .remove("radiance");
        json["intensity"] = serde_json::json!(1.0);
        let error = BakedGi::from_json(&serde_json::to_vec(&json).unwrap()).unwrap_err();
        assert!(error.contains("unknown field `intensity`"), "{error}");
    }

    #[test]
    fn rejects_wrong_format_missing_probes_and_invalid_grid_capacity() {
        let mut gi = fixture();
        gi.format = "amoris-baked-gi-v2".into();
        assert!(gi.validate().unwrap_err().contains("format"));
        gi = fixture();
        gi.probes.pop();
        assert!(gi.validate().unwrap_err().contains("8 probes"));
        gi = fixture();
        gi.dimensions = [u32::MAX; 3];
        assert!(gi.validate().unwrap_err().contains("overflows"));
        gi.dimensions = [MAX_PROBES as u32 + 1, 1, 1];
        assert!(gi.validate().unwrap_err().contains("maximum"));
        gi.dimensions = [0, 1, 1];
        assert!(gi.validate().unwrap_err().contains("positive"));
    }

    #[test]
    fn rejects_non_finite_grid_and_sh_values() {
        let mut gi = fixture();
        gi.origin[0] = f32::NAN;
        assert!(gi.validate().unwrap_err().contains("origin"));
        gi = fixture();
        gi.spacing[1] = -1.0;
        assert!(gi.validate().unwrap_err().contains("spacing"));
        gi = fixture();
        gi.origin[0] = f32::MAX;
        gi.spacing[0] = f32::MAX;
        assert!(gi.validate().unwrap_err().contains("extent"));
        gi = fixture();
        gi.probes[0].radiance_sh[0][0] = f32::INFINITY;
        assert!(gi.validate().unwrap_err().contains("radiance SH"));
    }

    #[test]
    fn rejects_invalid_distance_shape_moments_and_ranges() {
        let mut gi = fixture();
        gi.distance_resolution = MAX_DISTANCE_RESOLUTION + 1;
        assert!(gi.validate().unwrap_err().contains("distance_resolution"));
        gi = fixture();
        gi.probes[0].distance_moments.pop();
        assert!(gi.validate().unwrap_err().contains("4 distance moments"));
        for bad in [
            [f32::NAN, 16.0],
            [4.0, f32::INFINITY],
            [-1.0, 1.0],
            [2.0, -1.0],
            [5.0, 25.0],
            [2.0, 17.0],
            [2.0, 3.0],
        ] {
            gi = fixture();
            gi.probes[0].distance_moments[0] = bad;
            assert!(
                gi.validate()
                    .unwrap_err()
                    .contains("invalid distance moments")
            );
        }
        gi = fixture();
        gi.max_distance = f32::MAX;
        assert!(gi.validate().unwrap_err().contains("max_distance"));
    }

    #[test]
    fn validates_total_moment_and_bake_settings_capacity() {
        let mut gi = fixture();
        gi.dimensions = [MAX_PROBES as u32, 1, 1];
        gi.probes.resize(MAX_PROBES, gi.probes[0].clone());
        gi.distance_resolution = MAX_DISTANCE_RESOLUTION;
        assert!(gi.validate().unwrap_err().contains("maximum"));
        gi = fixture();
        gi.rays_per_probe = 0;
        assert!(gi.validate().unwrap_err().contains("rays_per_probe"));
        gi = fixture();
        gi.bounces = MAX_BOUNCES + 1;
        assert!(gi.validate().unwrap_err().contains("bounces"));
    }
}
