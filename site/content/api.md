# CLI and MCP

The editor and automation use one command catalog. Every entry includes its purpose,
parameters, and JSON Schema. Commands are discoverable from the running host.

## Find the command you need

```sh
./target/release/pocket help
./target/release/pocket catalog --host http://127.0.0.1:7878 --json
./target/release/pocket help world.edit
```

`--host` belongs after the subcommand. Without it, the client tries `POCKET_HOST`, then
the project's `.pocket/host.json`.

## Inspect and control a sample

```sh
./target/release/pocket world tree --host http://127.0.0.1:7878
./target/release/pocket world get Sloop Boat Tally --host http://127.0.0.1:7878 --json
./target/release/pocket play start --paused --host http://127.0.0.1:7878
./target/release/pocket world set Sloop Boat rudder=0.15 sheet=1 --host http://127.0.0.1:7878
./target/release/pocket step 60 --host http://127.0.0.1:7878 --json
./target/release/pocket events --name crate.taken --host http://127.0.0.1:7878
./target/release/pocket play stop --host http://127.0.0.1:7878
```

These are developer-mode commands. The core includes player perception and intention
types, but player-specific `observe` / `act` MCP projections are not exposed in this build.
The showcase's sailing controller is a scripted MCP client over the developer command
surface. It evaluates four temporary Play forks, checks that Stop restores the edit
world's hash, chooses a rudder value, and advances the real world with fixed ticks.

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
./target/release/pocket call capture \
  '{"position":[-12,6,13],"look_at":[3,0,0],"width":1280,"height":720}' \
  --host http://127.0.0.1:7878 --json
```

The native renderer writes a PNG under the project's `.pocket/captures/` and returns its
path, dimensions, tick, instance count, GPU time, and pending asset count.

## MCP

```sh
./target/release/pocket mcp samples/sailing
```

Use this command as a stdio MCP server in your client. It connects to a live host for the
project when one exists, or runs its own game otherwise. A running HTTP host also exposes
the MCP endpoint at `http://127.0.0.1:7878/mcp`.

Read the [host protocol](https://github.com/qiulinfan/amoris/blob/main/docs/spec/host-protocol.md)
for command envelopes, subscriptions, and transport details.
