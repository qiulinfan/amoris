# Evidence: the environment interface

Produced on 2026-09-19 with the debug (sanitized) build by the Python client (`docs/design/environment.md`), against the sprites sample:

```
python3 sdk/python/pocket_env.py sprites --policy right --episodes 2 --ticks 600 --step 4 --seed 1 --json > tests/evidence/environment/play-right.json
python3 sdk/python/pocket_env.py sprites --policy random --episodes 3 --ticks 600 --step 4 --seed 1 --json > tests/evidence/environment/play-random.json
```

Each row is one episode: `env.reset(seed)` then `env.step` every four ticks until `done` (here `max_ticks = 600`, ten seconds), with the final observation's `score`, `t`, the number of acts and the exposed state.

| File | Policy | What it shows |
|---|---|---|
| `play-right.json` | hold right, press jump every 40 ticks | Two episodes with seeds 1 and 2: both end with `score 1` (the coin ahead), the player at `x 9.6` against the east wall with five coins left; the ledge's coins need a jump timed to the ledge, which this policy does not have. The two episodes are identical apart from their seed because the platformer has no randomness. |
| `play-random.json` | each act picks -1, 0 or 1 for `move_x` and jumps with probability 0.15 | Three episodes with seeds 1, 2 and 3 end with scores 2, 2 and 3 and the player at `x` 0.8, -1 and -3: the random walk collects the coins near the start (three lie within two units to the left, one to the right) and none over the ledge. The policy's own random stream comes from the seed, so every row is reproducible. |

`runtime_tests` (`[env]`) drives the same interface in-process: a reset at tick 5, a 30-tick hold of `move_x` that collects the coin ahead (`reward 1`, the `coin.collected` event in the observation), a jump whose `player.jumped` and `body2d.landed` events arrive with the next observation, a second episode under the same seed whose state after the same acts equals the first's, refused unknown actions, an episode ended by `max_ticks` with a capture written alongside. `sdk/python/test_pocket_env.py` does the same over HTTP and is the `python` module of `pocket test`.

## Learning

`train-cem.json` is the history of one run of the cross-entropy player (`sdk/python/train_example.py`; `docs/design/environment.md`, Learning):

```
python3 sdk/python/train_example.py --generations 8 --population 16 --envs 4 --seed 1 --out tests/evidence/environment/train-cem.json
```

Eight generations of sixteen open-loop plans (twenty macro actions, each held for thirty ticks) played over four runtimes in parallel, 600-tick episodes scored by the coins collected: 128 episodes in 56 s of wall time on the debug build. One row per generation: `best`, `mean`, the `best_plan` and the `seconds` since the start. The best plan of every generation collects all six coins, which ends the episode (`done` is "no coins left", so 6 is the ceiling), while the population's mean rises from 3.75 to 5.5 as the action probabilities move toward the elites: the plans settle on going left first, where three coins lie near the start, then right with jumps. The same seed gives the same plans and the same scores.
