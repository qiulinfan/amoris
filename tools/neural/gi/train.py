"""Fit a 6 -> 32 -> 32 -> 3 MLP to Amoris's baked diffuse probe field.

    uv run --project tools/neural/gi tools/neural/gi/train.py \
        --input samples/gi/assets/room.gi.json --output /tmp/room.neural-gi.json

This is an offline approximation of a baked light field, not online neural radiance caching.
The teacher is probe_gi.wgsl's baked_diffuse, not an unbiased ground-truth path tracer. Held-out
metrics compare random positions and normals against that teacher, not rendered image quality.
Input position uses 2 * (world - input_min) / input_extent - 1; input normals remain unit vectors.
Every layer is output-neuron/input-neuron row-major. Hidden layers use ReLU; the final layer is
linear, with max(RGB, 0) at shader inference. All teacher and network arithmetic uses float32.
"""

from __future__ import annotations

import argparse
import hashlib
import itertools
import json
import math
import pathlib
import time
from dataclasses import dataclass

import numpy as np
import torch

BAKED_FORMAT = "amoris-baked-gi-v1"
NETWORK_FORMAT = "amoris-neural-gi-v1"
MAX_JSON_BYTES = 64 * 1024 * 1024
MAX_PROBES = 65_536
MAX_DISTANCE_RESOLUTION = 32
MAX_DISTANCE_MOMENTS = 8_388_608
MAX_SAMPLES = 1_048_576
MAX_STEPS = 100_000
F32 = np.float32


def _object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON field {key!r}")
        result[key] = value
    return result


def _fields(value, names, label):
    if not isinstance(value, dict) or set(value) != set(names):
        raise ValueError(f"{label} requires exactly these fields: {', '.join(names)}")


def _numbers(value, label):
    if isinstance(value, list):
        for item in value:
            _numbers(item, label)
    elif isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f"{label} must contain JSON numbers")


def _f32(value, shape, label):
    _numbers(value, label)
    with np.errstate(over="ignore", invalid="ignore"):
        array = np.asarray(value, dtype=np.float32)
    if array.shape != shape or not np.isfinite(array).all():
        raise ValueError(f"{label} must have finite float32 shape {shape}")
    return array


def _integer(value, minimum, maximum, label):
    if type(value) is not int or not minimum <= value <= maximum:
        raise ValueError(f"{label} must be an integer in {minimum}..{maximum}")
    return value


@dataclass
class Teacher:
    origin: np.ndarray
    spacing: np.ndarray
    dimensions: np.ndarray
    sh: np.ndarray
    moments: np.ndarray
    resolution: int
    source_hash: str
    scene_signature: str

    @property
    def input_min(self):
        return self.origin - F32(0.5) * self.spacing

    @property
    def input_extent(self):
        return self.spacing * self.dimensions.astype(np.float32)

    def features(self, positions, normals):
        normalized = F32(2.0) * (positions - self.input_min) / self.input_extent - F32(1.0)
        return np.concatenate((normalized, normals), axis=1).astype(np.float32)

    def sample_moments(self, indices, uv):
        """Clamp bilinear octahedral moments at texel centers, as gi_moments in WGSL."""
        texel = np.clip(uv * F32(self.resolution) - F32(0.5), F32(0.0), F32(self.resolution - 1))
        low = np.floor(texel).astype(np.int64)
        high = np.minimum(low + 1, self.resolution - 1)
        fraction = texel - low.astype(np.float32)
        a = self.moments[indices, low[:, 0] + low[:, 1] * self.resolution]
        b = self.moments[indices, high[:, 0] + low[:, 1] * self.resolution]
        c = self.moments[indices, low[:, 0] + high[:, 1] * self.resolution]
        d = self.moments[indices, high[:, 0] + high[:, 1] * self.resolution]
        fx, fy = fraction[:, 0:1], fraction[:, 1:2]
        row0 = a * (F32(1.0) - fx) + b * fx
        row1 = c * (F32(1.0) - fx) + d * fx
        return row0 * (F32(1.0) - fy) + row1 * fy

    def diffuse(self, positions, normals, batch=16_384):
        """CPU port of probe_gi.wgsl, including clamped borders and dark/invalid probes."""
        positions = np.asarray(positions, dtype=np.float32)
        normals = np.asarray(normals, dtype=np.float32)
        if positions.ndim != 2 or positions.shape[1] != 3 or normals.shape != positions.shape:
            raise ValueError("positions and normals must have matching [sample, 3] shapes")
        if not np.isfinite(positions).all() or not np.isfinite(normals).all():
            raise ValueError("positions and normals must be finite")
        result = np.zeros_like(positions)
        for start in range(0, len(positions), batch):
            stop = min(start + batch, len(positions))
            result[start:stop] = self._diffuse_batch(positions[start:stop], normals[start:stop])
        return result

    def _diffuse_batch(self, positions, normals):
        query = (positions - self.origin) / self.spacing
        inside = ((query >= F32(-0.5)) & (query <= self.dimensions.astype(np.float32) - F32(0.5))).all(axis=1)
        grid = np.clip(query, F32(0.0), (self.dimensions - 1).astype(np.float32))
        low = np.floor(grid).astype(np.int64)
        high = np.minimum(low + 1, self.dimensions - 1)
        fraction = grid - low.astype(np.float32)
        biased = positions + normals * (self.spacing.min() * F32(0.03))
        color = np.zeros_like(positions)
        weight_sum = np.zeros(len(positions), dtype=np.float32)
        nx, ny, _ = self.dimensions
        x, y, z = normals[:, 0], normals[:, 1], normals[:, 2]
        c1 = F32(0.488602512) * (F32(2.0) / F32(3.0))
        c2 = F32(1.092548431) * F32(0.25)
        for cz, cy, cx in itertools.product((0, 1), repeat=3):
            corner = np.asarray((cx, cy, cz), dtype=bool)
            cells = np.where(corner, high, low)
            weights = np.where(corner, fraction, F32(1.0) - fraction)
            weight = weights[:, 0] * weights[:, 1] * weights[:, 2]
            index = cells[:, 0] + nx * (cells[:, 1] + ny * cells[:, 2])
            at = self.origin + cells.astype(np.float32) * self.spacing
            to = biased - at
            distance = np.sqrt(np.sum(to * to, axis=1, dtype=np.float32))
            direction = to / np.maximum(distance[:, None], F32(1e-6))
            oct_direction = direction / np.maximum(np.abs(direction).sum(axis=1)[:, None], F32(1e-6))
            p = oct_direction[:, :2].copy()
            folded = (F32(1.0) - np.abs(p[:, ::-1])) * np.where(p >= 0, F32(1.0), F32(-1.0))
            p = np.where(oct_direction[:, 2:3] < 0, folded, p)
            uv = p * F32(0.5) + F32(0.5)
            moments = self.sample_moments(index, uv)
            mean, second = moments[:, 0], moments[:, 1]
            variance = np.maximum(second - mean * mean, F32(1e-5))
            delta = distance - mean
            visibility = variance / (variance + delta * delta)
            weight *= np.where(distance > mean, visibility * visibility * visibility, F32(1.0))
            facing = np.maximum(F32(0.05), F32(0.5) + F32(0.5) * np.sum(normals * -direction, axis=1))
            weight *= facing * facing
            weight *= (second != 0) & inside
            sh = self.sh[index]
            diffuse = (
                sh[:, 0] * F32(0.282094792)
                + c1 * (sh[:, 1] * y[:, None] + sh[:, 2] * z[:, None] + sh[:, 3] * x[:, None])
                + c2 * (sh[:, 4] * (x * y)[:, None] + sh[:, 5] * (y * z)[:, None] + sh[:, 7] * (x * z)[:, None])
                + sh[:, 6] * (F32(0.315391565) * F32(0.25) * (F32(3.0) * z * z - F32(1.0)))[:, None]
                + sh[:, 8] * (F32(0.546274215) * F32(0.25) * (x * x - y * y))[:, None]
            )
            color += weight[:, None] * np.maximum(diffuse, F32(0.0))
            weight_sum += weight
        return color / np.maximum(weight_sum[:, None], F32(1e-6))


def load_teacher(path: pathlib.Path):
    with path.open("rb") as source:
        data = source.read(MAX_JSON_BYTES + 1)
    if len(data) > MAX_JSON_BYTES:
        raise ValueError(f"baked GI JSON exceeds {MAX_JSON_BYTES} bytes")
    def nonfinite(token):
        raise ValueError(f"non-finite JSON number {token}")
    raw = json.loads(data, object_pairs_hook=_object, parse_constant=nonfinite)
    _fields(raw, ("format", "origin", "spacing", "dimensions", "probes", "distance_resolution",
                  "max_distance", "rays_per_probe", "bounces", "scene_signature"), "baked GI")
    if raw["format"] != BAKED_FORMAT:
        raise ValueError(f"expected baked GI format {BAKED_FORMAT}")
    origin = _f32(raw["origin"], (3,), "origin")
    spacing = _f32(raw["spacing"], (3,), "spacing")
    if not (spacing > 0).all():
        raise ValueError("spacing must be positive")
    if not isinstance(raw["dimensions"], list) or len(raw["dimensions"]) != 3:
        raise ValueError("dimensions must have length 3")
    dimensions = np.asarray([_integer(v, 1, MAX_PROBES, "dimension") for v in raw["dimensions"]], dtype=np.int64)
    count = math.prod(dimensions.tolist())
    if count > MAX_PROBES or not isinstance(raw["probes"], list) or len(raw["probes"]) != count:
        raise ValueError(f"dimensions must match {count} probes, with maximum {MAX_PROBES}")
    resolution = _integer(raw["distance_resolution"], 1, MAX_DISTANCE_RESOLUTION, "distance_resolution")
    texels = resolution * resolution
    if count * texels > MAX_DISTANCE_MOMENTS:
        raise ValueError(f"distance moments exceed capacity {MAX_DISTANCE_MOMENTS}")
    distance = _f32(raw["max_distance"], (), "max_distance")
    with np.errstate(over="ignore"):
        maximum_squared = distance * distance
    if distance <= 0 or not np.isfinite(maximum_squared):
        raise ValueError("max_distance must be positive and its square finite")
    _integer(raw["rays_per_probe"], 1, 1_048_576, "rays_per_probe")
    _integer(raw["bounces"], 1, 16, "bounces")
    signature = raw["scene_signature"]
    if not isinstance(signature, str) or not 1 <= len(signature.encode("utf-8")) <= 256:
        raise ValueError("scene_signature must contain 1..256 UTF-8 bytes")
    sh = np.empty((count, 9, 3), dtype=np.float32)
    moments = np.empty((count, texels, 2), dtype=np.float32)
    for index, probe in enumerate(raw["probes"]):
        _fields(probe, ("radiance_sh", "distance_moments"), f"probe {index}")
        sh[index] = _f32(probe["radiance_sh"], (9, 3), f"probe {index} radiance_sh")
        moments[index] = _f32(probe["distance_moments"], (texels, 2), f"probe {index} distance_moments")
    mean, second = moments[..., 0], moments[..., 1]
    with np.errstate(over="ignore"):
        square = mean * mean
        tolerance = F32(2e-5) * np.maximum(np.maximum(second, square), F32(1.0))
    if ((mean < 0) | (second < 0) | (mean > distance)
        | (second > maximum_squared + tolerance) | (second + tolerance < square)).any():
        raise ValueError("invalid distance moments")
    teacher = Teacher(origin, spacing, dimensions, sh, moments, resolution,
                      hashlib.sha256(data).hexdigest(), signature)
    if not np.isfinite(teacher.input_min).all() or not np.isfinite(teacher.input_extent).all() or not (teacher.input_extent > 0).all():
        raise ValueError("half-cell input AABB must be finite and have positive extent")
    return teacher


class Network(torch.nn.Module):
    def __init__(self):
        super().__init__()
        self.layers = torch.nn.ModuleList((torch.nn.Linear(6, 32), torch.nn.Linear(32, 32), torch.nn.Linear(32, 3)))

    def forward(self, features):
        value = torch.relu(self.layers[0](features))
        value = torch.relu(self.layers[1](value))
        return self.layers[2](value)


def export_network(model, teacher):
    layers = [{"weights": layer.weight.detach().cpu().numpy().astype(np.float32).tolist(),
               "bias": layer.bias.detach().cpu().numpy().astype(np.float32).tolist()}
              for layer in model.layers]
    for layer in layers:
        if not np.isfinite(np.asarray(layer["weights"])).all() or not np.isfinite(np.asarray(layer["bias"])).all():
            raise ValueError("training produced non-finite network parameters")
    return {"format": NETWORK_FORMAT, "origin": teacher.origin.tolist(),
            "spacing": teacher.spacing.tolist(), "dimensions": teacher.dimensions.tolist(),
            "input_min": teacher.input_min.tolist(), "input_extent": teacher.input_extent.tolist(),
            "in": 6, "hidden": 32, "output": 3, "layers": layers}


def train(teacher, steps, samples, seed, device):
    _integer(steps, 1, MAX_STEPS, "steps")
    _integer(samples, 32, MAX_SAMPLES, "samples")
    _integer(seed, 0, 0xFFFFFFFF, "seed")
    if device not in ("cpu", "mps"):
        raise ValueError("device must be cpu or mps")
    if device == "mps" and not torch.backends.mps.is_available():
        raise ValueError("MPS was selected but is unavailable")
    start = time.perf_counter()
    generator = np.random.default_rng(seed)
    positions = teacher.input_min + generator.random((samples, 3), dtype=np.float32) * teacher.input_extent
    normals = generator.standard_normal((samples, 3), dtype=np.float32)
    normals /= np.maximum(np.linalg.norm(normals, axis=1)[:, None], F32(1e-6))
    features = teacher.features(positions, normals)
    targets = teacher.diffuse(positions, normals)
    if not np.isfinite(features).all() or not np.isfinite(targets).all():
        raise ValueError("teacher samples contain non-finite values")
    order = generator.permutation(samples)
    split = samples * 4 // 5
    train_ids, test_ids = order[:split], order[split:]
    x_train = torch.from_numpy(features[train_ids]).to(device)
    y_train = torch.from_numpy(targets[train_ids]).to(device)
    x_test = torch.from_numpy(features[test_ids]).to(device)
    y_test = torch.from_numpy(targets[test_ids]).to(device)
    torch.manual_seed(seed)
    if device == "mps":
        torch.mps.manual_seed(seed)
    model = Network().to(device=device, dtype=torch.float32)
    optimizer = torch.optim.Adam(model.parameters(), lr=0.002)
    sample_seconds = time.perf_counter() - start
    if device == "mps":
        torch.mps.synchronize()
    train_start = time.perf_counter()
    model.train()
    for _ in range(steps):
        ids = generator.integers(split, size=min(1024, split))
        batch = torch.from_numpy(ids).to(device)
        prediction = model(x_train[batch])
        loss = torch.nn.functional.mse_loss(prediction, y_train[batch])
        optimizer.zero_grad(set_to_none=True)
        loss.backward()
        optimizer.step()
    if device == "mps":
        torch.mps.synchronize()
    training_seconds = time.perf_counter() - train_start
    model.eval()
    with torch.no_grad():
        train_loss = torch.nn.functional.mse_loss(model(x_train), y_train).item()
        predictions = model(x_test).clamp(min=0)
        mse = torch.nn.functional.mse_loss(predictions, y_test).item()
        mae = torch.nn.functional.l1_loss(predictions, y_test).item()
        peak = y_test.max().item()
    if not all(math.isfinite(v) for v in (train_loss, mse, mae, peak)):
        raise ValueError("training or evaluation produced non-finite metrics")
    # Linear HDR RGB has no fixed display peak. State the measured held-out teacher peak used.
    psnr = 10 * math.log10(peak * peak / mse) if peak > 0 and mse > 0 else None
    report = {"teacher": "baked_diffuse probe field, not unbiased ground-truth paths",
              "teacher_sampling": "trilinear SH probes with bilinear octahedral distance moments",
              "teacher_code_sha256": hashlib.sha256(pathlib.Path(__file__).read_bytes()).hexdigest(),
              "dataset": "uniform random half-cell AABB positions and uniform sphere normals",
              "input_sha256": teacher.source_hash, "scene_signature": teacher.scene_signature,
              "seed": seed, "device": device, "dtype": "float32", "torch": torch.__version__,
              "steps": steps, "samples": samples, "train_samples": split,
              "heldout_samples": samples - split, "train_loss": train_loss,
              "heldout_mse": mse, "heldout_mae": mae, "heldout_psnr_db": psnr,
              "psnr_peak_linear_rgb": peak, "sample_seconds": sample_seconds,
              "training_seconds": training_seconds, "total_seconds": time.perf_counter() - start}
    return export_network(model, teacher), report


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--input", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("--steps", type=int, default=2000)
    parser.add_argument("--samples", type=int, default=65_536)
    parser.add_argument("--seed", type=int, default=7)
    parser.add_argument("--device", choices=("mps", "cpu"), default="mps" if torch.backends.mps.is_available() else "cpu")
    args = parser.parse_args()
    try:
        teacher = load_teacher(args.input)
        network, report = train(teacher, args.steps, args.samples, args.seed, args.device)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(network, separators=(",", ":"), allow_nan=False) + "\n")
        report["output"] = str(args.output)
        print(json.dumps(report, indent=2, allow_nan=False))
    except (ValueError, OSError) as error:
        parser.error(str(error))


if __name__ == "__main__":
    main()
