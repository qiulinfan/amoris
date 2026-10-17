// The history with sources (editor.md 8, items 1 and 5): the store reads the host's entries (or
// makes them from the labels of an older host) with the next redo first, marks what another client
// made, and a streamed drag is one entry that Escape takes back with an undo of its group, also
// when an agent's edit splits it or comes after it. The drag runs against the mock's world (the
// same merging and the same conditional undo as the host's history.rs).
import { beforeEach, expect, test } from "bun:test";
import { EDITOR, emptyChanges, type Caller } from "../mock/world";
import { sailingWorld } from "../mock/scene";
import { MockError } from "../mock/util";
import type { EditOp, EntityData } from "../src/host/protocol";
import { agentOf, historyEntries, useHistory } from "../src/state/history";

// The editor's host client is made when `api` loads, from the page's location; nothing connects.
const page = (globalThis as { window?: object }).window ?? {};
Object.defineProperty(globalThis, "window", { configurable: true, value: { ...page, location: new URL("http://127.0.0.1:7878/") } });
const { LiveEdit } = await import("../src/actions/world");
const { api } = await import("../src/host/api");
const { HostError } = await import("../src/host/client");
const { useConnection } = await import("../src/state/connection");
const { useWorld } = await import("../src/state/world");

test("entries come from the host, or from the labels of a host without them", () => {
  // The host lists each stack most recent last: "cli edit" is the next to redo.
  const h = {
    undo: ["Move Sloop", "set Cargo on Crate1"],
    redo: ["Rename Crate2", "cli edit"],
    entries: {
      undo: [
        { label: "Move Sloop", source: "editor" as const, group: "drag-1" },
        { label: "set Cargo on Crate1", source: { developer: 1 }, session: "mcp-1" },
      ],
      redo: [
        { label: "Rename Crate2", source: "editor" as const },
        { label: "cli edit", source: { developer: 0 }, session: "api" },
      ],
    },
  };
  useHistory.getState().set(h);
  expect(useHistory.getState().undo.map(agentOf)).toEqual([null, "mcp-1"]);
  expect(useHistory.getState().redo.map((e) => e.label)).toEqual(["cli edit", "Rename Crate2"]);
  expect(useHistory.getState().redo.map(agentOf)).toEqual(["api", null]);
  expect(agentOf({ label: "x", source: { developer: 4 } })).toBe("developer 4");
  // An older host: labels only; agents' labels started with "agent:".
  const old = historyEntries({ undo: ["Move Sloop", "agent: move Crate4"], redo: ["b", "a"] });
  expect(old.undo.map(agentOf)).toEqual([null, "agent"]);
  expect(old.redo.map((e) => e.label)).toEqual(["a", "b"]);
});

/** The editor's `api` answered by the mock's world, as the editor's client or as `as`. */
function wire(as: { caller: Caller }) {
  const world = sailingWorld();
  const a = api as unknown as {
    world: Record<string, unknown>;
    history: Record<string, unknown>;
  };
  a.world.edit = async (ops: EditOp[], label?: string, group?: string) =>
    world.edit(ops, label, group, emptyChanges(), as.caller);
  a.world.get = async (id: number): Promise<EntityData> => world.get(id) as EntityData;
  a.history.list = async () => world.history();
  a.history.undo = async (group?: string) => {
    try {
      return { label: world.undo(emptyChanges(), group) };
    } catch (e) {
      throw e instanceof MockError ? new HostError(e.toWire(), "history.undo") : e;
    }
  };
  return world;
}

const sloopX = (w: ReturnType<typeof sailingWorld>) =>
  ([...w.entities.values()].find((e) => e.name === "Sloop")!.components.Transform as { position: number[] }).position[0]!;

beforeEach(() => {
  useConnection.getState().set({ editGroups: true });
});

async function drag(edit: LiveEdit, id: number, from: number, steps: number) {
  for (let i = 1; i <= steps; i++) {
    edit.update(id, "Transform", { position: [from + i, 0, 0] });
    await Bun.sleep(25);
  }
}

test("a streamed drag is one entry, and Escape takes it back with one undo", async () => {
  const as = { caller: EDITOR };
  const world = wire(as);
  const sloop = [...world.entities.values()].find((e) => e.name === "Sloop")!;
  useWorld.getState().setEntity(world.get(sloop.id) as EntityData);
  const x0 = sloopX(world);
  const before = world.history().undo.length;
  const edit = new LiveEdit("Move Sloop");
  await drag(edit, sloop.id, x0, 10);
  await edit.commit();
  const h = world.history();
  expect(h.undo.length).toBe(before + 1);
  expect(h.entries.undo.at(-1)).toMatchObject({ label: "Move Sloop", source: "editor" });
  expect(sloopX(world)).toBe(x0 + 10);

  const again = new LiveEdit("Move Sloop");
  await drag(again, sloop.id, x0 + 10, 6);
  await again.cancel();
  expect(world.history().undo.length).toBe(before + 1);
  expect(sloopX(world)).toBe(x0 + 10);
});

test("an agent's edit during a drag splits it; Escape writes the drag's start back", async () => {
  const as = { caller: EDITOR };
  const world = wire(as);
  const sloop = [...world.entities.values()].find((e) => e.name === "Sloop")!;
  useWorld.getState().setEntity(world.get(sloop.id) as EntityData);
  const x0 = sloopX(world);
  const edit = new LiveEdit("Move Sloop");
  await drag(edit, sloop.id, x0, 4);
  // An agent moves a crate (its own client) while the drag goes on.
  const crate = [...world.entities.values()].find((e) => e.name === "Crate1")!;
  world.edit([{ set: { entity: crate.id, component: "Cargo", value: { value: 9 } } }], "agent move", undefined, emptyChanges(), {
    source: { developer: 1 },
    session: "mcp-1",
  });
  await drag(edit, sloop.id, x0 + 4, 4);
  await Bun.sleep(80);
  const split = world.history().entries.undo;
  expect(split.map((e) => e.session ?? e.source)).toEqual(["editor", "mcp-1", "editor"]);
  await edit.cancel();
  // The drag's start is back; the agent's edit stays, and the history keeps all of it.
  expect(sloopX(world)).toBe(x0);
  expect((world.get("Crate1").components.Cargo as { value: number }).value).toBe(9);
  expect(world.history().entries.undo.map((e) => e.label)).toEqual(["Move Sloop", "agent move", "Move Sloop", "Cancel Move Sloop"]);
});

test("an agent's edit after a drag's last step is not undone by Escape", async () => {
  const as = { caller: EDITOR };
  const world = wire(as);
  const sloop = [...world.entities.values()].find((e) => e.name === "Sloop")!;
  useWorld.getState().setEntity(world.get(sloop.id) as EntityData);
  const x0 = sloopX(world);
  const edit = new LiveEdit("Move Sloop");
  await drag(edit, sloop.id, x0, 4);
  await Bun.sleep(80);
  // The agent's edit lands after the drag's last step, before Escape reaches the host: the drag's
  // group is no longer the next to undo, so the host refuses the undo and the editor writes back.
  world.edit([{ set: { entity: "Crate1", component: "Cargo", value: { value: 7 } } }], "agent cargo", undefined, emptyChanges(), {
    source: { developer: 1 },
    session: "mcp-1",
  });
  await edit.cancel();
  expect(sloopX(world)).toBe(x0);
  expect((world.get("Crate1").components.Cargo as { value: number }).value).toBe(7);
  expect(world.history().undo.slice(-3)).toEqual(["Move Sloop", "agent cargo", "Cancel Move Sloop"]);
  expect(world.history().redo).toEqual([]);
});

test("the mock merges a group only for one caller, and an undone entry merges with nothing", () => {
  const world = sailingWorld();
  const set = (x: number): EditOp[] => [{ set: { entity: "Sloop", component: "Transform", value: { position: [x, 0, 0] } } }];
  const agent: Caller = { source: { developer: 2 }, session: "mcp-2" };
  world.edit(set(1), "a", "g", emptyChanges());
  world.edit(set(2), "a", "g", emptyChanges());
  world.edit(set(3), "b", "g", emptyChanges(), agent);
  world.edit(set(4), "c", "g", emptyChanges());
  expect(world.history().undo).toEqual(["a", "b", "c"]);
  world.undo(emptyChanges());
  world.undo(emptyChanges());
  // Most recent last, as the host lists it: "b" is the next to redo.
  expect(world.history().redo).toEqual(["c", "b"]);
  expect(world.history().entries.redo[1]).toEqual({ label: "b", source: { developer: 2 }, session: "mcp-2" });
  // Redone, "b" carries no group: the agent's next edit of "g" starts an entry of its own.
  world.redo(emptyChanges());
  world.edit(set(5), "d", "g", emptyChanges(), agent);
  expect(world.history().undo).toEqual(["a", "b", "d"]);
  // An undo of a group: refused unless the next entry is the whole group.
  expect(() => world.undo(emptyChanges(), "g")).toThrow(MockError);
  expect(world.history().undo).toEqual(["a", "b", "d"]);
  world.edit(set(6), "e", "h", emptyChanges());
  world.edit(set(7), "e", "h", emptyChanges());
  expect(world.undo(emptyChanges(), "h")).toBe("e");
  expect(world.history().undo).toEqual(["a", "b", "d"]);
});

test("the mock starts an entry for a spawn and past 64 components, as the host", () => {
  const world = sailingWorld();
  const n0 = world.history().undo.length;
  world.edit([{ set: { entity: "Sloop", component: "Transform", value: { position: [1, 0, 0] } } }], "a", "g", emptyChanges());
  world.edit([{ spawn: { name: "Buoy", components: {} } }], "a", "g", emptyChanges());
  expect(world.history().undo.length).toBe(n0 + 2);
  // Edits of 64 components (names and removes) in one group, then one more: a new entry.
  const pairs: EditOp[] = [];
  for (const e of world.entities.values()) {
    pairs.push({ set: { entity: e.id, component: "Name", value: `${e.name}` } });
    for (const c of Object.keys(e.components)) pairs.push({ remove: { entity: e.id, component: c } });
  }
  expect(pairs.length).toBeGreaterThan(64);
  const before = world.history().undo.length;
  world.edit(pairs.slice(0, 40), "w", "wide", emptyChanges());
  world.edit(pairs.slice(40, 64), "w", "wide", emptyChanges());
  expect(world.history().undo.length).toBe(before + 1);
  world.edit(pairs.slice(64, 65), "w", "wide", emptyChanges());
  expect(world.history().undo.length).toBe(before + 2);
});
