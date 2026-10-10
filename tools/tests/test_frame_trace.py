"""The profiler tools on fixed inputs: no GPU, RenderDoc or engine process is started.

    python -m unittest tools/tests/test_frame_trace.py
"""
import importlib.util
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parents[1]


def load(name, relative):
    spec = importlib.util.spec_from_file_location(name, TOOLS / relative)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


frame_trace = load("frame_trace", "frame_trace.py")
renderdoc_frame = load("renderdoc_frame", "renderdoc_frame.py")


def x(name, pid, ts, dur, frame, **args):
    return {"name": name, "ph": "X", "pid": pid, "tid": 1, "ts": ts, "dur": dur,
            "args": {"frame": frame, **args}}


def trace(opaque_ms, interval_ms=10.0, spike_at=None):
    """Frames every `interval_ms` (one twice as long at `spike_at`), each submitting at 1 ms and
    its GPU frame starting 0.5 ms later with two shadow passes and an opaque pass."""
    events, t = [], 0.0
    for f, opaque in enumerate(opaque_ms):
        length = interval_ms * (2 if f == spike_at else 1)
        events.append(x("frame", 1, t, length * 1000, f))
        events.append(x("submit", 1, t + 500, 500, f))
        g = t + 1500
        events.append(x("GPU frame", 2, g, (0.2 + opaque) * 1000, f, busy_ms=0.2 + opaque))
        events.append(x("shadows", 2, g, 100, f))
        events.append(x("shadows", 2, g + 100, 100, f))
        events.append(x("opaque+sky", 2, g + 200, opaque * 1000, f))
        t += length * 1000
    return {"traceEvents": events, "otherData": {"backend": "test", "frames": len(opaque_ms)}}


class FrameTraceTests(unittest.TestCase):
    def test_stats_sum_repeated_passes_and_find_spikes(self):
        s = frame_trace.stats(trace([4.0, 5.0, 6.0, 5.0], spike_at=1))
        gpu, frame = s["rows"]["gpu"], s["rows"]["frame"]
        self.assertEqual(gpu["opaque+sky"]["median"], 5.0)
        self.assertEqual(gpu["shadows"]["median"], 0.2)  # two 0.1 ms cascades per frame
        self.assertEqual(frame["submit to GPU start"]["median"], 0.5)
        self.assertEqual(frame["frame interval"]["max"], 20.0)
        self.assertEqual([x["frame"] for x in s["spikes"]], [2])
        # Idle gaps 5.8 (frame 0's GPU work ends at 5.7 ms, frame 1's starts at 11.5), 14.8, 3.8.
        idle = frame["GPU idle before frame"]
        self.assertAlmostEqual(idle["median"], 5.8, places=6)
        self.assertAlmostEqual(idle["min"], 3.8, places=6)

    def test_diff_gives_delta_and_ratio_per_row(self):
        a = frame_trace.stats(trace([4.0, 4.0, 4.0]))
        b = frame_trace.stats(trace([5.0, 5.0, 5.0]))
        d = frame_trace.diff(a, b)["gpu"]["opaque+sky"]
        self.assertEqual((d["a"], d["b"], d["delta"], d["ratio"]), (4.0, 5.0, 1.0, 1.25))


class ShaderStatsTests(unittest.TestCase):
    # One loop with two back edges to its header (the second a `continue`), one sample, and the
    # module's declaration of the sample operation after the function, as DXC writes it.
    DXIL = """define void @fs() {
  %1 = alloca [4 x float], align 4
  %2 = alloca [32 x float], align 4
  br label %3

; <label>:3                                       ; preds = %8, %3, %0
  %4 = phi i32 [ 0, %0 ], [ %5, %3 ], [ %5, %8 ]
  %5 = add i32 %4, 1
  %6 = call %dx.types.ResRet.f32 @dx.op.sampleGrad.f32(i32 63, i32 0)
  %7 = icmp ult i32 %5, 4
  br i1 %7, label %3, label %8

; <label>:8                                       ; preds = %3
  %9 = icmp ult i32 %5, 8
  br i1 %9, label %3, label %10

; <label>:10                                      ; preds = %8
  ret void
}

; Function Attrs: nounwind readonly
declare %dx.types.ResRet.f32 @dx.op.sampleGrad.f32(i32, %dx.types.Handle, %dx.types.Handle, \
float, float, float, float, i32, i32, i32, float, float, float, float, float, float, float) #1
"""
    SPIRV = """%_ptr_Function__arr_float_uint_32 = OpTypePointer Function %_arr_float_uint_32
        %10 = OpVariable %_ptr_Function__arr_float_uint_32 Function
        %11 = OpVariable %_ptr_Function__arr_v4float_uint_2 Function
               OpLoopMerge %20 %21 None
               OpBranchConditional %22 %23 %20
         %24 = OpImageSampleImplicitLod %v4float %25 %26
"""

    def test_dxil_counts_loop_headers_calls_and_local_arrays(self):
        s = renderdoc_frame.stats(self.DXIL, "dxil")
        self.assertEqual((s["loops"], s["samples"], s["conditional_branches"]), (1, 1, 2))
        self.assertEqual((s["dx_ops"], s["dx_op_kinds"]), (1, {"sampleGrad": 1}))
        self.assertEqual((s["local_arrays"], s["local_array_bytes"]), (2, 144))

    def test_results_keep_paths_relative_to_the_checkout(self):
        root = renderdoc_frame.ROOT
        r = renderdoc_frame.relative({"capture": str(root / "out" / "a.rdc"),
                                      "shaders": [str(root / "out" / "a-ps.dxil"), "DXIL"],
                                      "elsewhere": str(root.parent / "other" / "b.rdc")})
        self.assertEqual(r["capture"], "out/a.rdc")
        self.assertEqual(r["shaders"], ["out/a-ps.dxil", "DXIL"])
        self.assertTrue(Path(r["elsewhere"]).is_absolute())

    def test_spirv_counts_loops_samples_and_local_arrays(self):
        s = renderdoc_frame.stats(self.SPIRV, "spir-v")
        self.assertEqual((s["loops"], s["samples"], s["conditional_branches"]), (1, 1, 1))
        self.assertEqual((s["local_arrays"], s["local_array_bytes"]), (2, 160))


if __name__ == "__main__":
    unittest.main()
