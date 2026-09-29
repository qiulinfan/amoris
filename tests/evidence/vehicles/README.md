# Vehicle evidence

- `drive.png` (2026-09-29): `samples/drive` at 1280 by 720 after a second and a half of settling and three seconds of full throttle: the red car on its four raycast wheels (the tyres placed by the engine under the body) chased by the camera over the rolling grassland (a terrain with scattered tufts), the gates ahead standing on the ground and facing the route.
- `scenarios.json`: `pocket scenario drive --seeds 5 --json`: the car holds still on its brakes on the slope it starts on, a bot steering for the next gate's bearing passes the first three gates in order (720 ticks; all four take 956), and braking from above 10 units per second stops it within three seconds.

Reproduce: `./.pocket/pocket scenario drive --seeds 5`, `physics_tests "[vehicle]"`.
