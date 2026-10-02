# iOS Simulator

Three samples packed as iOS apps (`pocket run <name> --ios`, docs/build-system.md, iOS) and run on a
simulated iPhone 18 Pro (iOS 27.0, Xcode 27) on Apple silicon, made by `tools/scripts/ios_evidence.py`
on 2026-10-01. Each app was started paused with its control server open (`-- --serve <port>
--paused`), stepped and driven from the Mac through that server (a simulator shares the Mac's
network, so the same JSON-RPC an agent uses on a desktop runtime reaches the phone), and
photographed with `xcrun simctl io <device> screenshot` (here scaled to 1100 pixels across).

- `hello.png`: 120 ticks stepped from the Mac; the ball had bounced twice. The GPU reports the
  simulator's Metal adapter, the frame 2622 by 1206 pixels (874 by 402 points at 3x), landscape.
- `crates.png`: `input.hold {action: move_x}` drove the car for a second and a half and
  `input.press {action: fire}` let the ball go: the car went from x -2.5 to 4.7 and eight of the
  fifteen crates were down, the HUD saying so in its rich text.
- `walker.png`: `input.hold {action: move_z, sign: -1}` walked the skinned hero from z 5 to z 0 in a
  second, after which it stood idle.

- `farm.png` (2026-10-02, scaled to 874 across): the farm sample at 18:36 set from the Mac
  (`world.set` on its Sky), eight plots at stages set on their `Plot` components, the Weather's
  ground wet: lamps lit after dark, puddles, the plants at their stages. Made by calling the
  script's `run` and `screenshot` for `farm`; the rain itself was off, as the farm's own weather turn
  sets it each tick.

- `island.png` (2026-10-02, scaled to 1100 across): the island sample (`samples/island`) after 60
  ticks, then its `Boat` given `sail: 1` and `throttle: 0.5` from the Mac (`world.set`) and 180
  ticks more: the boat at 3.6 units a second, afloat, under volumetric clouds, on the ocean to the
  horizon, the island's grass drawn on the GPU (`render.stats`: `clouds` true, three grass fields,
  MSAA 4). Made by calling the script's `run` and `screenshot` for `island`.

`ios.json` has the windows, the states before and after, and the hello run's GPU description.
