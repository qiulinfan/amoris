# goto-wrap, run cli-2

Model zai-coding-plan/glm-5.3-flash, via cli, prompt variant free. Prompt: [prompt.txt](prompt.txt).

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

**[4.49 s] bash** (completed)

```
pocket scripts list
```

```
scripts/components.ts 1389 B
scripts/helm.ts 3339 B
scripts/main.ts 398 B
scripts/rules.ts 3512 B

```

**[9.23 s] bash** (completed)

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

**[9.28 s] bash** (completed)

```
pocket scripts read scripts/components.ts; pocket scripts read scripts/rules.ts; pocket scripts read scripts/main.ts
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
        const b = bo
... (5299 chars)
```

**[80.34 s] bash** (completed)

```
pocket world query Boat,Helm,Transform --fields Boat.heading_deg,Boat.awa_deg,Helm.goto,Transform.position
```

```
#3 Sloop  Boat.awa_deg=0.0  Boat.heading_deg=0.0  Helm.goto=null  Transform.position=[0.0,0.0,0.0]

```

**[80.34 s] bash** (completed)

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

**[84.56 s] bash** (completed)

```
pocket world query Cargo,Transform --fields Transform.position --name
```

```
check.usage: --name needs a value
  did you mean: name

```

**[89.22 s] bash** (completed)

```
pocket world query Cargo,Transform --fields name,Transform.position
```

```
sim.component_unknown: There is no component 'name'; did you mean ["Name"]?

```

**[91.37 s] bash** (completed)

```
pocket world query Cargo,Transform,Name --fields Name.value,Transform.position
```

```
#4 Crate1  Name.value=null  Transform.position=[5.0,0.0,2.2]
#5 Crate2  Name.value=null  Transform.position=[11.0,0.0,-2.4]
#6 Crate3  Name.value=null  Transform.position=[18.0,0.0,2.6]
#7 Crate4  Name.value=null  Transform.position=[26.0,0.0,-6.0]

```

**[121.05 s] bash** (completed)

```
pocket world schema Transform; pocket world get Sloop
```

```
Transform (Engine v1) Where an entity is: its origin and orientation in the world (metres; a unit quaternion
`[x, y, z, w]`). Physics writes it back after every step for the bodies it moves; a write from
outside moves the body there at the next step.
  position?: [number]  The origin in the world, metres.
  rotation?: [number]  The orientation, a unit quaternion `[x, y, z, w]`.
#3 Sloop
  Boat: {"afloat":false,"aground":false,"awa_deg":0.0,"aws":0.0,"boom_deg":0.0,"drive":0.0,"heading_deg":0.0,"heel_deg":0.0,"hoist":1.0,"hoist_now":1.0,"rudder":0.0,"rudder_now":0.0,"sheet":1.0,"sheet_now":1.0,"speed":0.0,"trim":"Furled"}
  Collider: {"density":null,"friction":0.4,"restitution":0.1,"shape":{"Compound":{"parts":[{"position":[0.0,0.0,0.0],"rotation":[0.0,0.0,0.0,1.0],"shape":{"ConvexHull":{"points":[[-0.01,0.05,-1.75],[0.01,0.05,-1.75],[-0.03,0.4,-1.75],[0.03,0.4,-1.75],[-0.12,-0.2,-1.0],[0.12,-0.2,-1.0],[-0.3,0.36,-1.0],[0.3,0.36,-1.0]]}}},{"position":[0.0,0.0,0.0],"rotation":[0.0,0.0,0.0,1.0],"shape":{"ConvexHull":{"points":[[-0.12,-0.2,-1.0],[0.12,-0.2,-1.0],[-0.3,0.36,-1.0],[0.3,0.36,-1.0],[-0.18,-0.25,0.0],[0.18,-0.25,0.0],[-0.4,0.35,0.0],[0.4,0.35,0.0]]}}},{"position":[0.0,0.0,0.0],"rotation":[0.0,0.0,0.0,1.0],"shape":{"ConvexHull":{"points":[[-0.18,-0.25,0.0],[0.18,-0.25,0.0],[-0.4,0.35,0.0],[0.4,0.35,0.0],[-0.16,-0.22,1.0],[0.16,-0.22,1.0],[-0.38,0.35,1.0],[0.38,0.35,1.0]]}}},{"position":[0.0,0.0,0.0],"rotation":[0.0,0.0,0.0,1.0],"shape":{"ConvexHull":{"points":[[-0.16,-0.22,1.0],[0.16,-0.22,1.0],[-0.38,0.35,1.0],[0.38,0.35,1.0],[-0.12,-0.15,1.65],[0.12,-0.15,1.65],[-0.32,0.36,1.65],[0.32,0.36,1.65]]}}},{"position":[0.0,0.0,0.0],"rotation":[0.0,0.0,0.0,1.0],"shape":{"ConvexHull":{"points":[[-0.03,-0.24,-0.45],[0.03,-0.24,-0.45],[-0.03,-0.24,0.4],[0.03,-0.24,0.4],[-0.04,-0.85,-0.25],[0.04,-0.85,-0.25],[-0.04,-0.85,0.3],[0.04,-0.85,0.3]]}}},{"position":[0.0,0.0,0.0],"rotation":[0.0,0.0,0.0,1.0],"shape":{"ConvexHull":{"points":[[-0.015,0.0,1.66],[0.015,0.0,1.66],[-0.015,0.0,1.9],[0.015,0.0,1.9],[-0.015,-0.55,1.66],[0.015,-0.55,1.66],[-0.015,-0.55,1.86],[0.015,-0.55,1.86]]}}}]}}}
  Crew: {"take":null}
  ExternalForce: {"force":[0.0,0.0,0.0],"torque":[0.0,0.0,0.0]}
  Floater: {"drag":0.05,"heave":6.0,"points":[{"at":[-0.018055555555555557,0.07250000000000001,-1.625],"size":0.12833333333333333,"volume":0.0046375},{"at":[0.018055555555555554,0.07250000000000001,-1.625],"size":0.12833333333333333,"volume":0.0046375},{"at":[-0.025833333333333333,0.200833333333
... (11485 chars)
```

**[146.01 s] bash** (completed)

```
pocket step 1 && pocket world query Helm,Boat,Transform --fields Boat.heading_deg,Transform.position,Transform.rotation
```

```
tick 1 hash 646aa9a3436c
#3 Sloop  Boat.heading_deg=90.03761313838427  Transform.position=[0.0014278467278927565,-0.0004680156707763672,-0.0008669241797178984]  Transform.rotation=[-0.0012498028809204698,-0.7073377370834351,0.00030890406924299896,0.7068747282028198]

```

**[178.76 s] agent:**

Boat faces +x (heading 90°). Now I'll reproduce: turn the boat to heading ≈ 300°, then ask for Crate1 (bearing ≈ 114° from origin). Predicted defect in `angleDiff`: `(a-b) % 360` in JS keeps the sign, and only `d > 180` is corrected — `d = 113.8 − 300 = −186.2` stays negative → rudder −1 → turns left the long way.

**[178.87 s] bash** (completed)

```
pocket world set Sloop Transform rotation='[0,0.5,0,0.8660254]' && pocket step 1 && pocket world get Sloop Boat --fields heading_deg
```

```
set Transform on Sloop (1 edit at tick 1)
tick 2 hash 64d0203d8df1
#3 Sloop
  Boat: {"afloat":true,"aground":false,"awa_deg":-27.93250855003047,"aws":5.79216487364147,"boom_deg":27.93250855003047,"drive":0.0,"heading_deg":300.0600815180909,"heel_deg":0.2220359643389638,"hoist":1.0,"hoist_now":1.0,"rudder":0.0,"rudder_now":0.0,"sheet":1.0,"sheet_now":1.0,"speed":0.20053829056898284,"trim":"Luffing"}

```

**[186.13 s] agent:**

Heading = 300.06°, Crate1 bearing ≈ 113.8° → d ≈ −186.3°. Now steering for the crate:

**[186.17 s] bash** (completed)

```
pocket world set Sloop Helm goto=Crate1 && pocket step 40 --watch Sloop.Boat.heading_deg
```

```
set Helm on Sloop (1 edit at tick 2)
tick 3 hash 194718e02ebe | stopped: Boat.heading_deg 300.0600815180909 -> 300.12029735069746

```

**[189.66 s] bash** (completed)

```
pocket step 40 && pocket world get Sloop Boat --fields heading_deg,rudder_now
```

```
tick 43 hash bda477e49726
#3 Sloop
  Boat: {"afloat":true,"aground":false,"awa_deg":-33.570664588327574,"aws":5.567885514876424,"boom_deg":33.570664588327574,"drive":0.0,"heading_deg":299.64007529924714,"heel_deg":1.8788467726867273,"hoist":1.0,"hoist_now":1.0,"rudder":-1.0,"rudder_now":-1.0,"sheet":0.14586743762527762,"sheet_now":0.6583333333333345,"speed":-0.8386172896100603,"trim":"Luffing"}

```

**[249.92 s] agent:**

Bug reproduced: crate is 173.8° to starboard (short way = right), but `rudder=-1` and heading is decreasing — the helmsman turns left. The defect is `angleDiff` in scripts/helm.ts:28: JS `%` keeps the sign, so `d` in (−360,−180) is never wrapped to positive. Fixing:

**[249.94 s] bash** (completed)

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

**[254.38 s] bash** (completed)

```
pocket scripts apply
```

```
outcome applied | typecheck unavailable | bundle 2c246ae732de | 0 diagnostics

```

**[261.58 s] agent:**

Fix applied. Re-running the identical repro (heading 300°, goto Crate1 at bearing 113.8° — short way is 173.8° to starboard):

**[261.61 s] bash** (completed)

```
pocket world set Sloop Transform rotation='[0,0.5,0,0.8660254]' Helm goto=Crate1 && pocket step 40 && pocket world get Sloop Boat --fields heading_deg,rudder_now
```

```
check.usage: 'Helm' is not field=value

```

**[267.44 s] bash** (completed)

```
pocket world edit '[{"op":"set","entity":"Sloop","component":"Transform","fields":{"rotation":[0,0.5,0,0.8660254]}},{"op":"set","entity":"Sloop","component":"Helm","fields":{"goto":"Crate1"}}]' && pocket step 40 && pocket world get Sloop Boat --fields heading_deg,rudder_now
```

```
request.misplaced_field: 'component' does not go here (/ops/0/component); it belongs at /ops/0/set/component.

```

**[270.37 s] bash** (completed)

```
pocket world edit '[{"op":"set","entity":"Sloop","set":{"component":"Transform","fields":{"rotation":[0,0.5,0,0.8660254]}}},{"op":"set","entity":"Sloop","set":{"component":"Helm","fields":{"goto":"Crate1"}}}]' && pocket step 40 && pocket world get Sloop Boat --fields heading_deg,rudder_now
```

```
request.misplaced_field: 'entity' does not go here (/ops/0/entity); it belongs at /ops/0/remove/entity.

```

**[274.77 s] bash** (completed)

```
pocket help world.edit
```

```
world.edit (write): Spawn, set (fields merge), remove and destroy, all or nothing at one boundary; undoable.
  label?: string  The history's label; default: a summary of the edits.
  ops: [{spawn}|{set}|{remove}|{destroy}]  1 to 64 edits, applied in order at one boundary, all or none.

```

**[279.69 s] bash** (completed)

```
pocket help world_edit
```

```
world_edit (write): world.edit's canonical recorded form (edits tagged by op; revive restores a destroyed id).
  edits: [object|object|object|object|object]  1 to 64 edits, applied in order at one boundary.

```

**[286.26 s] bash** (completed)

```
pocket world set Sloop Transform rotation='[0,0.5,0,0.8660254]'; pocket world set Sloop Helm goto=Crate1; pocket step 40 && pocket world get Sloop Boat --fields heading_deg,rudder_now
```

```
set Transform on Sloop (1 edit at tick 43)
set Helm on Sloop (1 edit at tick 43)
tick 83 hash 0ba6e59afe55
#3 Sloop
  Boat: {"afloat":true,"aground":false,"awa_deg":-31.22782936190192,"aws":6.069388603405063,"boom_deg":27.625000000000107,"drive":0.08020380105989608,"heading_deg":300.63603038712546,"heel_deg":-0.9401856674943863,"hoist":1.0,"hoist_now":1.0,"rudder":1.0,"rudder_now":0.3333333333333331,"sheet":0.13317352742702165,"sheet_now":0.32500000000000123,"speed":0.24139419492540493,"trim":"Good"}

```

**[295.44 s] agent:**

`rudder=1.0` now (hard to starboard), heading rising. Confirming it converges on the crate's bearing:

**[295.51 s] bash** (completed)

```
pocket step 200 && pocket world get Sloop Boat --fields heading_deg,rudder_now; pocket world query Boat,Transform,Name --fields Name.value,Boat.heading_deg,Transform.position
```

```
tick 283 hash 0335b57861d9
#3 Sloop
  Boat: {"afloat":true,"aground":false,"awa_deg":-30.94958618157081,"aws":5.749500812832444,"boom_deg":11.207026238188973,"drive":0.9969854583805567,"heading_deg":302.7317374749054,"heel_deg":-6.032027183853307,"hoist":1.0,"hoist_now":1.0,"rudder":1.0,"rudder_now":1.0,"sheet":0.13184736750810555,"sheet_now":0.13184736750810555,"speed":-0.26123026346291783,"trim":"Good"}
#3 Sloop  Boat.heading_deg=302.7317374749054  Name.value=null  Transform.position=[0.07977345585823059,-0.04424256086349487,0.0575307160615921]

```

**[322.56 s] agent:**

Directional fix confirmed (rudder flipped from −1 to +1 for the identical situation; the boat now turns the short way — it converges slowly since the sim is nearly becalmed, but the steering is correct). Now restoring the edit world by undoing my test edits:

**[322.59 s] bash** (completed)

```
pocket history
```

```
undo: set Transform on Sloop | set Helm on Sloop | set Transform on Sloop | set Helm on Sloop
redo: 

```

**[327.62 s] bash** (completed)

```
pocket undo && pocket undo && pocket undo && pocket undo && pocket world get Sloop Helm,Boat,Transform --fields Helm.goto,Boat.heading_deg,Transform.rotation,Transform.position
```

```
undone: set Helm on Sloop (1 edit at tick 283)
undone: set Transform on Sloop (1 edit at tick 283)
undone: set Helm on Sloop (1 edit at tick 283)
undone: set Transform on Sloop (1 edit at tick 283)
sim.component_unknown: There is no component 'Helm,Boat,Transform'; did you mean []?

```

**[330.24 s] bash** (completed)

```
pocket world get Sloop Helm Boat Transform --fields Helm.goto,Boat.heading_deg,Transform.rotation,Transform.position
```

```
#3 Sloop
  Boat: {"afloat":true,"aground":false,"awa_deg":-30.94958618157081,"aws":5.749500812832444,"boom_deg":11.207026238188973,"drive":0.9969854583805567,"heading_deg":302.7317374749054,"heel_deg":-6.032027183853307,"hoist":1.0,"hoist_now":1.0,"rudder":1.0,"rudder_now":1.0,"sheet":0.13184736750810555,"sheet_now":0.13184736750810555,"speed":-0.26123026346291783,"trim":"Good"}
  Helm: {"anchor":false,"goto":null,"sail":1.0,"steer":0.0}
  Transform: {"position":[0.0014278467278927565,-0.0004680156707763672,-0.0008669241797178984],"rotation":[-0.0012498028809204698,-0.7073377370834351,0.00030890406924299896,0.7068747282028198]}

```

**[339.0 s] agent:**

Controls restored. Fully rewinding the sim state to tick 1 for cleanliness:

**[339.05 s] bash** (completed)

```
pocket snapshots list | head -5; pocket snapshots restore 1 2>&1 | head -2; pocket status
```

```
kept (every 60 ticks): 0 60 120 180 240
restored tick 0; tick 0 (0.00 s) edit paused real-time x1.0 | 9 entities | hash 0fb8052e39f5
tick 0 (0.00 s) edit paused real-time x1.0 | 9 entities | hash 0fb8052e39f5

```

**[344.25 s] bash** (completed)

```
pocket scripts check && pocket world get Sloop Boat --fields heading_deg
```

```
outcome dry_run | typecheck unavailable | bundle 2c246ae732de | 0 diagnostics
#3 Sloop
  Boat: {"afloat":false,"aground":false,"awa_deg":0.0,"aws":0.0,"boom_deg":0.0,"drive":0.0,"heading_deg":0.0,"heel_deg":0.0,"hoist":1.0,"hoist_now":1.0,"rudder":0.0,"rudder_now":0.0,"sheet":1.0,"sheet_now":1.0,"speed":0.0,"trim":"Furled"}

```

**[355.17 s] agent:**

Done. Summary:

- **Cause**: In `angleDiff` (scripts/helm.ts:28), `(a-b) % 360` in JS keeps the dividend's sign, but only `d > 180` was wrapped — a mark whose bearing is more than 180° behind the read heading (e.g. bearing 113.8° at heading 300° → d = −186.3°) yields a negative error, so the helmsman puts the rudder hard to port and circles left the long way round.
- **Fix**: added the missing negative wrap (`else if (d < -180) d += 360;`), applied via `pocket scripts apply` (bundle `2c246ae732de`, 0 diagnostics).
- **Verified in-game**: identical setup (heading ≈ 300°, `goto=Crate1`) gave `rudder=-1`, heading falling before the fix; `rudder=+1`, heading rising toward the crate's bearing after it. Test edits were undone and the world restored to tick 0.

{"cause": "The helmsman's bearing-error wrap in angleDiff only corrected d > 180 but not d < -180 (JS % keeps the sign), so a mark behind to starboard produced a negative error and hard-port rudder, circling left the long way round.", "file": "scripts/helm.ts", "line": 28, "fixed": true}
