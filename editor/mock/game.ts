// The mock's game: a few lines standing in for pocket-physics (the boat, the floating crates) and
// for the sailing sample's systems (samples/sailing/scripts/rules.ts: muster, log, take_aboard),
// plus a helmsman that steers the sloop from crate to crate so a Play session shows something.
// Script systems are stepped one at a time so the debugger can pause between and inside them.

import type { World, Entity, Changes } from "./world";
import { yaw, type Q4, type V3 } from "./scene";

export const DT = 1 / 60;
const DEG = Math.PI / 180;
const REACH = 3;
const REACH_UP = 3;

export interface Emitted {
  name: string;
  subject: number | null;
  data: Record<string, unknown>;
  causeName?: string; // resolved to a seq by the session: the last event of that name
}

export interface LogLine {
  level: "error" | "warn" | "info" | "debug";
  source: string;
  message: string;
  file?: string;
  line?: number;
}

export interface TickOut {
  events: Emitted[];
  logs: LogLine[];
}

export interface ScriptSystem {
  name: string;
  when: "start" | "always";
  run(ctx: GameCtx): void;
}

export interface GameCtx {
  world: World;
  tick: number;
  t: number;
  out: TickOut;
  changes: Changes;
  /** Line of a marker string in rules.ts, for log locations. */
  lineOf(marker: string): number | undefined;
}

function bearing(deg: number): V3 {
  return [Math.sin(deg * DEG), 0, -Math.cos(deg * DEG)];
}

function angleDiff(a: number, b: number): number {
  return ((a - b + 540) % 360) - 180;
}

function mulQ(a: Q4, b: Q4): Q4 {
  const [ax, ay, az, aw] = a;
  const [bx, by, bz, bw] = b;
  return [
    aw * bx + ax * bw + ay * bz - az * by,
    aw * by - ax * bz + ay * bw + az * bx,
    aw * bz + ax * by - ay * bx + az * bw,
    aw * bw - ax * bx - ay * by - az * bz,
  ];
}

export function seaHeight(world: World, x: number, z: number, t: number): number {
  const sea = [...world.entities.values()].find((e) => e.components.Sea)?.components.Sea as
    | { level: number; waves: { length: number; height: number; toward_deg: number; phase: number }[] }
    | undefined;
  if (!sea) return 0;
  let h = sea.level;
  for (const w of sea.waves) {
    const k = (2 * Math.PI) / Math.max(0.1, w.length);
    const omega = Math.sqrt(9.81 * k);
    const d = bearing(w.toward_deg);
    h += (w.height / 2) * Math.sin(k * (d[0] * x + d[2] * z) - omega * t + w.phase);
  }
  return h;
}

const touch = (ctx: GameCtx, e: Entity, c: string) => ctx.changes.changed.add(`${e.id}|${c}`);

/** pocket-physics, much simplified: actuators, a polar of the sail, the hull, the floating crates. */
export function engineStep(ctx: GameCtx) {
  const { world, t } = ctx;
  const wind = [...world.entities.values()].find((e) => e.components.Wind)?.components.Wind as
    | { from_deg: number; speed: number }
    | undefined;
  const windToward = (wind?.from_deg ?? 270) + 180;
  const windSpeed = wind?.speed ?? 0;
  for (const e of world.entities.values()) {
    const boat = e.components.Boat as Record<string, number | boolean | string> | undefined;
    const tr = e.components.Transform as { position: V3; rotation: Q4 } | undefined;
    if (boat && tr) {
      const follow = (now: string, ctl: string, rate: number) => {
        const d = (boat[ctl] as number) - (boat[now] as number);
        boat[now] = (boat[now] as number) + Math.sign(d) * Math.min(Math.abs(d), rate * DT);
      };
      follow("hoist_now", "hoist", 0.5);
      follow("sheet_now", "sheet", 0.8);
      follow("rudder_now", "rudder", 2);
      const heading = boat.heading_deg as number;
      const twa = angleDiff(windToward + 180, heading); // where the wind comes from, off the bow
      const off = Math.abs(twa);
      // A forgiving polar: the mock beats slowly to windward instead of stopping in irons.
      const polar = off < 35 ? 0.3 : off < 60 ? 0.6 : off < 110 ? 0.95 : off < 150 ? 0.85 : 0.7;
      const drive = (boat.hoist_now as number) * polar;
      const target = drive * windSpeed * 0.5;
      const speed = (boat.speed as number) + (target - (boat.speed as number)) * Math.min(1, DT * 0.6);
      const turn = (boat.rudder_now as number) * Math.max(0.4, Math.min(1, Math.abs(speed) / 1.5)) * 28 * DT;
      const nextHeading = (heading + turn + 360) % 360;
      const fwd = bearing(nextHeading);
      const p = tr.position;
      const nx = p[0] + fwd[0] * speed * DT;
      const nz = p[2] + fwd[2] * speed * DT;
      const ny = seaHeight(world, nx, nz, t) * 0.6;
      const heel = (boat.hoist_now as number) * Math.sin(twa * DEG) * 9 * Math.min(1, speed / 2);
      tr.position = [nx, ny, nz];
      const roll: Q4 = [0, 0, Math.sin((-heel * DEG) / 2), Math.cos((-heel * DEG) / 2)];
      tr.rotation = mulQ(yaw(-nextHeading), roll);
      Object.assign(boat, {
        speed,
        heading_deg: nextHeading,
        heel_deg: heel,
        afloat: true,
        awa_deg: twa,
        aws: Math.max(0, windSpeed - speed * Math.cos(twa * DEG)),
        boom_deg: (boat.sheet_now as number) * 75 * (twa >= 0 ? -1 : 1),
        drive: polar,
        trim: (boat.hoist_now as number) < 0.05 ? "Furled" : polar < 0.3 ? "Luffing" : "Good",
      });
      const v = e.components.Velocity as { linear: V3 } | undefined;
      if (v) v.linear = [fwd[0] * speed, 0, fwd[2] * speed];
      touch(ctx, e, "Boat");
      touch(ctx, e, "Transform");
      if (v) touch(ctx, e, "Velocity");
      continue;
    }
    if (e.components.Cargo && tr) {
      const p = tr.position;
      const phase = e.id * 1.7;
      tr.position = [p[0] + 0.02 * DT * Math.sin(t * 0.3 + phase), seaHeight(world, p[0], p[2], t) * 0.8, p[2]];
      const rock = 4 * Math.sin(t * 1.3 + phase) * DEG;
      const base = Math.atan2(tr.rotation[1], tr.rotation[3]) * 2;
      tr.rotation = mulQ([0, Math.sin(base / 2), 0, Math.cos(base / 2)], [Math.sin(rock / 2), 0, 0, Math.cos(rock / 2)]);
      touch(ctx, e, "Transform");
    }
  }
}

/** The helmsman: steers for the nearest crate and orders the crew to take it when near. */
export function helmsman(ctx: GameCtx) {
  const { world } = ctx;
  const crates = [...world.entities.values()].filter((e) => e.components.Cargo && e.components.Transform);
  for (const e of world.entities.values()) {
    const boat = e.components.Boat as Record<string, number> | undefined;
    const crew = e.components.Crew as { take: number | null } | undefined;
    const tr = e.components.Transform as { position: V3 } | undefined;
    if (!boat || !tr) continue;
    const p = tr.position;
    let goal: Entity | undefined;
    let best = Infinity;
    for (const c of crates) {
      const q = (c.components.Transform as { position: V3 }).position;
      const d = Math.hypot(q[0] - p[0], q[2] - p[2]);
      if (d < best) {
        best = d;
        goal = c;
      }
    }
    if (!goal) {
      goal = [...world.entities.values()].find((x) => x.name === "Finish");
      if (goal && boat.hoist > 0) {
        const q = (goal.components.Transform as { position: V3 }).position;
        if (Math.hypot(q[0] - p[0], q[2] - p[2]) < 4) {
          boat.hoist = 0;
          touch(ctx, e, "Boat");
        }
      }
    }
    if (!goal) continue;
    const q = (goal.components.Transform as { position: V3 }).position;
    const want = Math.atan2(q[0] - p[0], -(q[2] - p[2])) / DEG;
    const err = angleDiff(want, boat.heading_deg);
    const rudder = Math.max(-1, Math.min(1, err / 25));
    // Ease the sail for a sharp turn near the target so the boat does not circle it.
    const hoist = goal.components.Cargo ? (best < 10 && Math.abs(err) > 50 ? 0.55 : 1) : boat.hoist;
    if (Math.abs(rudder - boat.rudder) > 0.01 || hoist !== boat.hoist) {
      boat.rudder = rudder;
      boat.hoist = hoist;
      touch(ctx, e, "Boat");
    }
    if (crew && goal.components.Cargo && best < REACH && crew.take === null) {
      crew.take = goal.id;
      touch(ctx, e, "Crew");
      ctx.out.events.push({ name: "crew.order", subject: e.id, data: { take: goal.id, range_m: Math.round(best * 10) / 10 } });
    }
  }
}

export const SYSTEMS: ScriptSystem[] = [
  {
    name: "muster",
    when: "start",
    run(ctx) {
      const crates = [...ctx.world.entities.values()].filter((e) => e.components.Cargo).length;
      for (const e of ctx.world.entities.values()) {
        const tally = e.components.Tally as { total: number } | undefined;
        if (!tally) continue;
        tally.total = crates;
        touch(ctx, e, "Tally");
        ctx.out.logs.push({
          level: "info",
          source: "script:muster",
          message: `${e.name} musters ${crates} crates adrift`,
          file: "scripts/rules.ts",
          line: ctx.lineOf("boats.each((_r, e) => ctx.world.set(e, \"Tally\""),
        });
      }
    },
  },
  {
    name: "log",
    when: "always",
    run(ctx) {
      for (const e of ctx.world.entities.values()) {
        const boat = e.components.Boat as { speed: number; hoist_now: number } | undefined;
        const log = e.components.Log as { distance: number; top_speed: number; sail_set: boolean } | undefined;
        if (!boat || !log) continue;
        const speed = Math.abs(boat.speed);
        log.distance += speed * DT;
        log.top_speed = Math.max(log.top_speed, speed);
        const set = boat.hoist_now >= 0.5;
        if (set !== log.sail_set) {
          ctx.out.events.push({ name: set ? "sail.set" : "sail.furled", subject: e.id, data: { tick: ctx.tick } });
          ctx.out.logs.push({
            level: "info",
            source: "script:log",
            message: `${e.name}: sail ${set ? "set" : "furled"} at tick ${ctx.tick}`,
            file: "scripts/rules.ts",
            line: ctx.lineOf('ctx.emit(set ? "sail.set"'),
          });
          log.sail_set = set;
        }
        touch(ctx, e, "Log");
      }
    },
  },
  {
    name: "take_aboard",
    when: "always",
    run(ctx) {
      const w = ctx.world;
      for (const e of [...w.entities.values()]) {
        const crew = e.components.Crew as { take: number | null } | undefined;
        const tally = e.components.Tally as { taken: number; worth: number; total: number } | undefined;
        const tr = e.components.Transform as { position: V3 } | undefined;
        if (!crew || !tally || !tr || crew.take === null || crew.take === 0) continue;
        const target = crew.take;
        crew.take = null;
        touch(ctx, e, "Crew");
        const crate = w.entities.get(target);
        if (!crate || !crate.components.Cargo) {
          ctx.out.events.push({ name: "interact.ignored", subject: e.id, data: { code: "sail.crate_gone", crate: target }, causeName: "crew.order" });
          ctx.out.logs.push({ level: "warn", source: "script:take_aboard", message: `crate ${target} is gone`, file: "scripts/rules.ts", line: ctx.lineOf("sail.crate_gone") });
          continue;
        }
        const p = (crate.components.Transform as { position: V3 }).position;
        const across = Math.hypot(p[0] - tr.position[0], p[2] - tr.position[2]);
        if (across > REACH || Math.abs(p[1] - tr.position[1]) > REACH_UP) {
          ctx.out.events.push({ name: "interact.ignored", subject: e.id, data: { code: "sail.out_of_reach", crate: target, range_m: Math.round(across * 10) / 10 }, causeName: "crew.order" });
          ctx.out.logs.push({ level: "warn", source: "script:take_aboard", message: `${crate.name} is out of reach (${across.toFixed(1)} m)`, file: "scripts/rules.ts", line: ctx.lineOf("sail.out_of_reach") });
          continue;
        }
        const value = (crate.components.Cargo as { value: number }).value;
        w.entities.delete(target);
        ctx.changes.despawned.add(target);
        tally.taken += 1;
        tally.worth += value;
        touch(ctx, e, "Tally");
        const left = tally.total - tally.taken;
        ctx.out.events.push({ name: "crate.taken", subject: e.id, data: { crate: target, taken: tally.taken, left }, causeName: "crew.order" });
        ctx.out.logs.push({
          level: "info",
          source: "script:take_aboard",
          message: `${crate.name} aboard (worth ${value}); ${left} left`,
          file: "scripts/rules.ts",
          line: ctx.lineOf('ctx.emit("crate.taken"'),
        });
        if (left === 0) {
          ctx.out.events.push({ name: "crates.all", subject: e.id, data: { taken: tally.taken }, causeName: "crate.taken" });
          ctx.out.logs.push({ level: "info", source: "script:take_aboard", message: `All ${tally.taken} crates aboard, worth ${tally.worth}`, file: "scripts/rules.ts", line: ctx.lineOf('ctx.emit("crates.all"') });
        }
      }
    },
  },
];
