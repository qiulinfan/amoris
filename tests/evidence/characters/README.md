# Character evidence

- `walker.png` (2026-09-29): `samples/walker` after 120 headless frames at 960 by 540: the capsule character (the built-in `capsule` mesh) in the yard, the stairs to the west deck, the 20 degree ramp to the east deck, the 55 degree rock at the left, the lift up beside the tower, two crates and the three coins.
- `hero-poses.png` (2026-10-01): the walker's hero from the side (the CameraRig taken off, the camera 3.2 to its right) at 640 by 480, cropped: standing (idle), walking ahead at half speed (the blend space between idle, walk and run), backing off, creeping crouched, and leaving the ground in its jump (the camera stays at its height). The clips are files of their own put on the model by `[animations]`.
- `scenarios.json`: `pocket scenario walker --seeds 5 --json`: 30 of 30 runs; a bot holding the move actions toward waypoints climbs the stairs and the ramp to their coins without a second landing, is stopped by the rock at no more than a step's height, waits for the lift, rides it up and walks onto the tower for its coin, pushes a crate more than a metre, and jumps and lands.

Reproduce: `./.pocket/pocket run walker -- --headless --frames 120 --json --capture walker.png --size 960x540`, `./.pocket/pocket scenario walker --seeds 5`, `physics_tests "[character]"`.
