// The host's as-built forms the editor reads (editor.md 8, items 8 and 12): `events.why` as
// `{event, causes, complete}`, the status's `pacing`, enum values in the registry's own spelling;
// and the mock answering the same forms as the host, so what passes against it means something.
import { expect, test } from "bun:test";
import { Lightbulb, Sun } from "lucide-react";
import { Session, type GameEvent } from "../mock/session";
import { allComponents, componentInfo } from "../mock/schemas";
import { MockError } from "../mock/util";
import type { ComponentInfo, HostWhy, Status } from "../src/host/protocol";
import { enumValue, sameVariant, searchComponents } from "../src/host/schema";
import { entityKind } from "../src/ui/icons";

// The editor's host client is made when `api` loads, from the page's location; nothing connects.
const page = (globalThis as { window?: object }).window ?? {};
Object.defineProperty(globalThis, "window", { configurable: true, value: { ...page, location: new URL("http://127.0.0.1:7878/") } });
const { causeChain, speedOf } = await import("../src/host/api");

const event = (seq: number, name: string, cause: number | null): GameEvent => ({ seq, id: seq, tick: seq, name, subject: null, data: {}, cause });

test("events.why's causes, nearest first, become a chain that ends with the event", () => {
  const knock = event(1, "probe.knock", null);
  const ring = event(2, "probe.ring", 1);
  const echo = event(3, "probe.echo", 2);
  const why: HostWhy = { event: echo, causes: [ring, knock], complete: true };
  expect(causeChain(why)).toEqual({ chain: [knock, ring, echo], complete: true });
  // The host's own array is left as it came.
  expect(why.causes).toEqual([ring, knock]);
  // A cause that left the ring: the chain is what the host still held, and says so.
  expect(causeChain({ event: echo, causes: [], complete: false })).toEqual({ chain: [echo], complete: false });
});

test("the speed is the status pacing's, and a stepped world has none", () => {
  const status = (pacing: Status["pacing"]): Status => ({ tick: 0, t_s: 0, paused: true, pacing, mode: "edit", world_hash: "", entities: 0 });
  expect(speedOf(status({ real_time: { speed: 2 } }))).toBe(2);
  expect(speedOf(status("stepped"))).toBeNull();
  expect(speedOf(null)).toBeNull();
  expect(speedOf(undefined)).toBeNull();
});

test("an enum's value is read in any spelling and sent in the registry's own", () => {
  expect(sameVariant("directional", "Directional")).toBe(true);
  expect(sameVariant("Directional", "directional")).toBe(true);
  expect(sameVariant("real_time", "RealTime")).toBe(true);
  expect(sameVariant("point", "directional")).toBe(false);
  expect(sameVariant(undefined, "directional")).toBe(false);
  const light = componentInfo("Light") as ComponentInfo;
  expect(enumValue(light, "kind", "point")).toBe("point");
  expect(enumValue(light, "kind", "Directional")).toBe("directional");
  // A registry that spells its values in PascalCase gets them back so.
  const pascal: ComponentInfo = {
    ...light,
    schema: { ...light.schema, $defs: { LightKind: { oneOf: ["Directional", "Point", "Spot"].map((v) => ({ type: "string", const: v })) } } },
  };
  expect(enumValue(pascal, "kind", "point")).toBe("Point");
  expect(enumValue(light, "kind", "area")).toBeNull();
  expect(enumValue(light, "colour", "point")).toBeNull();
  expect(enumValue(undefined, "kind", "point")).toBeNull();
  // A registry enum (project components) and an externally tagged one: its unit variants only.
  const rigid = componentInfo("RigidBody") as ComponentInfo;
  expect(enumValue(rigid, "kind", "fixed")).toBe("Fixed");
  expect(enumValue(componentInfo("Collider") as ComponentInfo, "shape", "ball")).toBeNull();
});

test("a light's icon follows its kind in the host's spelling", () => {
  expect(entityKind(["Light", "Transform"], "directional").icon).toBe(Sun);
  expect(entityKind(["Light", "Transform"], "Directional").icon).toBe(Sun);
  expect(entityKind(["Light", "Transform"], "point").icon).toBe(Lightbulb);
  expect(entityKind(["Light", "Transform"], undefined).icon).toBe(Lightbulb);
});

test("a component search lists names before docs that only mention the text", () => {
  const list = [
    { name: "Environment", doc: "The world's sky, ambient light, fog and image settings." },
    { name: "Highlight", doc: "Marks an entity." },
    { name: "Light", doc: "A light at the entity's Transform." },
    { name: "LightProbe", doc: "Samples what reaches a point." },
    { name: "Transform", doc: "Where an entity is." },
  ];
  expect(searchComponents(list, "Light").map((c) => c.name)).toEqual(["Light", "LightProbe", "Highlight", "Environment"]);
  expect(searchComponents(list, " light ").map((c) => c.name)[0]).toBe("Light");
  expect(searchComponents(list, "")).toBe(list);
  expect(searchComponents(list, "zzz")).toEqual([]);
  // The mock's registry: Light is the first answer to its own name.
  expect(searchComponents(allComponents(), "light")[0]?.name).toBe("Light");
});

function refusal(run: () => unknown): MockError {
  try {
    run();
  } catch (e) {
    if (e instanceof MockError) return e;
    throw e;
  }
  throw new Error("the call was not refused");
}

test("the mock's pacing is the host's: one per world, Play at the speed asked or 1", () => {
  const s = new Session(() => {});
  const status = s.call("time.control", {}) as Status & { speed?: number };
  expect(status.pacing).toEqual({ real_time: { speed: 1 } });
  expect("speed" in status).toBe(false);
  expect(speedOf(s.call("time.control", { speed: 2 }) as Status)).toBe(2);
  // Play starts at 1 whatever the edit world's speed, unless the call names one.
  let play = s.call("play.start", {}) as Status;
  expect([play.mode, speedOf(play), play.paused]).toEqual(["play", 1, false]);
  expect(speedOf(s.call("time.control", { speed: 0.5 }) as Status)).toBe(0.5);
  // Stop shows the edit world's pacing again.
  expect(speedOf(s.call("play.stop", {}) as Status)).toBe(2);
  play = s.call("play.start", { speed: 4, paused: true }) as Status;
  expect([speedOf(play), play.paused]).toEqual([4, true]);
  s.call("play.stop", {});
  // `pacing` itself: stepped, or real time at a speed.
  expect((s.call("time.control", { pacing: "stepped" }) as Status).pacing).toBe("stepped");
  expect(speedOf(s.call("time.control", { pacing: "stepped" }) as Status)).toBeNull();
  expect((s.call("time.control", { pacing: { real_time: { speed: 3 } } }) as Status).pacing).toEqual({ real_time: { speed: 3 } });
  // Refusals in the host's words; nothing changes.
  const zero = refusal(() => s.call("time.control", { speed: 0 }));
  expect(zero.code).toBe("request.out_of_range");
  expect(zero.message).toBe("'speed' must be above 0 and at most 1000; got 0.");
  expect(zero.detail).toEqual({ got: 0, max: 1000, min_exclusive: 0, path: "/pacing/real_time/speed" });
  expect(refusal(() => s.call("time.control", { speed: 2000 })).code).toBe("request.out_of_range");
  expect(refusal(() => s.call("play.start", { speed: 0 })).code).toBe("request.out_of_range");
  expect(s.mode).toBe("edit");
  const fast = refusal(() => s.call("time.control", { pacing: "fast" }));
  expect(fast.code).toBe("request.invalid_value");
  expect(fast.detail).toMatchObject({ allowed: ["stepped"], got: "fast", path: "/pacing" });
  expect((s.call("time.control", {}) as Status).pacing).toEqual({ real_time: { speed: 3 } });
});

test("a stepped Play world runs only when stepped", () => {
  const s = new Session(() => {});
  s.call("play.start", {});
  s.call("time.control", { pacing: "stepped" });
  for (let i = 0; i < 10; i++) s.frame();
  expect(s.tick).toBe(0);
  s.call("time.step", { ticks: 3 });
  expect(s.tick).toBe(3);
  s.call("time.control", { speed: 4 });
  s.frame();
  expect(s.tick).toBe(7);
});

test("the mock's events are the host's: id beside seq, since as {events, last, missed}, why as {event, causes, complete}", () => {
  const s = new Session(() => {});
  expect(s.call("events.since", { seq: 0 })).toEqual({ events: [], last: 0, missed: 0 });
  s.call("play.start", {});
  for (let i = 0; i < 20000 && !s.events.some((e) => e.cause !== null); i++) s.stepOnce();
  const caused = s.events.find((e) => e.cause !== null)!;
  expect(caused).toBeDefined();
  const since = s.call("events.since", { seq: 0, limit: 5000 }) as { events: GameEvent[]; last: number; missed: number };
  expect(Object.keys(since).sort()).toEqual(["events", "last", "missed"]);
  expect(since.last).toBe(s.events.at(-1)!.seq);
  expect(since.missed).toBe(0);
  expect(since.events.every((e) => typeof e.id === "number" && typeof e.seq === "number")).toBe(true);
  // Without `seq`: the newest `limit`; after one: the events past it, and `last` for the next call.
  const newest = s.call("events.since", { limit: 1 }) as { events: GameEvent[]; last: number };
  expect(newest.events.map((e) => e.seq)).toEqual([since.last]);
  expect(s.call("events.since", { seq: since.last })).toEqual({ events: [], last: since.last, missed: 0 });
  const why = s.call("events.why", { seq: caused.seq }) as HostWhy;
  expect(Object.keys(why).sort()).toEqual(["causes", "complete", "event"]);
  expect(why.event.seq).toBe(caused.seq);
  expect(why.complete).toBe(true);
  // Nearest first: the first cause is the one the event names by its id.
  expect(why.causes[0]!.id).toBe(caused.cause!);
  const chain = causeChain(why).chain;
  expect(chain.length).toBeGreaterThanOrEqual(2);
  expect(chain.at(-1)!.seq).toBe(caused.seq);
  expect(chain[0]!.cause).toBeNull();
  const missing = refusal(() => s.call("events.why", { seq: 10_000_000 }));
  expect(missing.code).toBe("events.not_found");
});

test("the mock's world.get lists Name, as the host's does", () => {
  const s = new Session(() => {});
  const sun = s.call("world.get", { entity: "Sun" }) as { name: string; components: Record<string, unknown> };
  expect(Object.keys(sun.components)).toEqual(["Light", "Name", "Transform"]);
  expect(sun.components.Name).toBe("Sun");
  expect((s.call("world.get", { entity: "Sun", components: ["Name"] }) as typeof sun).components).toEqual({ Name: "Sun" });
  expect(Object.keys((s.call("world.get", { entity: "Sun", components: ["Light"] }) as typeof sun).components)).toEqual(["Light"]);
});
