# Evidence: perception benchmarks

Produced on 2026-09-19 with the debug (sanitized) build by `pocket bench <sample> --json` (`docs/design/scenarios.md`, Perception benchmarks). Each file keeps the rows: the question, the answer the instruments gave, the truth, the runtime commands used with their bytes, the tokens (bytes over four), the ticks of play the answer needed, and what one 960x540 image per tick would cost at a token per 750 pixels (692 per frame).

| Question | Answered through | Tokens | Ticks of play | Images would cost | Ratio |
|---|---|---|---|---|---|
| why did the lantern fall | `world.find`, `events.since` (component.removed), `events.why`: root `joint.broken` | 154 | 272 | 188224 | 1222x |
| which body reached the goal first | `events.since` (goal.reached, limit 1): `/Crate` | 49 | 114 | 78888 | 1610x |
| how much does the chain's top link carry | `physics.joints`: 12.1 N on `/Link1` | 212 | 122 | 84424 | 398x |
| is every body still inside the arena | `world.query` over every body's Transform: true | 1493 | 362 | 250504 | 168x |
| how high does the player jump | `recorder.start`, `recorder.track` of `position.y`: 2.21 (v^2 / 2g is 2.30) | 312 | 86 | 59512 | 191x |
| is the player grounded after landing | `world.get` Body2D: true | 56 | 104 | 71968 | 1285x |
| what does the camera see | `render.visible`: the coins, the level and the player | 352 | 8 | 5536 | 16x |
| where is the player after a second of walking | `world.get` Transform: 5.9 | 46 | 61 | 42212 | 918x |

Every answer matched the truth (`physics.json`: 4 of 4, 1908 tokens against 602040; `sprites.json`: 4 of 4, 766 tokens against 179228). The widest query (every body's transform) is the most expensive answer and still 168 times cheaper than watching; a targeted event or component read is a few dozen tokens. `runtime_tests` (`[bench]`) runs the jump benchmark in-process and checks the record: correct, two commands, under 1500 tokens, the frame baseline at 692 tokens per tick.
