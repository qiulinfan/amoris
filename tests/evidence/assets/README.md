# Assets evidence (2026-09-18)

`assets-sample.png`: `pocket run assets -- --headless --frames 20 --size 960x540 --capture ...`. From left to right: the glTF crate (`assets/crate.glb`, two baked nodes sharing one material with the checker texture; the red corner marks the texture origin on each face), the magenta cube standing in for `assets/does-not-exist.glb` (listed in `render.stats.assets.missing` and warned once in the log), and the pyramid from `assets/pyramid.gltf` (embedded buffer, flat normals built by the reader). The ground is a primitive plane with `texture: assets/checker.png`.

The run's log shows what an agent sees without pixels:

```
[info] script: assets in the project {"files":["assets/checker.png (image, 193 B)","assets/crate.glb (mesh, 2124 B)","assets/pyramid.gltf (mesh, 888 B)"]}
[info] assets: loaded mesh assets/crate.glb (48 vertices, 24 triangles, 1 materials)
[info] assets: loaded image assets/checker.png (64x64)
[info] assets: loaded mesh assets/pyramid.gltf (5 vertices, 6 triangles, 1 materials)
[warn] renderer: asset assets/does-not-exist.glb: assets/does-not-exist.glb does not exist (looked in .../samples/assets/assets/does-not-exist.glb)
```

and the report's `render` block: `{"draw_calls": 5, "meshes": 4, "assets": {"meshes": 2, "textures": 1, "missing": ["assets/does-not-exist.glb"]}}`.

Reproduce: `pocket test --filter assets`, `pocket test --filter renderer`, `python3 tools/scripts/make_sample_assets.py` (regenerates the binary files byte for byte).

`voxels.png` (`tools/scripts/voxel_evidence.py`, 2026-10-01): `samples/assets/assets/tree.voxels`, a tree written as nine text layers of a seven-letter palette (trunk, two greens, red apples a little glossy, a glowing firefly), shown three times on a lawn and turned 0, 45 and 135 degrees, at 960x540 in release. It meshes into 226 triangles in three materials (the plain cells, the apples, the firefly).

`obj-import-findings.png` (2026-10-02; `docs/research/2026-10-02-rendering-and-import-assessment.md`, Industrial OBJ import): what the OBJ reader gets wrong on files industrial software exports, each pair the engine (left, release runtime) beside Blender 4.5's own import (right, Workbench): concave n-gons fanned from their first corner (`ngons.obj`), a welded part without `vn` smoothed per position (`machined_near.obj`), the same part 4.5 km from the origin losing precision at parse (`machined_far.obj`, right of the third row), and a welded cube whose `s off` is ignored (`cube_soff.obj`). Made with `tools/scripts/dev/objstress/generate.py`, then `measure.py ngons`, `machined_near`, `machined_far`, `cube_soff` (the engine's captures) and `blender_render.py` with the same cameras (Blender's), stitched with Pillow.

`obj-import-fixed.png` (2026-10-02, Windows 11, release runtime on Direct3D 12): the same stress files after the OBJ reader's fixes (`docs/design/assets.md`, Importing models and Import settings), each row the engine before the change (left) beside after it (right) with the same camera: the concave n-gons ear-clipped (`ngons.obj`, all eight polygons right), the welded part without `vn` given crease-angle normals and read with `up: "z"`, `unit: "mm"` (`machined_2m.obj`: a square tower, drilled holes, sharp gear teeth; recentred in doubles, it lands exactly where the old case's hand-made Transform put it), the cube whose `s off` now shades it flat (`cube_soff.obj`); the last row is `machined_near.obj` at the origin beside `machined_far.obj` 4.5 km out, both read with `recenter: true`, which capture to identical pixels. Made with `tools/scripts/dev/objstress/generate.py`, `measure.py ngons`, `machined`, `cube_soff` with the runtime before the change (captures copied to `build/objstress/before/`), the same and `machined_near`, `machined_far` after it, then `tools/scripts/dev/objstress/evidence.py`.
