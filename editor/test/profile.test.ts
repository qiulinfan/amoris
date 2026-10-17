import { expect, test } from "bun:test";
import type { HostProfile } from "../src/host/protocol";
import { useProfile } from "../src/state/profile";
import { useRender } from "../src/state/render";
import { FrameMeter } from "../src/viewport/meter";

const host = (tick: number): HostProfile => ({
  tick,
  ticks: 15,
  held: 0,
  missed: 0,
  tick_ms: 0.4,
  tick_max_ms: 0.9,
  busy_ms: 0.6,
  systems: [
    { name: "physics.step", ms: 0.2, max_ms: 0.5 },
    { name: "loop.publish", ms: 0.2, max_ms: 0.3 },
  ],
  phases: [{ name: "Physics", ms: 0.25, max_ms: 0.6 }],
});

test("host samples carry the viewport's frame and GPU timings of the moment they arrive", () => {
  useProfile.getState().clear();
  useRender.getState().set(null);
  useProfile.getState().push(host(15));
  let s = useProfile.getState().samples.at(-1)!;
  expect([s.frame_ms, s.gpu_ms, s.gpu]).toEqual([null, null, []]);
  expect(s.systems.map((r) => r.name)).toEqual(["physics.step", "loop.publish"]);

  useRender.getState().set({ frames: 15, fps: 60, frame_ms: 16.7, render_ms: 0.8, gpu_ms: 2.5, passes: [{ pass: "forward", ms: 1.5 }] });
  useProfile.getState().push(host(30));
  s = useProfile.getState().samples.at(-1)!;
  expect([s.tick, s.frame_ms, s.fps, s.render_ms, s.gpu_ms]).toEqual([30, 16.7, 60, 0.8, 2.5]);
  expect(s.gpu).toEqual([{ pass: "forward", ms: 1.5 }]);

  useProfile.getState().setFrozen(true);
  useProfile.getState().push(host(45));
  expect(useProfile.getState().samples.length).toBe(2);
  useProfile.getState().setFrozen(false);
});

test("the frame meter averages the intervals since its last read", () => {
  const m = new FrameMeter();
  expect(m.read().frames).toBe(0);
  m.frame(1000, 1);
  m.frame(1010, 2);
  m.frame(1030, 3);
  const r = m.read();
  expect([r.frames, r.frame_ms, r.frame_max_ms, r.fps, r.render_ms]).toEqual([2, 15, 20, 1000 / 15, 2]);
  const empty = m.read();
  expect([empty.frames, empty.frame_ms, empty.fps]).toEqual([0, null, null]);
  m.frame(1046, 1);
  expect(m.read().frame_ms).toBe(16);
});
