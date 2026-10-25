# GPU water: retained integration scope

Status: closed at the owner-selected GPU shallow-water foundation, 2026-10-21. The owner stopped
unfinished modernization work and requested shipping only verified implementation. Unadopted
3D fluid/rigid-pressure prototypes and the unfinished surfing/fall games are not included.

The retained implementation provides host-backed full-readback and resident GPU solvers,
reconstructible checkpoints, bounded replay programs, GPU state digests, checked asynchronous
native/browser ticks, sparse script/body observations, field buoyancy and horizontal feedback,
current-column swimming, fixed-collider beds, and measured inlet/outlet/Sea boundary controls.
Game-side crates remain headless; wgpu stays in host/presentation crates. GPU errors stop the
tick, and execution environments are checked rather than silently switching solver.

Field presentation consumes resident checkpoints/programs directly and retains numerical/image
checks for mesh bounds, LOD/morph, optics, camera-side medium, foam and dilute visual spray.
These are bounded presentation features. They do not establish final beach quality, dense
waterfalls, rolling breakers, displaced body volume or global fluid/body energy conservation.

`samples/water-pool` is a repeatable swimming validation game: actual player input, underwater
marker, resurfacing, dry-pad return and retry. Other water samples are numerical integration
fixtures, not completed surfing or waterfall games. Specifications are in water.md 17--45;
methods and concise measurements are in bench/liquids.md. Raw captures, measurements and logs
remain in ignored out/ or CI artifacts.

Amoris Pioneer, 2026-10-22: on the owner's request, `explore/water` carries the resident water past
this scope: displaced body volume, implicit buoyancy and Froude-Krylov forces, ballistic falls,
breaking, dispersive surface waves, boards, entry crowns and underwater caustics, and on
`explore/water-nh` non-hydrostatic pressure, added mass, water entry and body coupling that no
longer pumps energy into the water (water.md 46 to 56; bench/liquids.md 7). Its
`samples/pool-party`, `samples/river-falls` and `samples/surf-beach` are completed games whose
checks play them through the player's controls.
