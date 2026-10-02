# Driving Pocket from an agent: `pocket mcp`

`pocket mcp` serves the Model Context Protocol over stdio. Any MCP client (Claude Code, Codex, an
IDE, a CI harness) gets the same commands that scripts and tests use: build and test the engine, run
a project headless, or start a paused session and step it while reading the world as text.

## Register

Claude Code:

```bash
claude mcp add pocket -- /Users/you/aipocket/.pocket/pocket --root /Users/you/aipocket mcp
```

Codex (`~/.codex/config.toml`):

```toml
[mcp_servers.pocket]
command = "/Users/you/aipocket/.pocket/pocket"
args = ["--root", "/Users/you/aipocket", "mcp"]
```

The binary comes from `scripts/bootstrap.sh`. On a machine where Xcode itself is unusable, set
`DEVELOPER_DIR=/Library/Developer/CommandLineTools` in the client's environment for the server
process.

The repository's `.mcp.json` registers the same server for any client that reads a project's
`.mcp.json` (Claude Code, oh-my-pi, Cursor): started at the repository root, they find the `pocket`
server without a command.

pi, oh-my-pi and other agents:

- **oh-my-pi** (`omp`) reads `.mcp.json`: run it at the repository root and the `pocket` tools are
  there (it calls them through its `write` tool on `xd://mcp__pocket_<tool>`).
- **pi** has no MCP by design; `integrations/pi/pocket.ts` is a pi extension with the same reach: a
  `pocket` tool (a command and its params) and `pocket_look` (the frame as an image, with what is
  visible; `around: true` or an entity's name for the six-sided sheet of `render.views`). Load it
  with `pi -e integrations/pi/pocket.ts` or copy it into `~/.pi/agent/extensions/`; it drives
  `$POCKET_RPC_URL`, or the url `/pocket-attach <url>` sets.
- **Any agent with a shell**: `pocket rpc <method> '<params>'` sends one command to
  `$POCKET_RPC_URL` (or `--url`) and prints the result: JSON, or the text of a text result such as
  `world.tree`. `pocket rpc help '{"command": "world.set"}'` says how to call a command. The server
  takes a JSON-RPC batch (an array of requests) and also the agent tools' shape,
  `{"calls": [{"method", "params"}, ...]}`, answered with each call's method and its result or error
  in order: an agent scripting its own checks in a shell or a Python cell sent exactly that.

## Attaching to a running runtime

The MCP server drives a runtime it started (`runtime_start`) or one that is already running:
`runtime_attach {url}` attaches to any runtime's control server (an editor started with
`-- --serve 4711`, a game under test, the agent benchmark's), and with `POCKET_RPC_URL` set in its
environment the server attaches to that runtime by itself on the first call. `runtime_stop` lets an
attached runtime go (`quit: true` stops it). So an agent can work on the game a person has open in
the editor, and see what they see: `capture {image: true}` returns the frame as an image for a model
that reads images.

## Applying an edit

After an agent edits a running project's scripts or `project.toml`, `project.apply {ticks?}` (the
MCP tool `project_apply`, `pocket_apply` in the pi extension, `pocket rpc project.apply` in a shell)
does the rest in one call: the runtime has the pocket tool bundle the scripts into the bundle it
runs and type-check them, puts the type errors where `pocket run --watch` does
(`script.diagnostics`, the editor's Script tab), reloads the project (a fresh world from the scene,
the scripts started over it, `project.toml`'s settings read again) and steps it `ticks` (1), and
answers `bundle_ms`, `check_ms`, `type_errors` with the first ten, the reload, the state after and
any `script_errors`. A script that does not bundle is answered with the bundler's message and leaves
the running project as it was. The runtime finds the tool through `POCKET_ROOT` and `POCKET_TOOL`,
which `pocket run`, `pocket editor` and the MCP server set when they start it; a packed game has no
tool and says so. `project.reload` on its own reloads the bundle as it is, and lists the sources
newer than it as `stale`.

In the agent benchmark the script, mechanic and file tasks are where the tokens go
(docs/agent-eval.md): an agent that edited a script used to find out what it did only by rebundling
through the shell or waiting for the harness; now it is one call after the edit.

## Asking the engine how to call it

`project.brief` is the place to start: one screen of text with the project's files, the scene's
roots, the components in use and the project's own (with their fields and value names), the input
actions, the exposed state, the events so far, what `world.lint` finds and where to look next;
`runtime_start` and `runtime_attach` answer with it, so an agent knows the game before its first
question. Every command has a usage line and a summary in the engine itself:
`help {command: "world.set"}` answers
`world.set {entity, component, value, components?, cause?, quiet?}` and what it does, `help {}` all
of them, `commands {usage: true}` the list with usages (the MCP tools `runtime_help` and
`runtime_commands`). For an agent's context there is a compact form and filters:
`commands {text: true}` is an index of the names by family (1.9 KB for all of them), and with
`family` (`world`, `render`, `physics`, ...) or `search` (the commands that mention a word, as for
`help`) it is one line a command, its usage and the first clause of its summary (every command that
way is 20 KB, which agents asked for seven times in a run of the forty-eight tasks, and kept in
their context afterwards); `world.schema {component}` (or `components`, or `search`) gives one
component's fields instead of all forty. The pi extension shows results as compact JSON and, when it
must cut one, names these filters. A command refuses a parameter it does not take and names the ones
it takes
(`world.query does not take 'pattern': it takes with?, without?, name?, under?, fields?, limit?`),
so a guessed key fails at once instead of being ignored while the call looks like it worked; a
mistyped command is answered with the names it is close to; and where a command takes an `entity`
and no `path`, a `path` stands for it. `world.set` without a `value` object (or a `component`) says
what it needs. The fields themselves are checked the same way in `world.set`,
`world.spawn {components}` and `world.instantiate {components}`: a key that is not a field of the
component is refused with the nearest one (`Light has no field 'colr'; did you mean 'color'?`), a
value of the wrong type with the type it should be, and nothing of the call is applied; inside a
list field the element is named (`Animator.layers[0] has no field 'wieght'`). Integer codes have
names a write may use instead (`Light.kind: "point"`, `RigidBody.kind: "kinematic"`,
`Collider.shape: "capsule"`, `Sky.mode: "atmosphere"`, ...; `world.schema` lists them under `names`,
and a scene file may use them too), and `world.set` answers with the component as it now is
(`{ok, value}`), so the result of a partial write is read without a second call. These came from
watching a model work (docs/agent-eval.md): its first `world.set` passed the fields under `fields`,
was answered `{"ok": true}`, changed nothing, and the model spent a dozen calls reading the engine's
source to find out why. The 48 failed calls of a later run of fifty-five tasks showed what agents
still reached for, and the commands now take it: `world.query {fields}` names a component's field
(`"Transform.position"`, `"Transform.position.y"`; nine calls had tried it) or a bare field one of
the `with` components has, and a bare field nothing can resolve is answered with the components that
have it; `world.set` takes a component the way `world.spawn` does (`{entity, Transform: {...}}`), a
component's fields beside its name (`{entity, component: "Transform", position: {...}}`), or several
at once (`components: {...}`, all checked before any is set, answered as `{ok, values}`);
`world.get` without a component answers every component the entity has; an id written as a string
(`"492"`) is that entity when nothing is named so; in `world.spawn` and `world.set` a field named by
the only field that starts so (three letters or more: `pos`, `rot`) or by a short form (`scl`,
`col`, `colour`) is that field, and the answer's `renamed` says what was read as what
(`{"Transform.pos": "position"}`; a run of fifty-seven tasks had two calls refused for `pos`), while
a start two fields share is refused with the nearest; `tilemap.get` reads a cell when given one and
the map's rows otherwise, and `tilemap.text {entity}` reads the rows (`tilemap.rows`), the names an
agent reached for first; `world.remove` without a component points to `world.destroy`; a script
function asked for as a command (`dialogue.check`) is answered with how to call it through
`script.eval`; `log.tail` takes `lines` for `n`, and `world.set` `values` or `fields` for `value`;
an MCP tool's name sent as a method (`runtime_commands`) is answered with the command it stands for;
and `input.release` lets go of what `input.hold` holds.

A field typed as an entity (`Joint2D.body`; `world.schema` shows the type) takes the entity's name
or path in `world.spawn` and `world.set` as well as its id (`Joint2D: {body: "Plank0"}`), turned
into the id as it is written; a name that names nothing is refused. In the first run of the bridge
task the agent worked out the planks' ids from `world.roots` before spawning them, which held only
because nothing else spawned in between.

Scripts are asked the same way: `help {sdk: "timer.after"}` answers an SDK export's signature as
written, the first sentence of its documentation and its file; `help {sdk: "timer"}` every export
whose name holds the text, `help {sdk: ""}` the parts and their exports, and an interface's entry
lists its fields (`RayHit`), which is what a function returning it answers. The table is generated
from `sdk/runtime/*.ts` by `pocket gen` (with every build), as is `docs/generated/sdk.md`, the whole
list on one page. Before it, the traces showed agents reading the SDK's sources to learn what
`world.summary()` returns or when a timer fires: an eighth of what they read.

## Tools

| Tool | What it does |
|---|---|
| `pocket_doctor`, `pocket_build`, `pocket_test`, `pocket_gen`, `pocket_check` | The build tool with structured results (diagnostics with file, line, column; per-suite test results; `pocket_check` the TypeScript type errors of a project or the workspace). |
| `pocket_scenario` | Run a project's gameplay scenarios (`scenarios/*.ts`) at several seeds: pass counts, ticks to pass, each failure's step and reason (`docs/design/scenarios.md`). The runs go to a pool of runtimes at once (`POCKET_JOBS`, else half the cores). |
| `pocket_bench` | Run a project's perception benchmarks (`benches/*.ts`): per question, whether the instruments answered correctly, the tokens it cost, and what frame-by-frame vision would have cost (`docs/design/scenarios.md`, Perception benchmarks). |
| `pocket_run_headless` | Run a project for N frames and return the JSON report: exposed state, state hash, world summary, event histogram and tail, optional capture. |
| `runtime_start` | Build, bundle and launch a project paused with the control server. Headless by default; `headless: false` opens a window. The release runtime unless `config: "debug"` (sanitized, 25 to 30 times slower; docs/decisions/0006-agent-runs-release.md), as for `pocket_scenario`, `pocket_bench` and `pocket_run_headless`. With `POCKET_RUNTIME` set in the server's environment, that binary is launched as it is, nothing built (and so for `pocket_scenario`, `pocket_bench` and `pocket_run_headless`, and the CLI's `run` and `scenario`). The runtime is started with `--exit-with-parent`: when the MCP server goes (its agent ended without `runtime_stop`), the runtime ends within a twentieth of a second instead of serving on alone; three such had been found paused five hours after a benchmark run. Any runtime also ends on SIGINT, SIGTERM and SIGHUP, paused and serving or not, printing its report as at a run's end. |
| `step` | Advance N ticks; returns tick, exposed state and hashes. `until` ends it early at the first tick after which a condition holds: an event (`{event: "coin."}`, a type or a prefix), an exposed value (`{state: "score", at_least: 3}`) or a component field (`{entity: "Player", component: "Transform", field: "position.y", below: 0}`), compared with `equals`, `above`, `below`, `at_least`, `at_most` or `changes`; the answer's `until` holds `met`, the `tick` and the event or value seen, so "walk right until the coin is taken" is one call instead of a loop of steps. `watch` follows values through the step: `["score", "Player:Transform.position.y"]` (an exposed name, `Entity:Component.field`, or `{entity, component, field}`) answers each one's `first` and `last`, how often it `changes`, and for numbers `min` and `max` with `min_tick` and `max_tick`, so the top of a jump or the lowest a value fell is one call; `every: n` adds `series`, the value every n ticks as `[tick, value]`. `keys: [...]` answers only those exposed values and the tick (`missing` names any not exposed); `state {keys}` does the same without stepping. Headless, only the last tick is drawn (`render: "each"` draws them all), which makes a long step 8 to 40 times faster; what a script reads from `render.stats` is the last drawn frame's. A call that reads the picture or the camera (`capture`, `render.visible`, `render.pick`, `render.project` and the like) after undrawn ticks draws the world as it is first. |
| `world_tree`, `world_query`, `world_describe`, `world_schema` | The observable world as text and JSON. |
| `events_since` | The causal event log after a sequence number, with `cause` links; `runtime_command {method: "events.why", params: {seq}}` turns one event into its chain of causes. |
| `capture`, `render_pick` | The last frame as PNG, the entity id buffer, the entity under a pixel. |
| `asset_preview` | A model file drawn on its own (framed, under a sky and a sun) and returned as a picture, to look at a model before placing it (`assets.preview`). |
| `look_around` | The scene, or one entity and what is under it, from six sides at once in one picture (`render.views`), returned as an image: what was built, checked from every side without moving the camera. |
| `ui_snapshot`, `ui_query`, `ui_click`, `ui_type`, `ui_key` | The interface as text, element lookup by name/text/type, and synthetic input through the same path a player's input takes (see `docs/design/pocket-ui.md`). With `runtime_start {editor: true}` the same tools operate the editor (`docs/editor.md`). |
| `runtime_command` | Any other command (`world.spawn`, `world.set`, `events.emit`, `world.save`, `save.write`, `input.hold`, `perf`, `log.tail`, `render.visible`, `render.unproject`, `tilemap.set/fill/save`, `nav.bake/path/reachable/nearest`, `recorder.at/diff/track/first` when the session started with `history`, ...); `runtime_commands` lists them. |
| `runtime_stop` | Quit the session and return its final report. |

## A session

1. `runtime_start { project: "playground" }` builds the runtime, bundles the TypeScript, starts
   `pocket_runtime --serve 0 --paused --headless` and reports the initial state.
2. `world_tree { depth: 2 }` shows the scene loaded from `scene.json`.
3. `step { ticks: 120 }` runs two simulated seconds; the exposed state says how many enemies exist
   and the player's health.
4. `events_since { seq: 0, type: "player." }` explains what hit the player; each `player.hit`
   carries the `cause` of the enemy's spawn event.
5. `capture { path: "out.png", ids: "ids.png" }` writes the picture and the id buffer and lists the
   visible entities with pixel counts. A relative path is under the project (never out of it; the
   answer's `path` says where), an absolute one where it says; `render.views` and `render.ids` write
   the same way.
6. `runtime_stop` returns the report with the deterministic state hash, which `pocket_run_headless`
   with the same seed and frame count reproduces.

Everything the session did is also reachable without MCP: the runtime's `--serve` port speaks
JSON-RPC 2.0 on `POST /rpc` and serves `GET /tree`, `/state`, `/summary`, `/events?since=N`,
`/schema`, `/commands`.
