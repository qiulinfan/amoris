# M2 evidence: graphics

Produced on 2026-09-18 with the debug (sanitized) build, headless, on Apple M5 through wgpu-native (Metal).

| File | Command | What it shows |
|---|---|---|
| `playground-240.png` | `pocket_runtime --project samples/playground --bundle build/ts/playground.js --headless --frames 240 --size 640x360 --capture ...` | The playground scene at tick 240: ground cube, player cube with a point light above it, enemy spheres approaching, a directional sun, the scene camera from `scene.json`. |
| `playground-240-ids.png` | `render.ids` on the same frame | The entity id buffer rendered alongside color: every pixel names its entity, so `render.pick(x, y)` answers "what is this" without vision. |
| `playground-240.json` | same run, `--json` | The report with `render` stats (draw calls, lights, camera) and the capture's `ids.visible` list (entities on screen with pixel counts). |
| `hello-60.png` | `pocket_runtime --project samples/hello ... --frames 60` | The M0 sample now draws its bouncing ball as a sphere; the exposed numbers and the picture come from the same state. |

`renderer_tests` (Catch2) checks: camera and lights are picked up from the world, at least the ground and player are visible in the id buffer, projecting the player's world position and picking that pixel returns the player, the capture differs from the clear color, and `Bounds` follows transforms. The hello golden hash was regenerated because hello now spawns entities (its exposed state is unchanged).
