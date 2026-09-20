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

`policy-cem.json` is one run of the reactive player (`sdk/python/policy_example.py`), the policy that reads the observation instead of replaying a plan:

```
python3 sdk/python/policy_example.py --generations 8 --population 12 --envs 4 --seed 1 --out tests/evidence/environment/policy-cem.json
```

Six features from the exposed state (a bias, the nearest coin's `dx` and `dy`, `grounded`, `vx`, `wall`) pass through two rows of six weights, one giving the move's sign and one deciding the jump, and the policy acts every four ticks. Each candidate is played from three start spots along the ground (`x` -7.5, -2.3 and 2.0, set with `world.set` after `env.reset`) for 300 ticks, so the score is the mean of three episodes and a replayed route from one start cannot win. Eight generations of twelve candidates, 288 episodes in 115 s of wall time on the debug build. The file holds the features, the starts, the history (`best`, `mean`, `seconds` per generation), the best weights and `per_start`: the learned policy against the scripted run-right-and-jump player from each start.

| Start `x` | Learned | Scripted |
|---|---|---|
| -7.5 | 6 | 3 |
| -2.3 | 6 | 3 |
| 2.0 | 6 | 0 |

The learned policy collects all six coins from every start inside five seconds; the scripted player, which only runs right, gets the coins in its way and none behind it. The population's mean rises from 1.4 to 4.6 over the eight generations. The best weights put most of their mass on `grounded` and `vx` in the move row (keep running while standing, turn at a wall: the `wall` weight is negative) and on `vx` and `wall` in the jump row, which is a sweep of the level rather than a chase of the nearest coin; with a 300-tick budget and six coins spread along the ground and the ledge, the sweep is what the objective rewards.

`planner.json` is one run of the planning player (`sdk/python/planner_example.py`): `python3 sdk/python/planner_example.py --decisions 12 --hold 20 --horizon 20 --envs 5 --seed 1 --json`. Nine decisions, each trying five macro actions in five runtimes that load the committed save slot (the first decision starts from a reset) and read the score after the macro's 20 ticks and 20 more, build a plan of ten macros (three steps left for the coins on the left, then right, a jump onto the ledge, right) that collects all six coins of the sprites level in 200 ticks; the committed plan replayed from the start reaches the same `score 6` at `t 200`, which shows every branch was taken from an exact copy of the run, and the whole search took about five seconds of wall time where the earlier replay-per-candidate search took about eight. The per-decision `steps` carry the macro chosen, its value (the score, with the distance to the next coin breaking ties) and the score and tick it was judged at; `snapshots: true` says the branches came from save slots.
