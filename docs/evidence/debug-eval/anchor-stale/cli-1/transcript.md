# anchor-stale, run cli-1

Model zai-coding-plan/glm-5.3-flash, via cli, prompt variant free. Prompt: [prompt.txt](prompt.txt).

**[0.19 s] bash** (completed)

```
pocket scripts list
```

```
scripts/components.ts 1389 B
scripts/helm.ts 3405 B
scripts/main.ts 398 B
scripts/rules.ts 3512 B

```

**[0.19 s] bash** (completed)

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

**[5.45 s] bash** (completed)

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

**[5.45 s] bash** (completed)

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
            let rudder = clamp(h.steer[r], -1, 1) * RUDDER_GAIN;

            const mark = h.goto[r];
            if (mark !== 0) {
                const t = ctx.world.get(mark as typeof boat, "Transform");
      
... (3405 chars)
```

**[24.47 s] agent:**

Found a suspicious spot already: `helm.ts` computes a local `sail` (0 at anchor) but writes `b.hoist[r] = wanted`. Let me check the other scripts and the Boat schema, and reproduce the symptom.

**[24.51 s] bash** (completed)

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

**[24.53 s] bash** (completed)

```
pocket scripts read scripts/components.ts; pocket scripts read scripts/main.ts
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

**[35.4 s] agent:**

The bug is clear: `helm.ts` computes `sail` (0 at anchor) but writes the unused `wanted` into `Boat.hoist`. Let me reproduce the symptom first, then fix.

**[35.52 s] bash** (completed)

```
pocket world set Sloop Helm anchor=true && pocket step 300 --watch Sloop.Boat.speed~x 2>&1 | tail -5; pocket world get Sloop Boat
```

```
set Helm on Sloop (1 edit at tick 0)
tick 300 hash 84b8b74a6a77 | stopped: limit
#3 Sloop
  Boat: {"afloat":true,"aground":false,"awa_deg":-100.30593354985834,"aws":3.8962425784159462,"boom_deg":47.94817064242448,"drive":0.9600034928920157,"heading_deg":37.36098831663197,"heel_deg":26.248656364791874,"hoist":1.0,"hoist_now":1.0,"rudder":0.0,"rudder_now":0.0,"sheet":0.5640961252049939,"sheet_now":0.5640961252049939,"speed":2.97062878558331,"trim":"Overtrimmed"}

```

**[48.8 s] agent:**

Reproduced: after 300 ticks at anchor, `hoist` is still 1.0 and speed 2.97 m/s. Now I'll get the exact defect line number and write the fixed script.

**[48.87 s] bash** (completed)

```
pocket scripts read scripts/helm.ts | grep -n 'hoist\[r\]\|sail = 0\|let sail\|const wanted'
```

```
71:            const wanted = clamp(h.sail[r], 0, 1);
72:            let sail = wanted;
74:                sail = 0;
78:            b.hoist[r] = wanted;

```

**[57.77 s] bash** (error)

```
pocket scripts read scripts/helm.ts > /tmp/helm_orig.ts && sed 's/^            b\.hoist\[r\] = wanted;$/            b.hoist[r] = sail;/' /tmp/helm_orig.ts > helm_fixed.ts && diff /tmp/helm_orig.ts helm_fixed.ts
```

```
The user has specified a rule which prevents you from using this specific tool call. Here are some of the relevant rules [...]
```

**[57.77 s] agent:**

Defect is at `scripts/helm.ts:78`. Writing the fixed file (one line: `wanted` → `sail`):

**[77.04 s] agent:**

/tmp is blocked; I'll write the file in the working directory instead.

**[77.07 s] write** (completed)

```
{"content": "// The helm: what the player asks of the boat (the Helm component) turned into the Boat's controls\n// every tick. The wheel steers, `goto` hands the wheel to the helmsman who steers for an entity,\n// the crew trims the sheet to the apparent wind, and at anchor the sail comes down.\nimport { component, field, system } from \"pocket\";\n\nexport const Helm = component(\"Helm\", {\n   
```

```
Wrote file successfully.
```

**[82.72 s] bash** (completed)

```
pocket scripts write scripts/helm.ts helm_fixed.ts && pocket scripts apply
```

```
path scripts/helm.ts | 0 diagnostics
outcome applied | typecheck unavailable | bundle 929bb38cf988 | 0 diagnostics

```

**[90.4 s] agent:**

Applied cleanly. Now confirm the fix in the running game (anchor is still set, world at tick 300):

**[90.5 s] bash** (completed)

```
pocket step 400 --watch Sloop.Boat.speed~x 2>&1 | tail -3; pocket world get Sloop Boat Helm
```

```
tick 700 hash 63a46a0731c9 | stopped: limit
#3 Sloop
  Boat: {"afloat":true,"aground":false,"awa_deg":-113.07365684227426,"aws":5.7830800576472114,"boom_deg":54.78195012236373,"drive":0.0,"heading_deg":31.671732658184457,"heel_deg":-1.6722138303948404,"hoist":0.0,"hoist_now":0.0,"rudder":0.0,"rudder_now":0.0,"sheet":0.644493530851338,"sheet_now":0.644493530851338,"speed":0.8838422632899385,"trim":"Furled"}
  Helm: {"anchor":true,"goto":null,"sail":1.0,"steer":0.0}

```

**[101.29 s] agent:**

Fixed and confirmed. Let me verify the sail re-sets when the anchor is raised (controls back to normal) and check the furled event fired:

**[101.45 s] bash** (completed)

```
pocket events --name sail.* --limit 3; pocket world set Sloop Helm anchor=false && pocket step 60 && pocket world get Sloop Boat | grep -o '"hoist[^,]*\|"trim":"[^"]*"\|"speed":[0-9.]*'
```

```
zsh:1: no matches found: sail.*
set Helm on Sloop (1 edit at tick 700)
tick 760 hash 8146d51fe53c
"hoist":1.0
"hoist_now":1.0
"speed":1.7487325875993172
"trim":"Overtrimmed"

```

**[107.58 s] bash** (completed)

```
pocket events --name 'sail.*' --limit 4
```

```
#1 t=1 sail.set subject=3 {"tick":1}
#2 t=331 sail.furled subject=3 {"tick":331}
#3 t=732 sail.set subject=3 {"tick":732}
last 3

```

**[121.98 s] agent:**

The fix works both ways:

- **Before fix (reproduced):** with `anchor=true`, after 300 ticks `hoist` stayed 1.0, trim "Overtrimmed", speed 2.97 m/s.
- **After fix:** the sail furled (event `sail.furled` at t=331), `hoist` → 0, drive → 0, speed decayed 2.97 → 0.88 m/s and falling; raising the anchor re-sets the sail (`sail.set` at t=732) and speed recovers.

**Cause:** in the `helm` system, when at anchor the code computed a furled sail into the local `sail` variable but then wrote the player's raw request into the boat's hoist control (`b.hoist[r] = wanted`), so the sail never came down and the boat kept sailing; the fix writes the anchored value (`b.hoist[r] = sail`).

```json
{"cause": "At anchor the helm system computed a furled sail into the local `sail` variable but then wrote the player's unmodified request into the boat's hoist control (`b.hoist[r] = wanted`), so the sail never came down and the sloop kept sailing.", "file": "scripts/helm.ts", "line": 78, "fixed": true}
```
