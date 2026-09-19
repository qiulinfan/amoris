# The environment interface

A game that agents help make is a game agents must be able to play. The environment interface is the engine's way of being played by a program: reset an episode, act, observe, over the same commands as everything else, so a scripted bot, a learned player, an evaluation harness or a person at a terminal all play the same game the same way. It is what `docs/design/agent-perception.md` calls embodiment.

```python
from pocket_env import PocketEnv                    # sdk/python, standard library only
with PocketEnv("sprites") as env:
    obs = env.reset(seed=1)
    while not obs["done"]:
        obs = env.step({"move_x": 1, "jump": obs["t"] % 40 == 0}, ticks=4)
    print(obs["score"], obs["t"])
```

## Commands

- `env.describe`: the actions the project takes (its input map: positive and negative keys, axes), the keys its observations carry, which of them is the score, whether it says when it is done, the tick rate, the seed, the episode and its `t`.
- `env.reset {seed, max_ticks}`: a fresh episode. The random stream is reseeded, the scene is loaded again and the scripts start again (`project.reload`), held inputs are released, the navigation grid, the debug shapes, the recorder's history and the script errors are dropped. The tick counter, the journal and the event bus carry on, so a session's whole history stays one story; `env.reset` is an event in it. Returns the first observation, at `t = 0`.
- `env.step {actions, ticks, capture}`: act, then run `ticks` ticks (one by default), then observe. `actions` maps action names to a number, held for the whole step with its sign as the direction (`move_x: -1`), or `true`, pressed once at the step's start (`jump: true`); an array of `{action, value}` does the same. An unknown action or a value of another type is refused before anything runs. The holds go through `input.hold` and `input.press`, so they are journaled and seen by the scripts exactly like keys.
- `env.observe {capture}`: the observation now, without running.

## The observation

```json
{"episode": 1, "t": 30, "tick": 35, "time": 0.5,
 "state": {"score": 1, "coins": 5, "player.x": 2.9, "player.grounded": true, "done": false},
 "score": 1, "reward": 1, "done": false,
 "events": [{"seq": 41, "tick": 12, "type": "coin.collected", "subject": 507, "data": {"score": 1}, "cause": 0}],
 "actions": {"move_x": {"down": true, "pressed": false, "released": false, "value": 1}},
 "world_hash": "9d2b...", "errors": 0}
```

`state` is what the project exposes (`expose(name, fn)` in its scripts): the same values `state` returns and scenarios read. `score` is the project's `reward` value when it exposes one, else its `score`, else null; `reward` is that value's change since the previous observation of the episode, so a learner can use it as it is. `done` is the project's `done` value, or `t` reaching `max_ticks`. `events` are the bus's events since the previous observation (at most 200), with their causes, so a player knows what happened and why without polling. `capture`, given a path, writes a PNG of the frame and reports it, for agents that look; the world stays queryable through every other command (`world.query`, `nav.path`, `render.ids`) over the same connection.

A project is playable as soon as it has an input map and exposes what matters. Exposing `score` (or `reward`) and `done` gives it a return and an end; nothing else is needed. The sprites sample exposes both.

## Determinism

An episode is a function of its seed and its acts: the same `reset(seed)` followed by the same `step` calls gives the same states, tick for tick, on the same build, because the runtime has no wall clock in the simulation and its randomness comes from the seeded stream. `tests/runtime_tests` (`[env]`) and `sdk/python/test_pocket_env.py` replay an episode and compare the states.

## The Python client

`sdk/python/pocket_env.py` starts a headless runtime with `--serve 0 --paused` and drives it over HTTP; `PocketEnv(project)` takes a project directory or a sample name (the project bundled with `pocket ts`, the runtime built), `reset`, `step`, `observe`, `describe`, `command` (any runtime command) and `close`; it is a context manager. `play(project, policy, episodes, ticks, step, seed)` runs scripted policies (`random`, `right`, `idle`) and returns one row per episode; `python3 sdk/python/pocket_env.py sprites --policy right --episodes 2` prints them. `pocket test` runs `sdk/python/test_pocket_env.py` as its `python` module when `python3` is on PATH.

## Learning

`PocketEnvPool(project, n)` runs `n` runtimes side by side and fans `reset`, `step` and `observe` out to them in threads, one episode per runtime at a time, so a population plays in parallel. `sdk/python/train_example.py` is a learned player on top of it: the cross-entropy method learns an open-loop plan for the sprites sample (a sequence of macro actions, each held for thirty ticks, scored by the coins collected), playing each generation's plans across the pool, keeping the best quarter and moving the action probabilities toward them. It is small on purpose: the point is that a program can play, be scored and improve through `env.reset` and `env.step` alone, and that the same seed gives the same run. `tests/evidence/environment/train-cem.json` is one run's history.

## Limits

One episode at a time per runtime process; the pool runs several processes for parallel episodes. A served runtime runs until its controller sends `quit` (the client's `close`); only a headless run without a control server stops itself, after 3600 frames. Observations are the exposed state and the events, not a fixed vector: a learner picks its own features from them. There is no reward shaping and no built-in policy beyond the two scripted ones; the engine gives the environment, not the player. `env.reset` restarts the project's own scene and scripts, so a project that keeps state outside them (files it wrote, saves) carries it over; the event log runs on too (its `seq` never goes back, so an agent's cursor stays valid), and a script that counts events starts from `events.lastSeq()` rather than from zero. The web runtime has no server, so the interface is native only.
