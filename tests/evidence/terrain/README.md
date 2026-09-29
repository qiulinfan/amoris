# Terrain evidence

- `hills.png` (2026-09-29): `samples/hills` at 1280 by 720 after 30 ticks: noise hills 96 units across (seed 11, scale 36, five octaves, 12 high) meshed by the engine, grass on the gentle ground, rock on the slopes and snow over the tops blended from height and slope, the lake, the trees the script planted where `terrain.height` found gentle grass above the water (with their shadows), and the character standing on the shore.
- `editor-sculpt.png` (2026-09-29): `samples/hills` in the editor with the Hills selected and Sculpt on (the handles out of the way): three drags across the pane raised the ground where the camera's ray met it (revision 40), the whole valley under the scene's camera with its lakes, snow and rock.
- `scatter.png` (2026-09-29): the same view with the sample's two Scatters: 931 bushes on the gentle grass between the water and the snow and 362 stones leaning with the slopes (the rock too), 1293 copies drawn in 6 draw calls with their shadows.

Reproduce: `./.pocket/pocket run hills -- --headless --frames 30 --json --capture hills.png --size 1280x720`, `./.pocket/pocket scenario hills`, `assets_tests "[terrain]"`, `runtime_tests "[terrain]"`.
