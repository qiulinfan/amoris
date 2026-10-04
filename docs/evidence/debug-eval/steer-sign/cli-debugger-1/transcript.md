# steer-sign, run cli-debugger-1

Model zai-coding-plan/glm-5.3-flash, via cli, prompt variant debugger. Prompt: [prompt.txt](prompt.txt).

**[0.06 s] bash** (completed)

```
pocket help
```

```
pocket: a client of a running host (pocket serve <project>), found through --host, POCKET_HOST or .pocket/host.json. --json prints exact JSON.

  pocket call <method> ['<json params>'|-]
  pocket status
  pocket info
  pocket save   (the edit world to scene.json)
  pocket world tree [filter] [--with C,..] | get <entity> [C..] | query <C,..> [--fields C.f,..] [--name n] [--limit n] | schema [C] | edit '<ops json>' [--label l] | set <entity> <C> f=v.. | spawn [name] [--prefab json] [--components json] | remove <entity> <C> | destroy <entity>..
  pocket step [ticks] [--until event:<name>|tick:<n>|<entity>.<C>.<field><op><value>] [--watch <entity>.<C>.<field>[~value]]
  pocket time pause | resume | speed <x> | stepped
  pocket play start [--speed x] [--paused] | stop
  pocket undo
  pocket redo
  pocket history
  pocket scripts list | read <path> | write <path> [<file>|-] | apply [--force] | check
  pocket events [--since seq] [--name n|n.*] [--limit n] | --why <seq>
  pocket logs [--since seq] [--limit n]
  pocket snapshots [list] | restore <tick>
  pocket assets [dir]
  pocket catalog
  pocket debug <action> ['<json params>']   (debug.<action>)
  pocket help [command|method]

Host commands: pocket serve <project> [--port 7878], pocket mcp <project>; also pocket run|check|replay|hashes.
Methods (pocket call <method> '<json>'; pocket help <method>):
  assets.list        The project's files with their kind and size (served at /assets/<path>).
  capture            A rendered image or the id buffer's summary of a camera view.
  catalog.list       Every command: name, kind, doc, aliases and the JSON Schema of its parameters.
  debug.state        The debugger: debug.breakpoints.set, debug.breakpoints.clear, debug.pause, debug.continue, debug.step, debug.state, debug.eval, debug.watch, debug.rewind.
  docs.search        Searches command and component names and docs.
  events.since       Game events after a stream number (the newest without one), filtered by name.
  events.why         An event and the chain of events that caused it.
  history.list       The undo and redo stacks' labels, most recent last.
  history.redo       Redoes the last undone edit.
  history.undo       Undoes the last world edit (from the editor or an agent).
  log.since          Script console lines, failed system runs and host messages.
  play.start         Play: forks the edit world and runs the fork in real time; edits go to the fork.
  play.stop          Stop: discards Play's fork and retur
... (4459 chars)
```

**[4.47 s] bash** (completed)

```
pocket scripts list && pocket status
```

```
scripts/components.ts 1389 B
scripts/helm.ts 3369 B
scripts/main.ts 398 B
scripts/rules.ts 3512 B
tick 0 (0.00 s) edit paused real-time x1.0 | 9 entities | hash 0fb8052e39f5

```

**[8.77 s] bash** (completed)

```
pocket scripts read scripts/helm.ts
```

```
// The helm: what the player asks of the boat (the Helm component) turned into the Boat's controls
// every tick. The wheel steers, `goto` hands the wheel to the helmsman who steers for an entity,
// the crew trims the sheet to the apparent wind, and at anchor the sail comes down.
import { component, field, system } from "pocket";

export const Helm = component("Helm", {
    version: 1, doc: "The player's helm: the wheel, the sail wanted, the anchor and a mark to steer for.",
    fields: {
        steer: field.f64(0, "The wheel, -1..1: -1 hard to port (left), 1 hard to starboard (right)."),
        sail: field.f64(1, "How much sail to set, 0..1."),
        anchor: field.bool(false, "Anchored: the sail comes down and the wheel is centred."),
        goto: field.entity("Steer for this entity instead of following the wheel; cleared when it is gone."),
    },
});

/** Full wheel puts this much rudder on (the rudder control runs -1..1). */
const RUDDER_GAIN = 0.6;
/** The helmsman's rudder per degree off the mark's bearing. */
const PILOT_GAIN = 1 / 30;

function clamp(x: number, lo: number, hi: number): number {
    return Math.min(hi, Math.max(lo, x));
}

/** a - b in degrees, wrapped into (-180, 180]. */
function angleDiff(a: number, b: number): number {
    let d = (a - b) % 360;
    if (d > 180) d -= 360;
    if (d <= -180) d += 360;
    return d;
}

/** The sheet that sets the boom at about half the apparent wind angle (the usual trim rule). */
function bestSheet(awaDeg: number): number {
    return clamp((Math.abs(awaDeg) / 2 - 5) / 80, 0, 1);
}

export const helm = system({
    name: "helm", phase: "update",
    doc: "Turns the player's helm (wheel, sail, anchor, goto) into the boat's rudder, hoist and sheet.",
    queries: {
        boats: {
            with: ["Helm", "Boat", "Transform"],
            fields: ["Helm.steer", "Helm.sail", "Helm.anchor", "Helm.goto", "Boat.rudder", "Boat.hoist",
                "Boat.sheet", "Boat.heading_deg", "Boat.awa_deg", "Transform.position"],
        },
    },
    run(ctx, { boats }) {
        const h = boats.cols.Helm;
        const b = boats.cols.Boat;
        const at = boats.cols.Transform.position;
        boats.each((r, boat) => {
            // The wheel, unless the helmsman is steering for a mark.
            let rudder = -clamp(h.steer[r], -1, 1) * RUDDER_GAIN;

            const mark = h.goto[r];
            if (mark !== 0) {
                const t = ctx.world.get(mark as typeof boat, "Transform");
     
... (3369 chars)
```

**[14.71 s] bash** (completed)

```
pocket scripts read scripts/rules.ts
```

```
// The sailing game's rules: stateless systems over the world (charter 3.2). The boat itself is the
// engine's Boat component, which wind, sail, keel and rudder drive (pocket-physics); these rules
// keep the score and the log.
import { system } from "pocket";

/** How near a crate must float, across the water, to be taken aboard (master's REACH). */
const REACH = 3;
/** How far above or below. */
const REACH_UP = 3;

export const muster = system({
    name: "muster", phase: "update", doc: "Counts the crates adrift into every boat's tally.",
    when: "start",
    queries: { boats: { with: ["Tally"] }, crates: { with: ["Cargo"] } },
    run(ctx, { boats, crates }) {
        boats.each((_r, e) => ctx.world.set(e, "Tally", { total: crates.len }));
    },
});

export const log = system({
    name: "log", phase: "update", doc: "Logs the distance sailed and the best speed, and reports the sail going up or down.",
    queries: { boats: { with: ["Boat", "Log"], fields: ["Boat.speed", "Boat.hoist_now", "Log.distance", "Log.top_speed", "Log.sail_set"] } },
    run(ctx, { boats }) {
        const b = boats.cols.Boat;
        const l = boats.cols.Log;
        boats.each((r, e) => {
            const speed = Math.abs(b.speed[r]);
            l.distance[r] = l.distance[r] + speed * ctx.dt;
            l.top_speed[r] = Math.max(l.top_speed[r], speed);
            const set = b.hoist_now[r] >= 0.5;
            if (set !== (l.sail_set[r] === 1)) {
                ctx.emit(set ? "sail.set" : "sail.furled", { tick: ctx.tick }, { subject: e });
                l.sail_set[r] = set ? 1 : 0;
            }
        });
    },
});

export const takeAboard = system({
    name: "take_aboard", phase: "update",
    doc: "A crew told to take a crate takes it aboard when it floats within reach, and says why not otherwise.",
    queries: { boats: { with: ["Crew", "Tally", "Transform"], fields: ["Crew.take", "Transform.position", "Tally.taken", "Tally.worth", "Tally.total"] } },
    run(ctx, { boats }) {
        const crew = boats.cols.Crew;
        const at = boats.cols.Transform.position;
        const tally = boats.cols.Tally;
        boats.each((r, boat) => {
            const target = crew.take[r];
            if (target === 0) return;
            crew.take[r] = 0;
            const crate = target as typeof boat;
            if (!ctx.world.exists(crate) || !ctx.world.has(crate, "Cargo")) {
                ctx.emit("interact.ignored", { code: "sail.crate_gone", crate: target }, { sub
... (3512 chars)
```

**[14.72 s] bash** (completed)

```
pocket scripts read scripts/components.ts && pocket scripts read scripts/main.ts
```

```
// The sailing game's own components (script-host.md 7.3). Master's island script kept its tally,
// its sail state and its last controls in module-level variables; here every bit of game state is
// a component, so a reload, a fork or a replay carries it (charter 3.2).
import { component, field } from "pocket";

export const Crew = component("Crew", {
    version: 1, doc: "The boat's crew and what they are told to do.",
    fields: {
        take: field.entity("A crate to take aboard at the next tick; cleared once the crew tried."),
    },
});

export const Tally = component("Tally", {
    version: 1, doc: "Crates aboard and crates there were.",
    fields: {
        taken: field.u32(0, "Crates taken aboard."),
        worth: field.u32(0, "The value of the crates aboard."),
        total: field.u32(0, "Crates adrift when the game began."),
    },
});

export const Log = component("Log", {
    version: 1, doc: "The ship's log.",
    fields: {
        distance: field.f64(0, "Metres sailed through the water."),
        top_speed: field.f64(0, "The best speed through the water so far, m/s."),
        sail_set: field.bool(false, "Whether the sail stood set at the last tick."),
    },
});

export const Cargo = component("Cargo", {
    version: 1, doc: "A crate adrift that a crew can take aboard.",
    fields: {
        value: field.u32(1, "What it is worth."),
    },
});
// The sailing game (charter 2.4.1) with the debugging evaluation's helm (tools/eval/debug_eval.py).
import { game } from "pocket";
import { Cargo, Crew, Log, Tally } from "./components";
import { Helm, helm } from "./helm";
import { log, muster, takeAboard } from "./rules";

export default game({
    components: [Crew, Tally, Log, Cargo, Helm],
    systems: [muster, helm, log, takeAboard],
});

```

**[24.64 s] bash** (completed)

```
pocket world schema Boat
```

```
Boat (Engine v1) A boat: its controls, the actuators that follow them, and what it reads. Scripts, intents and
boundary writes set the controls; the engine moves the actuators at `HOIST_RATE`, `SHEET_RATE`
and `RUDDER_RATE` and writes the readings.
  afloat?: boolean  Written by the engine: the hull is in the water, not sunk beneath it, and not aground.
  aground?: boolean  Written by the engine: the boat touches land (a fixed body) after the step.
  awa_deg?: number  Written by the engine: apparent wind angle off the bow, positive over the starboard side.
  aws?: number  Written by the engine: apparent wind speed, m/s.
  boom_deg?: number  Written by the engine: the boom's angle from the centreline, positive to starboard.
  drive?: number  Written by the engine: the sail's drive as a share of the best at this apparent wind.
  heading_deg?: number  Written by the engine: where the bow points, degrees (0 toward -z, 90 toward +x).
  heel_deg?: number  Written by the engine: positive heeled to starboard, degrees.
  hoist?: number  Control, 0..1: how much sail to set; 0 furled.
  hoist_now?: number  The sail as set now (follows `hoist`).
  rudder?: number  Control, -1..1: positive turns the bow to starboard.
  rudder_now?: number  The rudder blade now (follows `rudder`).
  sheet?: number  Control, 0..1: 0 hard in (the boom on the centreline), 1 eased right out.
  sheet_now?: number  The sheet as eased now (follows `sheet`).
  speed?: number  Written by the engine: speed through the water along the heading, m/s.
  trim?: Furled|Good|Luffing|Overtrimmed  Written by the engine.

```

**[33.65 s] agent:**

Schema confirms `Boat.rudder`: **positive turns the bow to starboard**. The wheel branch in helm.ts:36 negates steer, so `steer=-1` produces `rudder=+0.6` → turns right. Note the `goto` pilot branch doesn't negate, which corroborates. Now let me confirm in the running game with the debugger.

**[33.67 s] bash** (completed)

```
pocket help debug
```

```
usage: pocket debug <action> ['<json params>']   (debug.<action>)
debug.state (read): The debugger: debug.breakpoints.set, debug.breakpoints.clear, debug.pause, debug.continue, debug.step, debug.state, debug.eval, debug.watch, debug.rewind.
  (no parameters)

```

**[38.25 s] bash** (completed)

```
pocket call debug.breakpoints.set '{"system":"helm","line":36}' ; pocket debug state
```

```
request.missing_field: the debug.breakpoints.set request needs 'file' (a string).
{
  "attached": false,
  "breakpoints": [],
  "exceptions": "none",
  "instrumented": false,
  "state": "running",
  "system": null,
  "tick": 0,
  "waiting_for_debugger": false,
  "watches": []
}

```

**[39.76 s] bash** (completed)

```
pocket call debug.breakpoints.set '{"file":"scripts/helm.ts","line":36}'
```

```
{
  "file": "scripts/helm.ts",
  "id": "bp1",
  "line": 36,
  "locations": [
    {
      "column": 1,
      "file": "scripts/helm.ts",
      "line": 36
    }
  ],
  "verified": true
}

```

**[44.21 s] bash** (completed)

```
pocket world set Sloop Helm steer=-1 && pocket step 1
```

```
set Helm on Sloop (1 edit at tick 0)
tick 1 hash d0ac6645bd16

```

**[48.53 s] bash** (completed)

```
pocket debug state
```

```
{
  "attached": true,
  "breakpoints": [
    {
      "condition": null,
      "id": "bp1",
      "locations": [
        {
          "column": 1,
          "file": "scripts/helm.ts",
          "line": 36
        }
      ],
      "log": null,
      "owner": "agent",
      "target": {
        "file": "scripts/helm.ts",
        "line": 36
      }
    }
  ],
  "exceptions": "none",
  "instrumented": true,
  "state": "running",
  "system": "take_aboard",
  "tick": 1,
  "waiting_for_debugger": false,
  "watches": []
}

```

**[53.23 s] bash** (completed)

```
pocket step 1 ; pocket debug state
```

```
tick 2 hash c68b5c103350
{
  "attached": true,
  "breakpoints": [
    {
      "condition": null,
      "id": "bp1",
      "locations": [
        {
          "column": 1,
          "file": "scripts/helm.ts",
          "line": 36
        }
      ],
      "log": null,
      "owner": "agent",
      "target": {
        "file": "scripts/helm.ts",
        "line": 36
      }
    }
  ],
  "exceptions": "none",
  "instrumented": true,
  "state": "running",
  "system": "take_aboard",
  "tick": 2,
  "waiting_for_debugger": false,
  "watches": []
}

```

**[187.12 s] bash** (completed)

```
pocket call debug.pause && pocket step 1 ; pocket debug state
```

```
{
  "attached": true,
  "breakpoints": [
    {
      "condition": null,
      "id": "bp1",
      "locations": [
        {
          "column": 1,
          "file": "scripts/helm.ts",
          "line": 36
        }
      ],
      "log": null,
      "owner": "agent",
      "target": {
        "file": "scripts/helm.ts",
        "line": 36
      }
    }
  ],
  "exceptions": "none",
  "instrumented": true,
  "state": "running",
  "system": "take_aboard",
  "tick": 2,
  "waiting_for_debugger": false,
  "watches": []
}


<shell_metadata>
shell tool terminated command after exceeding timeout 120000 ms. If this command is expected to take longer and is not waiting for interactive input, retry with a larger timeout value in milliseconds.
</shell_metadata>
```

**[200.55 s] bash** (completed)

```
pocket debug state
```

```
{
  "attached": true,
  "breakpoints": [
    {
      "condition": null,
      "id": "bp1",
      "locations": [
        {
          "column": 1,
          "file": "scripts/helm.ts",
          "line": 36
        }
      ],
      "log": null,
      "owner": "agent",
      "target": {
        "file": "scripts/helm.ts",
        "line": 36
      }
    }
  ],
  "exceptions": "none",
  "frames": [
    {
      "closure": [
        {
          "name": "clamp",
          "type": "function",
          "value": "[function clamp]"
        },
        {
          "name": "RUDDER_GAIN",
          "type": "number",
          "value": 0.6
        },
        {
          "name": "angleDiff",
          "type": "function",
          "value": "[function angleDiff]"
        },
        {
          "name": "PILOT_GAIN",
          "type": "number",
          "value": 0.03333333333333333
        },
        {
          "name": "bestSheet",
          "type": "function",
          "value": "[function bestSheet]"
        }
      ],
      "frame": 0,
      "function": "run",
      "locals": [
        {
          "name": "ctx",
          "type": "object",
          "value": {
            "dt": 0.016666666666666666,
            "emit": "[function emit]",
            "events": [],
            "intent": "[function intent]",
            "part": {
              "entity": "[function entity]"
            },
            "query": "[function query]",
            "rng": {
              "chance": "[function chance]",
              "fill": "[function fill]",
              "int": "[function int]",
              "next": "[function next]",
              "normal": "[function normal]",
              "pick": "[function pick]",
              "range": "[function range]",
              "shuffle": "[function shuffle]",
              "weighted": "[function weighted]"
            },
            "rngFor": "[function rngFor]",
            "rngNamed": "[function rngNamed]",
            "rngTimeless": "[function rngTimeless]",
            "single": "[function single]",
            "system": "helm",
            "tick": 3,
            "time": 0.05,
            "world": {
              "despawn": "[function despawn]",
              "exists": "[function exists]",
              "get": "[function get]",
              "has": "[function has]",
              "insert": "[function insert]",
              "remove": "[function remove]",
              "set": "[function set]",
              "spawn": "[function spawn]"
            }

... (4413 chars)
```

**[220.74 s] agent:**

We're paused inside `helm` at tick 3 (line 49, start of `run`). Counting actual line numbers, the defect is at **line 54** (`let rudder = -clamp(h.steer[r], -1, 1) * RUDDER_GAIN;`), not 36 — that's why that breakpoint never hit. Let me set a breakpoint on line 54 and continue.

**[220.8 s] bash** (completed)

```
pocket call debug.breakpoints.set '{"file":"scripts/helm.ts","line":54}' && pocket call debug.continue && pocket debug state
```

```
{
  "file": "scripts/helm.ts",
  "id": "bp2",
  "line": 54,
  "locations": [
    {
      "column": 1,
      "file": "scripts/helm.ts",
      "line": 54
    }
  ],
  "verified": true
}
{
  "state": "running"
}
{
  "attached": true,
  "breakpoints": [
    {
      "condition": null,
      "id": "bp1",
      "locations": [
        {
          "column": 1,
          "file": "scripts/helm.ts",
          "line": 36
        }
      ],
      "log": null,
      "owner": "agent",
      "target": {
        "file": "scripts/helm.ts",
        "line": 36
      }
    },
    {
      "condition": null,
      "id": "bp2",
      "locations": [
        {
          "column": 1,
          "file": "scripts/helm.ts",
          "line": 54
        }
      ],
      "log": null,
      "owner": "agent",
      "target": {
        "file": "scripts/helm.ts",
        "line": 54
      }
    }
  ],
  "exceptions": "none",
  "frames": [
    {
      "closure": [
        {
          "name": "clamp",
          "type": "function",
          "value": "[function clamp]"
        },
        {
          "name": "h",
          "type": "object",
          "value": {
            "anchor": [
              0
            ],
            "goto": [
              0
            ],
            "sail": [
              1
            ],
            "steer": [
              -1
            ]
          }
        },
        {
          "name": "RUDDER_GAIN",
          "type": "number",
          "value": 0.6
        },
        {
          "name": "ctx",
          "type": "object",
          "value": {
            "dt": 0.016666666666666666,
            "emit": "[function emit]",
            "events": [],
            "intent": "[function intent]",
            "part": {
              "entity": "[function entity]"
            },
            "query": "[function query]",
            "rng": {
              "chance": "[function chance]",
              "fill": "[function fill]",
              "int": "[function int]",
              "next": "[function next]",
              "normal": "[function normal]",
              "pick": "[function pick]",
              "range": "[function range]",
              "shuffle": "[function shuffle]",
              "weighted": "[function weighted]"
            },
            "rngFor": "[function rngFor]",
            "rngNamed": "[function rngNamed]",
            "rngTimeless": "[function rngTimeless]",
            "single": "[function single]",
            "system": "helm",
            "tick": 3,
 
... (9947 chars)
```

**[226.93 s] agent:**

Debugger hit bp2 at helm.ts:54 with `Helm.steer = -1`. The frame already shows `Boat.rudder = [0.6]` from the previous tick — positive rudder, and the engine schema says positive turns the bow to starboard. Let me eval the expression directly for the record:

**[226.99 s] bash** (completed)

```
pocket call debug.eval '{"frame":0,"expr":"-clamp(h.steer[r], -1, 1) * RUDDER_GAIN"}' && pocket call debug.eval '{"frame":0,"expr":"clamp(h.steer[r], -1, 1) * RUDDER_GAIN"}'
```

```
{
  "description": "0.6",
  "type": "number",
  "value": 0.6
}
{
  "description": "-0.6",
  "type": "number",
  "value": -0.6
}

```

**[238.51 s] agent:**

Confirmed: `steer=-1` evaluates to rudder `+0.6` (starboard). Now clearing breakpoints, resuming, and fixing the negation on line 54:

**[238.59 s] bash** (completed)

```
pocket call debug.breakpoints.clear '{"id":"bp1"}' ; pocket call debug.breakpoints.clear '{"id":"bp2"}' ; pocket call debug.continue
```

```
{
  "cleared": 1
}
{
  "cleared": 1
}
{
  "state": "running"
}

```

**[250.99 s] write** (completed)

```
{"content": "// The helm: what the player asks of the boat (the Helm component) turned into the Boat's controls\n// every tick. The wheel steers, `goto` hands the wheel to the helmsman who steers for an entity,\n// the crew trims the sheet to the apparent wind, and at anchor the sail comes down.\nimport { component, field, system } from \"pocket\";\n\nexport const Helm = component(\"Helm\", {\n   
```

```
Wrote file successfully.
```

**[255.49 s] bash** (completed)

```
pocket scripts write scripts/helm.ts helm_fixed.ts && pocket scripts apply
```

```
path scripts/helm.ts | 0 diagnostics
outcome applied | typecheck unavailable | bundle 4381c2a2eade | 0 diagnostics

```

**[264.06 s] bash** (completed)

```
pocket world set Sloop Helm steer=-1 && pocket step 40 && pocket world get Sloop Boat heading_deg,rudder,rudder_now
```

```
set Helm on Sloop (1 edit at tick 3)
tick 43 hash 1d42c30d3555
sim.component_unknown: There is no component 'heading_deg,rudder,rudder_now'; did you mean []?

```

**[268.33 s] bash** (completed)

```
pocket world get Sloop Boat
```

```
#3 Sloop
  Boat: {"afloat":true,"aground":false,"awa_deg":-163.78891998126088,"aws":4.018600095459835,"boom_deg":82.72518514690316,"drive":0.9944115435098282,"heading_deg":80.39486402277167,"heel_deg":8.912193618040204,"hoist":1.0,"hoist_now":1.0,"rudder":-0.6,"rudder_now":-0.6,"sheet":0.9697588615744813,"sheet_now":0.9732374723165077,"speed":2.483082683245718,"trim":"Overtrimmed"}

```

**[276.47 s] agent:**

Fix confirmed in the running game: with `steer=-1`, `rudder` is now `-0.6` and `heading_deg` fell from 90.06 → 80.39 (turning to port). Quick sanity check of the opposite wheel, then restore neutral helm:

**[276.57 s] bash** (completed)

```
pocket world set Sloop Helm steer=1 && pocket step 20 && pocket world get Sloop Boat && pocket world set Sloop Helm steer=0
```

```
set Helm on Sloop (1 edit at tick 43)
tick 63 hash 974c9f093c02
#3 Sloop
  Boat: {"afloat":true,"aground":false,"awa_deg":-152.23319928978756,"aws":3.4053715867143595,"boom_deg":75.59682395182017,"drive":0.9925162661857327,"heading_deg":74.8393716440606,"heel_deg":8.687103396209185,"hoist":1.0,"hoist_now":1.0,"rudder":0.6,"rudder_now":0.06666666666666657,"sheet":0.8893743994331784,"sheet_now":0.8893743994331784,"speed":3.032038230389427,"trim":"Overtrimmed"}
set Helm on Sloop (1 edit at tick 63)

```

**[294.12 s] agent:**

The rudder actuator lags (`rudder_now=0.067` still catching up from `-0.6`), so let me confirm the starboard turn cleanly with a longer step:

**[294.57 s] bash** (completed)

```
pocket world set Sloop Helm steer=1 && pocket step 120 && pocket world get Sloop Boat && pocket world set Sloop Helm steer=0
```

```
set Helm on Sloop (1 edit at tick 63)
tick 183 hash ee1e4c54e389
#3 Sloop
  Boat: {"afloat":true,"aground":false,"awa_deg":-155.15139481511272,"aws":2.968090744695035,"boom_deg":77.24182677454051,"drive":0.993840021261166,"heading_deg":77.13670271682005,"heel_deg":9.926684209813658,"hoist":1.0,"hoist_now":1.0,"rudder":0.6,"rudder_now":0.6,"sheet":0.9087273738181236,"sheet_now":0.9087273738181236,"speed":3.077335629075567,"trim":"Overtrimmed"}
set Helm on Sloop (1 edit at tick 183)

```

**[306.95 s] agent:**

Fixed and verified both directions: `steer=-1` → rudder `-0.6`, heading fell 90.06°→74.84° (port/left); `steer=1` → rudder `+0.6`, heading rose to 77.14° (starboard/right). Helm left centred at steer=0.

**Cause (debugger-confirmed):** With a breakpoint at `scripts/helm.ts:54` and `Helm.steer = -1`, `debug.eval` showed `-clamp(h.steer[r], -1, 1) * RUDDER_GAIN` evaluating to `+0.6`, and the frame showed `Boat.rudder = 0.6` — but the engine's Boat schema says positive rudder turns the bow to starboard. The stray negation inverted the wheel, while the `goto` pilot branch (no negation) was already correct. Removed the `-`; applied via `pocket scripts apply` (0 diagnostics).

{"cause": "The wheel branch of the helm system negated the steer input, so steer=-1 produced a positive rudder, which the engine defines as turning the bow to starboard.", "file": "scripts/helm.ts", "line": 54, "fixed": true}
