"""Analytical shader-teacher checks and a small CPU export smoke test; no MPS workload."""

import copy
import json
import math
import pathlib
import tempfile
import unittest

import numpy as np

from train import load_teacher, train


def fixture():
    sh = [[0.0] * 3 for _ in range(9)]
    sh[0] = [1.0 / 0.282094792] * 3
    return {"format": "amoris-baked-gi-v1", "origin": [0.0] * 3, "spacing": [1.0] * 3,
            "dimensions": [1, 1, 1], "distance_resolution": 1, "max_distance": 10.0,
            "rays_per_probe": 32, "bounces": 1, "scene_signature": "test",
            "probes": [{"radiance_sh": sh, "distance_moments": [[10.0, 100.0]]}]}


class TeacherTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = pathlib.Path(self.temp.name) / "probe.json"

    def load(self, raw):
        self.path.write_text(json.dumps(raw))
        return load_teacher(self.path)

    def test_constant_field_half_cell_bounds_and_invalid_probe(self):
        teacher = self.load(fixture())
        points = np.asarray([[0, 0, 0], [0.5, 0, 0], [0.5001, 0, 0]], dtype=np.float32)
        normals = np.asarray([[0, 1, 0]] * 3, dtype=np.float32)
        np.testing.assert_allclose(teacher.diffuse(points, normals), [[1, 1, 1], [1, 1, 1], [0, 0, 0]], atol=1e-6)
        teacher.moments[:] = 0
        np.testing.assert_array_equal(teacher.diffuse(points, normals), np.zeros((3, 3)))

    def test_signed_directional_sh_is_lambert_convolved_and_clamped(self):
        raw = fixture()
        raw["probes"][0]["radiance_sh"] = [[0.0] * 3 for _ in range(9)]
        raw["probes"][0]["radiance_sh"][3] = [1.0, 2.0, 3.0]
        teacher = self.load(raw)
        result = teacher.diffuse([[0, 0, 0]] * 2, [[1, 0, 0], [-1, 0, 0]])
        expected = np.asarray([[1, 2, 3], [0, 0, 0]], dtype=np.float32) * np.float32(0.488602512 * 2 / 3)
        np.testing.assert_allclose(result, expected, atol=1e-6)

    def test_trilinear_mix_and_single_probe_axes(self):
        raw = fixture()
        raw["dimensions"] = [2, 1, 1]
        raw["spacing"] = [2.0, 1.0, 1.0]
        raw["probes"].append(copy.deepcopy(raw["probes"][0]))
        raw["probes"][0]["radiance_sh"][0] = [1 / 0.282094792, 0, 0]
        raw["probes"][1]["radiance_sh"][0] = [0, 0, 1 / 0.282094792]
        teacher = self.load(raw)
        np.testing.assert_allclose(teacher.diffuse([[1, 0, 0]], [[0, 1, 0]]), [[0.5, 0, 0.5]], atol=1e-6)
        np.testing.assert_array_equal(teacher.input_min, [-1, -0.5, -0.5])
        np.testing.assert_array_equal(teacher.input_extent, [4, 1, 1])
        np.testing.assert_array_equal(teacher.features(np.asarray([[-1, -0.5, -0.5]], dtype=np.float32),
                                                     np.asarray([[0, 1, 0]], dtype=np.float32)), [[-1, -1, -1, 0, 1, 0]])

    def test_occlusion_cubed_preserves_dark_regions(self):
        raw = fixture()
        raw["probes"][0]["distance_moments"] = [[0.1, 0.01001]]
        teacher = self.load(raw)
        result = teacher.diffuse([[0, 0.4, 0]], [[0, 1, 0]])
        delta = np.float32(0.4 + 0.03) - np.float32(0.1)
        variance = max(np.float32(0.01001) - np.float32(0.1) ** 2, np.float32(1e-5))
        visibility = variance / (variance + delta * delta)
        weight = visibility ** 3 * np.float32(0.05) ** 2
        expected = weight / max(weight, np.float32(1e-6))
        np.testing.assert_allclose(result, np.full((1, 3), expected), rtol=2e-5)
        self.assertLess(result.max(), 1e-6)

    def test_octahedral_moments_are_bilinear_at_texel_centers_and_clamped(self):
        raw = fixture()
        raw["distance_resolution"] = 2
        raw["probes"][0]["distance_moments"] = [[1, 1], [2, 4], [3, 9], [4, 16]]
        teacher = self.load(raw)
        uv = np.asarray([[0.5, 0.5], [0, 0], [1, 1], [0.25, 0.75], [0.375, 0.625]], dtype=np.float32)
        sampled = teacher.sample_moments(np.zeros(5, dtype=np.int64), uv)
        np.testing.assert_array_equal(sampled, [[2.5, 7.5], [1, 1], [4, 16], [3, 9], [2.75, 8.5]])

    def test_strict_input_rejects_unknown_bool_and_nonfinite_values(self):
        for field, value in (("ambient", 1.0), ("spacing", [True, 1, 1]), ("origin", [math.inf, 0, 0]),
                             ("dimensions", [65_537, 1, 1])):
            raw = fixture()
            raw[field] = value
            with self.assertRaises(ValueError):
                self.load(raw)
        raw = fixture()
        raw["probes"][0]["distance_moments"] = [[2.0, 3.0]]
        with self.assertRaisesRegex(ValueError, "distance moments"):
            self.load(raw)

    def test_cpu_training_exports_finite_fixed_shape_network_and_heldout_report(self):
        teacher = self.load(fixture())
        network, report = train(teacher, steps=4, samples=64, seed=7, device="cpu")
        self.assertEqual(network["format"], "amoris-neural-gi-v1")
        self.assertEqual((network["in"], network["hidden"], network["output"]), (6, 32, 3))
        for layer, shape in zip(network["layers"], ((32, 6), (32, 32), (3, 32)), strict=True):
            weights = np.asarray(layer["weights"])
            self.assertEqual(weights.shape, shape)
            self.assertEqual(len(layer["bias"]), shape[0])
            self.assertTrue(np.isfinite(weights).all())
        self.assertEqual(report["train_samples"] + report["heldout_samples"], 64)
        self.assertGreater(report["heldout_samples"], 0)
        self.assertEqual(report["device"], "cpu")
        self.assertTrue(math.isfinite(report["heldout_mse"]))
        self.assertIn("not unbiased ground-truth", report["teacher"])


if __name__ == "__main__":
    unittest.main()
