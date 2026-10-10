# Native sunlit town exploration

2026-10-05, `feature/metal`, Apple M5. The standalone macOS game bundles the native runtime,
the complete ORCA Bistro exterior and baked diffuse GI. No editor, Node, Bun or TypeScript
checker is required at playback. The original asset license and attribution travel with it.

The derived model keeps all geometry, materials and node transforms, caps embedded textures at
1024 pixels and reconstructs positive Z for 132 legacy RG-only normal maps. Source URLs,
licenses and conversion parameters are retained with the model.

## Walking

Click captures the mouse; WASD moves, Shift runs, Space jumps, R resets, and Escape releases
then exits. Focus loss releases the mouse and clears input. Rapier's existing kinematic
character controller sweeps a 1.8 m capsule, handles 25 cm steps, slopes, gravity and wall sliding.
The camera is derived presentation state, not a new authoritative ECS player or replay format.

Geometry-only extraction avoids decoding textures twice. Navigation uses 2,543,838 opaque,
unskinned triangles; mask foliage and blend glass are excluded. A real-model CPU smoke placed
the eye at (12,2.1,18), settled it to approximately (11.9998,1.9903,18.0002), then moved 5.914 m
in two seconds along the street with all 120 movement steps grounded. Floor, wall, diagonal,
step and jump cases also pass focused controller tests.

## Normal-map transport defect

The earlier DDS conversion flipped G but left BC5 normals in RG with B=0. All 132 Bistro normal
maps were affected. Treating these as ordinary RGB normals points them through the surface:
one road sample had `normal.y=-.986` and `normal dot sun=-.702`. The forward renderer and CPU
baker accepted them, suppressing sunlight and launching diffuse bounce rays into the geometry.
The full Metal PT already rejected mapped normals outside geometric/view hemispheres, explaining
why its static previews looked properly lit while the native walking view did not.

The source converter now recognizes BC5 DDS headers, retains its one G flip and reconstructs
`Z=sqrt(max(0,1-X*X-Y*Y))`. The town converter repairs only normal-only images whose source B
is uniformly zero, preserves RG/alpha, rejects conflicting image roles and records the change.
Forward and CPU baking also reject inward mapped normals, matching the PT's hemisphere guards.
Positive normal maps retain their authored detail. The CPU regression proves sunlight and an
escaping secondary bounce survive a malformed negative-Z map; full forward WGSL validation passes.

## Re-bake and real on/off verification

The repaired bake uses 43,725 probes, origin (-65,.75,-90), spacing (3.5,3,3.5), dimensions
(53,15,55), 512 rays/probe, eight diffuse bounces, 8x8 distance moments and a 40 m horizon.
The lowest probes sit above the main street instead of at the old underground y=-1 layer.
The renderer's own atmosphere is exported with Sun 12, ambient .7 and exposure +1.25 EV.
The diffuse bake omits 13 transparent primitives (68 valid triangles); the visible model retains
them. This remains static mesh diffuse GI; the standalone full PT is a separate rendering mode.

The completed field traced 71,845,047 rays in 182.55 s, plus 27.55 s loading, during an ordinary
development session. These are workflow timings, not an isolated benchmark.

`showcase_bench --gi-compare DIRECTORY` reuses the same loaded model and field, locks the camera,
light, exposure and capture time, settles 32 frames per mode and captures on/off/on-repeat.
The actual M5 test reports data resident, no GI error and no pending assets. Final on/off mean
absolute difference is 17.97 sRGB8 bytes; 87.19% of pixels differ by more than two bytes.
On versus on-repeat is exactly equal. G in the native game toggles the same GI intensity without
changing the camera or re-importing the model; the HUD reads real GPU-data readiness.

Validation: 439 workspace tests pass, seven default opt-in tests are ignored. The large-model
walking check was separately run; actual native rendering and the Metal GI comparison were
executed. The browser viewport was rebuilt from the repaired shader. Source captures, JSON
reports, old defect controls and package receipts remain under ignored `out/town-walk/`.

Entry points and scope are in `samples/town-walk/AGENTS.md`. Large model and probe payloads stay
outside Git; the standalone local package includes both.
