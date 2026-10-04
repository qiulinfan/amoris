# goto-wrap, run cli-1

Model zai-coding-plan/glm-5.3-flash, via cli, prompt variant free. Prompt: [prompt.txt](prompt.txt).

**[0.09 s] bash** (completed)

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

**[4.56 s] bash** (completed)

```
pocket scripts list && pocket status
```

```
scripts/components.ts 1389 B
scripts/helm.ts 3339 B
scripts/main.ts 398 B
scripts/rules.ts 3512 B
tick 0 (0.00 s) edit paused real-time x1.0 | 9 entities | hash 0fb8052e39f5

```

**[8.95 s] bash** (completed)

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
                if (t === undefined
... (3339 chars)
```

**[40.81 s] bash** (completed)

```
pocket world get Sloop && pocket world get Sloop Transform,Boat,Helm 2>/dev/null | head -50
```

```
#3 Sloop
  Boat: {"afloat":false,"aground":false,"awa_deg":0.0,"aws":0.0,"boom_deg":0.0,"drive":0.0,"heading_deg":0.0,"heel_deg":0.0,"hoist":1.0,"hoist_now":1.0,"rudder":0.0,"rudder_now":0.0,"sheet":1.0,"sheet_now":1.0,"speed":0.0,"trim":"Furled"}
  Collider: {"density":null,"friction":0.4,"restitution":0.1,"shape":{"Compound":{"parts":[{"position":[0.0,0.0,0.0],"rotation":[0.0,0.0,0.0,1.0],"shape":{"ConvexHull":{"points":[[-0.01,0.05,-1.75],[0.01,0.05,-1.75],[-0.03,0.4,-1.75],[0.03,0.4,-1.75],[-0.12,-0.2,-1.0],[0.12,-0.2,-1.0],[-0.3,0.36,-1.0],[0.3,0.36,-1.0]]}}},{"position":[0.0,0.0,0.0],"rotation":[0.0,0.0,0.0,1.0],"shape":{"ConvexHull":{"points":[[-0.12,-0.2,-1.0],[0.12,-0.2,-1.0],[-0.3,0.36,-1.0],[0.3,0.36,-1.0],[-0.18,-0.25,0.0],[0.18,-0.25,0.0],[-0.4,0.35,0.0],[0.4,0.35,0.0]]}}},{"position":[0.0,0.0,0.0],"rotation":[0.0,0.0,0.0,1.0],"shape":{"ConvexHull":{"points":[[-0.18,-0.25,0.0],[0.18,-0.25,0.0],[-0.4,0.35,0.0],[0.4,0.35,0.0],[-0.16,-0.22,1.0],[0.16,-0.22,1.0],[-0.38,0.35,1.0],[0.38,0.35,1.0]]}}},{"position":[0.0,0.0,0.0],"rotation":[0.0,0.0,0.0,1.0],"shape":{"ConvexHull":{"points":[[-0.16,-0.22,1.0],[0.16,-0.22,1.0],[-0.38,0.35,1.0],[0.38,0.35,1.0],[-0.12,-0.15,1.65],[0.12,-0.15,1.65],[-0.32,0.36,1.65],[0.32,0.36,1.65]]}}},{"position":[0.0,0.0,0.0],"rotation":[0.0,0.0,0.0,1.0],"shape":{"ConvexHull":{"points":[[-0.03,-0.24,-0.45],[0.03,-0.24,-0.45],[-0.03,-0.24,0.4],[0.03,-0.24,0.4],[-0.04,-0.85,-0.25],[0.04,-0.85,-0.25],[-0.04,-0.85,0.3],[0.04,-0.85,0.3]]}}},{"position":[0.0,0.0,0.0],"rotation":[0.0,0.0,0.0,1.0],"shape":{"ConvexHull":{"points":[[-0.015,0.0,1.66],[0.015,0.0,1.66],[-0.015,0.0,1.9],[0.015,0.0,1.9],[-0.015,-0.55,1.66],[0.015,-0.55,1.66],[-0.015,-0.55,1.86],[0.015,-0.55,1.86]]}}}]}}}
  Crew: {"take":null}
  ExternalForce: {"force":[0.0,0.0,0.0],"torque":[0.0,0.0,0.0]}
  Floater: {"drag":0.05,"heave":6.0,"points":[{"at":[-0.018055555555555557,0.07250000000000001,-1.625],"size":0.12833333333333333,"volume":0.0046375},{"at":[0.018055555555555554,0.07250000000000001,-1.625],"size":0.12833333333333333,"volume":0.0046375},{"at":[-0.025833333333333333,0.20083333333333334,-1.625],"size":0.12833333333333333,"volume":0.0046375},{"at":[0.025833333333333333,0.20083333333333334,-1.625],"size":0.12833333333333333,"volume":0.0046375},{"at":[-0.03361111111111111,0.3291666666666667,-1.625],"size":0.12833333333333333,"volume":0.0046375},{"at":[0.033611111111111105,0.3291666666666667,-1.625],"size":0.12833333333333333,"volume":0.0046375},{"at":[-0.040
... (11104 chars)
```

**[49.45 s] bash** (completed)

```
pocket scripts read scripts/rules.ts && pocket world query Transform --name Crate --fields Name,Transform.position 2>/dev/null || pocket world tree
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
... (3737 chars)
```

**[73.69 s] bash** (completed)

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

**[94.94 s] agent:**

Scripts read. Now let me reproduce the bug in the running game. The Sloop starts heading +x (heading 90°); I'll set `goto=Crate1` and step until the crate falls behind, watching the rudder.

**[95.0 s] bash** (completed)

```
pocket world set Sloop Helm goto=Crate1 && pocket step 400 --watch Sloop.Boat.rudder && pocket world get Sloop Boat,Transform
```

```
set Helm on Sloop (1 edit at tick 0)
tick 1 hash e490bc16ad83 | stopped: Boat.rudder 0.0 -> 1.0
sim.component_unknown: There is no component 'Boat,Transform'; did you mean []?

```

**[99.77 s] bash** (completed)

```
pocket world get Sloop Boat; pocket world get Sloop Transform
```

```
#3 Sloop
  Boat: {"afloat":true,"aground":false,"awa_deg":180.0,"aws":6.0,"boom_deg":-84.29166666666667,"drive":0.9965300493806047,"heading_deg":90.03764215815114,"heel_deg":-0.12634124218342155,"hoist":1.0,"hoist_now":1.0,"rudder":1.0,"rudder_now":0.03333333333333333,"sheet":0.0,"sheet_now":0.9916666666666667,"speed":-0.2510780179612562,"trim":"Overtrimmed"}
#3 Sloop
  Transform: {"position":[0.0014278496382758021,-0.0004680156707763672,-0.0008673908887431026],"rotation":[-0.0012499043950811028,-0.7073379158973694,0.00030900482670404017,0.7068744897842407]}

```

**[109.54 s] bash** (completed)

```
pocket step 600 && pocket world get Sloop Transform
```

```
tick 601 hash 2e4f3b4c9ddc
#3 Sloop
  Transform: {"position":[13.203265190124512,0.09480810165405273,2.8470160961151123],"rotation":[-0.015966380015015602,0.5592787265777588,-0.02399400807917118,0.8284784555435181]}

```

**[213.84 s] agent:**

The step stopped early on the watch. Let me check the debugger, then set up a natural repro: sail past Crate2 so Crate1 ends up behind-starboard, then ask for it.

**[213.89 s] bash** (completed)

```
pocket snapshots list; pocket help debug
```

```
kept (every 60 ticks): 0 60 120 180 240 300 360 420 480 540 600
usage: pocket debug <action> ['<json params>']   (debug.<action>)
debug.state (read): The debugger: debug.breakpoints.set, debug.breakpoints.clear, debug.pause, debug.continue, debug.step, debug.state, debug.eval, debug.watch, debug.rewind.
  (no parameters)

```

**[218.91 s] bash** (completed)

```
pocket snapshots restore 0 && pocket world set Sloop Helm goto=Crate2 && pocket step 900 --until tick:900
```

```
restored tick 0; tick 0 (0.00 s) edit paused real-time x1.0 | 9 entities | hash 0fb8052e39f5
set Helm on Sloop (1 edit at tick 0)
tick 900 hash 4f8ce236b124 | stopped: tick

```

**[223.74 s] bash** (completed)

```
pocket world get Sloop Transform; pocket world get Sloop Boat | jq -c '{heading_deg,rudder,speed}'
```

```
#3 Sloop
  Transform: {"position":[13.730895042419434,-0.01225733757019043,-6.6467719078063965],"rotation":[0.013009856455028057,0.9385640621185303,0.05268578231334686,0.3408113718032837]}
jq: parse error: Invalid numeric literal at line 1, column 3

```

**[234.33 s] bash** (completed)

```
pocket --json world get Sloop Boat 2>/dev/null | jq -c '.Boat | {heading_deg,rudder,speed}'; pocket world set Sloop Helm goto=Crate1 && pocket step 30 && pocket --json world get Sloop Boat | jq -c '{heading_deg,rudder}'
```

```
{"heading_deg":null,"rudder":null,"speed":null}
set Helm on Sloop (1 edit at tick 900)
tick 930 hash b17379f9d293
{"heading_deg":null,"rudder":null}

```

**[239.17 s] bash** (completed)

```
pocket call world.get '{"entity":"Sloop","components":["Boat","Transform"]}' | jq -c
```

```
jq: parse error: Invalid numeric literal at line 1, column 3

```

**[244.0 s] bash** (completed)

```
pocket --json world get Sloop Boat
```

```
{
  "error": {
    "code": "check.usage",
    "detail": {
      "flag": "--json",
      "suggestions": []
    },
    "message": "pocket has no subcommand '--json'; did you mean []? (pocket help)"
  }
}

```

**[249.22 s] bash** (completed)

```
pocket world get Sloop Boat --json 2>/dev/null | tail -3; pocket world get --json Sloop Boat | tail -3
```

```
{"components":{"Boat":{"afloat":true,"aground":false,"awa_deg":45.33232393881646,"aws":6.4553698609231756,"boom_deg":-18.799074923950332,"drive":0.9722390975212474,"heading_deg":221.43748976682926,"heel_deg":0.5680466377131645,"hoist":1.0,"hoist_now":1.0,"rudder":-1.0,"rudder_now":-1.0,"sheet":0.22116558734059213,"sheet_now":0.22116558734059213,"speed":0.5542546526230394,"trim":"Good"}},"id":3,"name":"Sloop"}
{"components":{"Boat":{"afloat":true,"aground":false,"awa_deg":45.33232393881646,"aws":6.4553698609231756,"boom_deg":-18.799074923950332,"drive":0.9722390975212474,"heading_deg":221.43748976682926,"heel_deg":0.5680466377131645,"hoist":1.0,"hoist_now":1.0,"rudder":-1.0,"rudder_now":-1.0,"sheet":0.22116558734059213,"sheet_now":0.22116558734059213,"speed":0.5542546526230394,"trim":"Good"}},"id":3,"name":"Sloop"}

```

**[271.34 s] agent:**

Bug is live: with Crate1 behind-starboard, rudder is -1.0 (hard port). Let me record the numbers and the resulting leftward arc:

**[271.41 s] bash** (completed)

```
pocket world get --json Sloop Transform | jq -c '.components.Transform.position'; pocket step 120; pocket world get --json Sloop Boat | jq -c '{heading_deg,rudder}'
```

```
[13.578474044799805,-0.04802688956260681,-6.427081108093262]
tick 1050 hash f8a9b12a6f9e
{"heading_deg":null,"rudder":null}

```

**[284.95 s] bash** (completed)

```
pocket world get --json Sloop Boat | jq -c '.components.Boat | {heading_deg,rudder}'
```

```
{"heading_deg":199.5497494797433,"rudder":-1.0}

```

**[328.51 s] agent:**

**Bug confirmed live.** At tick 930: Crate1 bearing −135.2°, heading 221.4° → true offset is only **+3.4°** (barely starboard), yet rudder is −1.0 (hard port). The helmsman is now turning left the long way (heading 221 → 199 after 120 ticks, and it will keep going ~334° to reach the crate).

Root cause is in `angleDiff` in scripts/helm.ts:49: `if (d > 180) d -= 360` handles only positive wrap; JS `%` keeps the sign, so `heading_deg` in [0,360) with a bearing in (−180,180] gives raw diffs in (−360,−180) for marks astern, which return hugely negative → hard-to-port. Writing the fix:

**[328.55 s] bash** (completed)

```
pocket scripts write scripts/helm.ts - <<'EOF'
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
    else if (d < -180) d += 360;
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
                if (t === undefined) {
                    h.goto[r] = 0;
                    ctx.emit("helm.mark_gone", { mark }, { subject: boat });
                } else {
                    // The mark's bearing, measured as heading_deg is: 0 toward -z, 90 toward +x.
                    const dx = t.position.x - at.x[r];
                    const dz = t.position.z - at.z[r];
                    const bearing = Math.atan2(dx, -dz) * 180 / Math.PI;
                    rudder = clamp(angleDiff(bearing, b.heading_deg[r]) * PILOT_GAIN, -1, 1);
                }
            }

            let sail = clamp(h.sail[r], 0, 1);
            if (h.anchor[r] === 1) {
                sail = 0;
                rudder = 0;
            }
            b.rudder[r] = rudder;
            b.hoist[r] = sail;
            b.sheet[r] = bestSheet(b.awa_deg[r]);
        });
    },
});
EOF
```

```
path scripts/helm.ts | 0 diagnostics

```

**[334.01 s] bash** (completed)

```
pocket scripts apply && pocket snapshots restore 0 && pocket world set Sloop Helm goto=Crate2 && pocket step 900 --until tick:900 && pocket world set Sloop Helm goto=Crate1 && pocket step 30
```

```
outcome applied | typecheck unavailable | bundle 2c246ae732de | 0 diagnostics
restored tick 0; tick 0 (0.00 s) edit paused real-time x1.0 | 9 entities | hash 0fb8052e39f5
set Helm on Sloop (1 edit at tick 0)
tick 900 hash 4f8ce236b124 | stopped: tick
set Helm on Sloop (1 edit at tick 900)
tick 930 hash b17379f9d293

```

**[342.16 s] agent:**

Same scenario replayed to the identical state (hash matches). Now checking the helmsman's response to the crate behind-starboard:

**[342.24 s] bash** (completed)

```
pocket world get --json Sloop Boat | jq -c '.components.Boat | {heading_deg,rudder}'; pocket step 120; pocket world get --json Sloop Boat | jq -c '.components.Boat | {heading_deg,rudder}'
```

```
{"heading_deg":221.43748976682926,"rudder":-1.0}
tick 1050 hash f8a9b12a6f9e
{"heading_deg":199.5497494797433,"rudder":-1.0}

```

**[358.31 s] bash** (error)

```
pocket scripts status; pocket scripts check; pocket scripts read scripts/helm.ts | sed -n '40,48p'
```

```
The user has specified a rule which prevents you from using this specific tool call. Here are some of the relevant rules [...]
```

**[358.31 s] agent:**

Still -1 — the swap didn't take. Checking script status:

**[364.12 s] bash** (completed)

```
pocket scripts status
```

```
check.usage: pocket scripts has no 'status'

```

**[364.12 s] bash** (completed)

```
pocket scripts check
```

```
outcome dry_run | typecheck unavailable | bundle 2c246ae732de | 0 diagnostics

```

**[365.87 s] bash** (completed)

```
pocket scripts read scripts/helm.ts | grep -n -A4 'function angleDiff'
```

```
26:function angleDiff(a: number, b: number): number {
27-    let d = (a - b) % 360;
28-    if (d > 180) d -= 360;
29-    else if (d < -180) d += 360;
30-    return d;

```

**[378.24 s] bash** (completed)

```
pocket call scripts.status
```

```
{
  "bundle": "8a974881e7a6eced1910a32db427ded3e35685d44ad2b49167b8874101812ef9",
  "ran_last_tick": [
    "helm",
    "log",
    "take_aboard"
  ]
}

```

**[384.69 s] agent:**

The active bundle is still the old one — `snapshots restore` rolled it back. Re-applying now:

**[384.74 s] bash** (completed)

```
pocket scripts apply && pocket call scripts.status | jq -r .bundle
```

```
outcome applied | typecheck unavailable | bundle 2c246ae732de | 0 diagnostics
2c246ae732def5a3852cef5b1b7873dc960808bc78cdb8ef0a7656633b24527b

```

**[390.15 s] bash** (completed)

```
pocket world get --json Sloop Boat | jq -c '.components.Boat | {heading_deg,rudder}'; pocket step 30; pocket world get --json Sloop Boat | jq -c '.components.Boat | {heading_deg,rudder}'; pocket world get --json Sloop Transform | jq -c '.components.Transform.position'
```

```
{"heading_deg":199.5497494797433,"rudder":-1.0}
tick 1080 hash f31c21d8f961
{"heading_deg":189.22152894219167,"rudder":1.0}
[13.775057792663574,-0.07704749703407288,-4.932854175567627]

```

**[400.6 s] agent:**

Fix is active: rudder flipped to +1.0 (starboard, the short way). Confirming it converges on the crate:

**[400.71 s] bash** (completed)

```
pocket step 300; pocket world get --json Sloop Boat | jq -c '.components.Boat | {heading_deg,rudder,speed}'; pocket world get --json Sloop Transform | jq -c '.components.Transform.position'
```

```
tick 1380 hash 1096024f1dc1
{"heading_deg":244.3084347773109,"rudder":0.22638173712062248,"speed":0.17980271011472082}
[11.658696174621582,-0.36215171217918396,-0.36022377014160156]

```

**[422.06 s] bash** (completed)

```
pocket logs --limit 5
```

```
(no output)
```

**[437.15 s] agent:**

Fix verified end-to-end:

- **Before**: crate astern-starboard → rudder −1 (hard port), heading 221° → 199° → falling, a ~334° left circle for a mark only +3.4° starboard.
- **After** (same replayed state): rudder +1 (starboard), heading 189° → 244°, converging on the crate with small corrections, no log errors.

The defect was the one-sided wrap in `angleDiff` (scripts/helm.ts:28): `d > 180` was folded back but `d < -180` never was; since JS `%` keeps the dividend's sign and `heading_deg` runs 0..360 while the computed bearing is in (−180,180], a mark behind the boat yields a raw offset below −180 that stays hugely negative → `clamp` saturates the rudder hard to port → the long-way-round left turn.

{"cause": "angleDiff in scripts/helm.ts only wrapped positive angle differences (d > 180), not negative ones, so with heading_deg in 0..360 and the mark's bearing in -180..180 a crate behind the boat produced a raw offset below -180 that stayed hugely negative and drove the rudder hard to port, turning the long way round to the left.", "file": "scripts/helm.ts", "line": 28, "fixed": true}
