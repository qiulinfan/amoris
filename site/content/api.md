# CLI and MCP

The editor and automation use one command catalog. Every entry includes its purpose,
parameters, and JSON Schema. Commands are discoverable from the running host.

## Find the command you need

```sh
./target/release/amoris help
./target/release/amoris catalog --host http://127.0.0.1:7878 --json
./target/release/amoris help world.edit
```

`--host` belongs after the subcommand. Without it, the client tries `AMORIS_HOST`, then
the project's `.amoris/host.json`.

## Inspect and control a sample

```sh
./target/release/amoris world tree --host http://127.0.0.1:7878
./target/release/amoris world get Sloop Boat Tally --host http://127.0.0.1:7878 --json
./target/release/amoris play start --paused --host http://127.0.0.1:7878
./target/release/amoris world set Sloop Boat rudder=0.15 sheet=1 --host http://127.0.0.1:7878
./target/release/amoris step 60 --host http://127.0.0.1:7878 --json
./target/release/amoris events --name crate.taken --host http://127.0.0.1:7878
./target/release/amoris play stop --host http://127.0.0.1:7878
```

These are developer-mode commands. The showcase's earlier scripted sailing controller
is an MCP client over the developer command
surface. It evaluates four temporary Play forks, checks that Stop restores the edit
world's hash, chooses a rudder value, and advances the real world with fixed ticks.

## Play through a restricted seat

Games with declared player seats expose `player.*` commands through CLI, HTTP, and MCP.
The engine limits observations to the seat's perception and refuses developer commands
when a call is made as that player. Start the sailing course host in one terminal:

```sh
./target/release/amoris serve samples/sailing-course --editor editor/dist
```

In another terminal, inspect the skipper's view and available intentions:

```sh
./target/release/amoris player observe --seat skipper --host http://127.0.0.1:7878
./target/release/amoris player intents --seat skipper --host http://127.0.0.1:7878
```

For a seat-bound stdio MCP session:

```sh
./target/release/amoris mcp samples/sailing-course --seat skipper
```

This session lists only the `player` tool, with actions including `observe`, `nearby`,
`affordances`, `intents`, `act`, and `wait`. HTTP callers pass `seat` alongside `method`
and `params` to `/api/call`. The seat parameter selects a player role; it is not a remote
authentication token. Token grants over HTTP MCP, player forks/replay, and script-defined
intent executors remain unfinished. See the
[player specification](https://github.com/qiulinfan/amoris/blob/main/docs/spec/player.md).

The [DeepSeek recording](agents.md) used an earlier project-level gateway. Its results
have not been rerun through these newer native player interfaces.

## Important command families

| Family | Commands |
| --- | --- |
| World | `world.tree`, `world.get`, `world.query`, `world.schema`, `world.edit` |
| History | `history.list`, `history.undo`, `history.redo` |
| Time and Play | `time.control`, `time.step`, `play.start`, `play.stop` |
| Scripts | `scripts.list`, `scripts.read`, `scripts.write`, `scripts.check`, `scripts.apply`, `scripts.types` |
| Events and logs | `events.since`, `events.why`, `log.since` |
| Snapshots | `snapshots.list`, `snapshots.restore` |
| Debugger | `debug.state`, `debug.pause`, `debug.continue`, `debug.step`, `debug.eval`, `debug.watch` |
| Presentation | `assets.list`, `capture` |
| Players | `player.session`, `player.observe`, `player.nearby`, `player.intents`, `player.act`, `player.wait` |

`world.edit` validates the whole batch before applying it and records an undoable edit.
Play operates on an explicit fork; Stop returns to the edit world.

## HTTP

`GET /api/catalog` lists the same command definitions. Calls use `POST /api/call`:

```json
{
  "id": 1,
  "method": "world.get",
  "params": { "entity": "Sloop", "components": ["Boat", "Tally"] }
}
```

A successful response contains `result`; a refused call contains `error` with `code`,
`message`, and `detail`. WebSocket clients subscribe through `/ws`; render feeds use `/render`.

## Capture an actual engine view

```sh
./target/release/amoris call capture \
  '{"position":[-12,6,13],"look_at":[3,0,0],"width":1280,"height":720}' \
  --host http://127.0.0.1:7878 --json
```

The native renderer writes a PNG under the project's `.amoris/captures/` and returns its
path, dimensions, tick, instance count, GPU time, and pending asset count.

## MCP

```sh
./target/release/amoris mcp samples/sailing
```

Use this command as a stdio MCP server in your client. It connects to a live host for the
project when one exists, or runs its own game otherwise. A running HTTP host also exposes
the MCP endpoint at `http://127.0.0.1:7878/mcp`.

Read the [host protocol](https://github.com/qiulinfan/amoris/blob/main/docs/spec/host-protocol.md)
for command envelopes, subscriptions, and transport details.
