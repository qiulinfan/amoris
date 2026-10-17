# Ray-traced sun shadows: quality and measured cost

Cascaded shadows remain the default. `AMORIS_RT_SHADOWS=1` enables ray-traced sun shadows when
the device exposes ray queries; unsupported devices keep cascades. The choice is opt-in because
scene size, traversal and acceleration-structure maintenance can outweigh the saved cascade work.

Design/API: [ray-traced shadows](../spec/rt-shadows.md),
[path tracing](../spec/path-tracing-nrc.md), [architecture](../spec/architecture.md).
Conditions: [quiet measurements](quiet-2026-10-09.md).

## Rendering contract

The ray-traced path replaces the sun's cascade lookup in forward shading. Static bottom-level
geometry is retained; skinned bottom levels and the top-level instance structure follow the pose
actually drawn, including interpolation. Masked casters apply their alpha test to candidates.
Full meshes are used for ray-traced shadows: coarse raster LOD receivers otherwise disagree with
the ray geometry. Rays have no cascade-distance cutoff.

Acceleration-structure builds occur outside timed render passes. GPU pass totals therefore omit
that work, while submit-to-idle wall time includes builds, validation and CPU encoding. A lower
pass total alone does not establish a faster frame.

## Correctness and image differences

The native check follows static, moved, interpolated and skinned states. It compares the pixels
each path darkens by more than 20% against the same state without shadows. The reported RTX 5060
Vulkan values below were also checked on the other Windows GPU/API configurations to a few pixels.

| State | Shadowed pixels, cascaded / ray traced | Overlap (IoU) | Changed since the last state, cascaded / ray traced | Overlap of the changes |
|---|---:|---:|---:|---:|
| mixed scene, static | 2,284 / 2,386 | 0.935 | | |
| every cell moved 1 m, halfway through the tick | 2,301 / 2,377 | 0.939 | 3,119 / 3,263 | 0.910 |
| moved, end of the tick | 2,308 / 2,389 | 0.935 | 3,101 / 3,230 | 0.909 |
| three skinned characters added | 4,844 / 5,207 | 0.914 | 3,096 / 3,386 | 0.894 |
| their arms raised by the skinning | 4,993 / 5,336 | 0.916 | 1,327 / 1,383 | 0.933 |

Required overlap is above 0.9, or 0.85 with thin skinned limbs; overlap of changes is above 0.8.
Masked cut-out light remains within a factor of two between paths. Ray-traced edges are harder
than the cascades' 3x3 filter. At 1920x1080, 0.64% of the mixed-scene pixels differ by more than
32 levels of 255. Five skinned characters captured through the app differ by 0.23% at that limit.

The far side of the 200,000-cube sphere shades inner faces beyond the cascade distance. Its larger
image difference is an intentional distance-limit distinction. Same-state cross-API ray-traced
images agree apart from small edge differences.

## Machine and final method

Windows R1, 2026-10-10: Ryzen 9 270 (16 threads), 15 GiB, RTX 5060 Laptop 8 GB and Radeon 780M,
AC/Balanced, wgpu 30.0.1. Drivers: NVIDIA 617.14, AMD 32.0.13062.3005 / Vulkan 24.30.62.03.
No concurrent builds or benchmark agents; desktop background load remains.

Every configuration ran in five rounds. Backend order and cascaded/ray-traced order alternate;
each path settles five seconds after initialization, and each round starts with NVIDIA at 70 C
or below. Resolution is 1920x1080, with 30 warm-up frames and 240 measured frames (60 for moving
200,000 cubes). Cells are GPU p50, median of five rounds; parentheses show spread. Changes use
the medians, followed by the observed round range.

## Final results

|  Scene  |  GPU, backend  |  Cascaded  |  Ray traced  |  Change (rounds)  |
| --- | --- | ---: | ---: | ---: |
|  mixed  |  RTX 5060, Vulkan  |  0.513 (0%)  |  0.527 (1%)  |  +3% (+3 to +4)  |
|  mixed  |  RTX 5060, Direct3D 12  |  0.577 (0%)  |  0.524 (1%)  |  -9% (-10 to -9)  |
|  mixed  |  Radeon 780M, Vulkan  |  2.274 (2%)  |  1.713 (1%)  |  -25% (-25 to -23)  |
|  mixed  |  Radeon 780M, Direct3D 12  |  2.579 (7%)  |  2.095 (4%)  |  -19% (-22 to -17)  |
|  mixed, every instance moving  |  RTX 5060, Vulkan  |  0.509 (0%)  |  0.523 (1%)  |  +3% (+2 to +4)  |
|  mixed, every instance moving  |  RTX 5060, Direct3D 12  |  0.572 (1%)  |  0.515 (2%)  |  -10% (-11 to -9)  |
|  mixed + 5 skinned characters  |  RTX 5060, Vulkan  |  0.532 (0%)  |  0.555 (0%)  |  +4% (+4)  |
|  mixed + 5 skinned characters  |  RTX 5060, Direct3D 12  |  0.600 (1%)  |  0.549 (1%)  |  -8% (-9 to -8)  |
|  mixed + 20 skinned characters  |  RTX 5060, Vulkan  |  0.571 (1%)  |  0.608 (0%)  |  +6% (+5 to +7)  |
|  mixed + 20 skinned characters  |  RTX 5060, Direct3D 12  |  0.649 (1%)  |  0.600 (1%)  |  -8% (-8 to -7)  |
|  many_cubes, 10,000  |  RTX 5060, Vulkan  |  0.502 (1%)  |  0.613 (0%)  |  +22% (+22 to +23)  |
|  many_cubes, 10,000  |  RTX 5060, Direct3D 12  |  0.546 (0%)  |  0.626 (0%)  |  +15% (+14 to +15)  |
|  many_cubes, 200,000  |  RTX 5060, Vulkan  |  0.759 (5%)  |  1.111 (4%)  |  +46% (+40 to +49)  |
|  many_cubes, 200,000  |  RTX 5060, Direct3D 12  |  0.811 (1%)  |  1.162 (4%)  |  +43% (+38 to +44)  |
|  many_cubes, 200,000 moving (idle GPU; the correction below)  |  RTX 5060, Vulkan  |  12.313 (2%)  |  14.521 (0%)  |  +18% (+17 to +20)  |
|  many_cubes, 200,000 moving, fixed `apply` (2026-10-15, three rounds)  |  RTX 5060, Direct3D 12  |  0.853 (1%)  |  1.202 (1%)  |  +41% (+40 to +41)  |
|  many_cubes, 200,000 moving, fixed `apply` (2026-10-15, three rounds)  |  RTX 5060, Vulkan  |  6.125 (1%)  |  7.348 (86%)  |  low-power state, no ratio  |

Small mixed scenes cost 3–6% more ray traced on RTX 5060 Vulkan, 8–10% less on RTX D3D12, and
19–25% less on the 780M. With 10,000 or 200,000 cubes, the RTX's ray-traced passes instead cost
15–46% more. The moving 200,000-cube case cost 18% more with Vulkan on the 2026-10-10 build, a
measurement of an idle GPU (the correction below). The default remains off.

Correction (2026-10-14, quiet re-measurement 2026-10-15; [dynamic scenes](dynamic-scenes.md) 5.5):
the moving 200,000-cube row of 2026-10-10 does not measure the shadow passes. Its frames took about
a second of CPU (`Scene::apply` searched a list for every moving slot, n² comparisons), and the GPU
idled between them. On an idle machine (three rounds, the cascaded path run last) the build before
the fix takes 12.1 to 12.6 ms of cascaded and 14.1 to 14.5 ms of ray-traced GPU time, the RTX's
graphics clock at a median of 180 MHz over the cascaded frames (nvidia-smi, 100 ms samples), and a
control reproduces that from the idle alone: the fixed build, made to busy-wait 900 ms before every
frame (`--stall 900`), takes 12.1 to 12.7 and 14.1 ms again at 180 MHz. Without the wait the fixed
build measures the passes on Direct3D 12: 0.853 ms cascaded (1,755 MHz, P2) against 1.202 ray
traced, 41% more, as with the 200,000 still cubes. Under Vulkan it does not: the RTX stays in P5 and
P8 (memory clock 810 MHz) over the cascaded path's frames of 41 ms in all three rounds, which take
6.1 ms of GPU time, and the ray-traced path took 7.35 to 7.51 ms in two rounds and 1.23 in one, so
this case gives no Vulkan ratio; the provisional measurement's Vulkan spread (0.86 to 6.4 ms) was
not the shared machine's doing. The ray-traced path spends 39 to 49 ms of render CPU a frame (59 to
60 with the stall or before the fix), 32 to 42 ms more than the cascaded one; no trace has split
that time yet (the top level's rebuild and wgpu's per-instance validation are the candidates).

## Limits that affect the decision

- At 200,000 instances, wgpu-core validates a bottom-level dependency per instance each submission:
  about 2.8 ms CPU even for static geometry. Fewer/larger acceleration-structure instances may
  matter more than the ray shader itself.
- Skinned geometry requires per-frame bottom/top-level rebuilds. With 20 characters, frame wall
  time grows by about 0.1–0.35 ms on the measured RTX workload although pass totals move little.
- The moving 200,000-instance example spent about one second updating poses in either path (a
  quadratic search in `Scene::apply`, fixed on 2026-10-14; [dynamic scenes](dynamic-scenes.md)),
  which also left the GPU idle between frames; its row above is not a measurement of the passes
  (the correction above). The ray-traced path's own render CPU is now most of its frame.
- The small-scene Vulkan costs depend on the forward-shader/encoding build. These final figures
  cannot be mixed with timings from another build or applied to a different scene without a check.
- Apple comparative timings are not established by this Windows dataset. Feature availability,
  correctness and performance remain separate questions on each backend.

## Reproduce

```sh
cargo build --release -p amoris-render --examples
cargo build --release -p amoris-app
python tools/rt_shadows_bench.py --adapters 5060,780m --backends vulkan,dx12 \
  --rounds 5 --all-rounds --alternate --settle 5 --cool 70 \
  --out out/rt-shadows --summary out/bench-runs/rt-shadows.json
python tools/quiet_tables.py rts out/bench-runs/rt-shadows.json
cargo test --release -p amoris-render --test rt_shadows
```

Keep captures and detailed receipts in ignored `out/`. The [fixed historical
report](https://github.com/qiulinfan/amoris-benchmarks-results/tree/main/sources/pioneer-20261010/docs/evidence/quiet/rt)
preserves the original measurements and controls.
