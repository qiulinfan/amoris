# Wind evidence

- `smoke.png` (2026-09-29): `samples/hills` after 540 ticks (9 seconds), seen across the wind from 24 units off the beacon's downwind side: the smoke its script spawns on the beacon's top (14 particles a second rising at 0.6, drag 0.8) streams off downwind with the scene's Wind (toward 30 degrees, 3 units a second, gusts 0.35 every 24 units; `wind.at` on the peak answered a speed of 2.16 in a lull, gust -0.8). Captured through the control server with the sample's camera turned off and one placed across the wind.

Reproduce: `physics_tests "[wind]"`, `renderer_tests "[wind]"`, and the capture as above (`./.pocket/pocket run hills -- --headless --serve PORT --paused`, `step {ticks: 540}`, a camera spawned 24 units across the wind from 6 units downwind of the beacon, `capture`).
