// How a streamed drag sends (editor.md 6): the changes of one input event go in one `world.edit`
// at once, at most one edit is in flight, and what comes while it is goes merged the moment it is
// answered; no timer paces the drag. Ending the drag sends nothing more until the last edit.
import { beforeEach, expect, test } from "bun:test";
import type { EditOp } from "../src/host/protocol";

// The editor's host client is made when `api` loads, from the page's location; nothing connects.
const page = (globalThis as { window?: object }).window ?? {};
Object.defineProperty(globalThis, "window", { configurable: true, value: { ...page, location: new URL("http://127.0.0.1:7878/") } });
const { LiveEdit } = await import("../src/actions/world");
const { api } = await import("../src/host/api");
const { useConnection } = await import("../src/state/connection");

type Sent = { ops: EditOp[]; group?: string; answer: () => void };

/** `api.world.edit` recording each call, answered when the test says; `world.get` refused (the
 * entities are not in the store). */
function wire(): Sent[] {
  const sent: Sent[] = [];
  const a = api as unknown as { world: Record<string, unknown> };
  a.world.edit = (ops: EditOp[], _label?: string, group?: string) =>
    new Promise<void>((resolve) => sent.push({ ops, group, answer: resolve }));
  a.world.get = async () => {
    throw new Error("not in this test");
  };
  return sent;
}

const x = (s: Sent, i = 0) => ((s.ops[i] as { set: { value: { position: number[] } } }).set.value.position[0]);
const tick = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  useConnection.getState().set({ editGroups: true });
});

test("one edit in flight, the next merged and sent on its answer", async () => {
  const sent = wire();
  const edit = new LiveEdit("Move");
  // One pointer event moving two entities: one edit, sent without waiting for a timer.
  edit.update(1, "Transform", { position: [1, 0, 0] });
  edit.update(2, "Transform", { position: [5, 0, 0] });
  await tick();
  expect(sent.length).toBe(1);
  expect(sent[0]!.ops.length).toBe(2);
  expect(sent[0]!.group).toMatch(/^drag-/);
  // Three more events while it is in flight: nothing goes yet.
  for (const v of [2, 3, 4]) {
    edit.update(1, "Transform", { position: [v, 0, 0] });
    await tick();
  }
  expect(sent.length).toBe(1);
  // Answered: the latest value goes at once, alone.
  sent[0]!.answer();
  await tick();
  expect(sent.length).toBe(2);
  expect(sent[1]!.ops.length).toBe(1);
  expect(x(sent[1]!)).toBe(4);
  // Answered with nothing new: nothing is sent; the next event goes at once.
  sent[1]!.answer();
  await tick();
  expect(sent.length).toBe(2);
  edit.update(1, "Transform", { position: [6, 0, 0] });
  await tick();
  expect(sent.length).toBe(3);
  expect(x(sent[2]!)).toBe(6);
  // Release while that edit is in flight: the stream stops, and the drag's last edit goes after
  // the answer, every field of the drag in it.
  edit.update(1, "Transform", { position: [7, 0, 0] });
  const done = edit.commit();
  await tick();
  expect(sent.length).toBe(3);
  sent[2]!.answer();
  await tick();
  expect(sent.length).toBe(4);
  expect(sent[3]!.ops.length).toBe(2);
  expect(x(sent[3]!)).toBe(7);
  sent[3]!.answer();
  await done;
  expect(sent.length).toBe(4);
});

test("a host without groups gets one edit on release", async () => {
  useConnection.getState().set({ editGroups: false });
  const sent = wire();
  const edit = new LiveEdit("Move");
  for (const v of [1, 2, 3]) {
    edit.update(1, "Transform", { position: [v, 0, 0] });
    await tick();
  }
  expect(sent.length).toBe(0);
  const done = edit.commit();
  await tick();
  expect(sent.length).toBe(1);
  expect(sent[0]!.group).toBeUndefined();
  expect(x(sent[0]!)).toBe(3);
  sent[0]!.answer();
  await done;
});
