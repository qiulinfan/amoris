# M0 evidence: hello

Produced on 2026-09-18 on macOS (Apple M5), Apple clang 21 through the Command Line Tools, debug configuration with AddressSanitizer and UndefinedBehaviorSanitizer.

| File | Command | What it shows |
|---|---|---|
| `hello-headless-120.json` | `pocket_runtime --project samples/hello --bundle build/ts/hello.js --headless --frames 120 --json --size 320x180` | Full JSON report: 120 frames = 120 ticks (headless is one tick per frame), the exposed state (`hue`, `ball.y`, `ball.grounded`, `bounces`), the cumulative state hash `08cb4ca0c92b98cd`, the capture's center pixel matching the script's clear color, the log tail with the two bounce events (ticks 46 and 111). |
| `hello-headless-120.png` | same run | The captured frame (a flat color; the sample draws nothing else yet). |
| `hello-window-150.png` | `pocket_runtime --project samples/hello --bundle build/ts/hello.js --frames 150 --capture ...` | Same pixels when rendered through a real window and presented via the surface blit: captures are taken from the offscreen target in both modes. |
| `../hello-golden.json` | written by `runtime_tests` on first run | The pinned state and hash for 120 headless ticks; the test fails if either changes. |

`pocket test --json` on the same day: `core_tests` (random, hashing, tick clock, math, results, logger ring, filesystem) and `runtime_tests` (headless run and report shape, determinism across runs and seeds, golden hash, script errors reported without crashing) both pass, 728 ms total.
