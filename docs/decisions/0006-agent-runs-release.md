# ADR 0006: Agent-facing runs use the release build

- Status: Accepted (2026-09-30).
- Deciders: repository owner (human, who asked for performance and the agent loop to come first),
  Claude (AI collaborator)

## Context

The debug configuration is `-O0` with AddressSanitizer and UndefinedBehaviorSanitizer
(`pocket.toml`, `[configs.debug]`). It is what `pocket test` verifies, and it was also what every
agent-facing run used: the MCP server's `runtime_start`, `pocket_scenario`, `pocket_bench` and
`pocket_run_headless`, the `pocket scenario` and `pocket bench` commands, and the Python environment
(`sdk/python/pocket_env.py`) the agent benchmark and reinforcement-learning pools run on.

The swarm sample measured what that costs: 3000 entities moved through typed arrays take 10.88 ms
per tick in debug and 0.353 ms in release (31 times), and through JSON commands 404 ms against 16 ms
(25 times) (`tests/evidence/swarm/README.md`). An agent stepping a game, a scenario at many seeds, a
policy trained over hundreds of episodes paid that factor on every tick, for checks that are not
what those runs are for.

## Decision

1. Agent-facing runs default to the release configuration (`-O2`, no sanitizers): the MCP tools
   `runtime_start`, `pocket_scenario`, `pocket_bench` and `pocket_run_headless`, which take `config`
   to choose otherwise; `pocket scenario` and `pocket bench` (`--config`); and `PocketEnv`
   (`POCKET_CONFIG`).
2. The test suite, `pocket build`, `pocket run` and `pocket editor` keep debug as their default, so
   the sanitizers still cover everything `pocket test` runs, scenarios and benches included (it
   passes its own configuration), and a person iterating by hand still gets them.
3. No third configuration: release is already built for packs and performance work, so this adds no
   build tree.

## Consequences

- An agent's first `runtime_start` builds the release runtime (a few minutes from clean, then
  incremental).
- A memory error that only shows under the sanitizers is no longer caught while an agent drives a
  session; it is still caught by `pocket test`, which runs every sample's scenarios in debug.
- Numbers an agent reads from `perf` are release numbers, the ones a shipped game would see.
