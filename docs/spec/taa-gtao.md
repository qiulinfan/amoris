# TAA and GTAO

Status: Draft, in progress on branch `explore/aa` (2026-10-09)

Charter: 4.4 ("HDR, bloom, TAA, AgX"; the Pioneer note of 2026-10-09 fixes where TAA and GTAO sit
in the frame and that the defaults are measured), 4.1 (one renderer for Metal, Vulkan, Direct3D 12
and WebGPU).

This specification says how the renderer anti-aliases over time (TAA) and how it darkens indirect
light with ground-truth ambient occlusion (GTAO), where both sit in the frame, how they interact
with multisampling, splats, particles, bloom, the entity-id pass and the editor's overlays, and how
they are switched. The schedule (3.3) listed a branch `feat/gtao` with measurements on an M5; that
branch was lost before it was merged. This is a new implementation.

## 1. The frame

1. Culling, lights, shadows, splat preparation as before.
2. **Opaque pass** into the scene targets (multisampled or not): color, and when the effects need
   them two more targets the forward shader writes: the share of the pixel's color that is indirect
   light (GTAO) and the object motion (TAA). With TAA the projection carries this frame's sub-pixel
   jitter.
3. **GTAO** at half resolution from the depth, then a depth-aware upsample that multiplies the
   indirect share of each pixel by the visibility.
4. **TAA**: the current image and the reprojected history give the new history and the image the
   rest of the frame uses.
5. **Splats** over that image, with the camera's unjittered matrices.
6. Bloom, the display transform (and an optional sharpening), the editor's overlays and the game UI,
   all unjittered.

The entity-id pass draws with the unjittered matrices.

## 2. Switches

`POCKET_AA` (`msaa`, `taa`, `msaa+taa`, `off`) and `POCKET_GTAO` (`on`, `off`) set the starting
options; `Renderer::set_antialiasing` and `Renderer::set_gtao` change them at run time. The defaults
are measured (charter 4.4) and recorded in docs/bench/taa-gtao.md.
