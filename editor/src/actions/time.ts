// The transport: Play forks the edit world, Stop discards the fork, Pause and Step drive time.

import { api } from "../host/api";
import { refreshSnapshots } from "../host/sync";
import { useSession } from "../state/session";
import { attempt } from "./report";

const status = () => useSession.getState().status;

export async function play() {
  const s = await attempt(api.play.start(), "Play");
  if (s) useSession.getState().setStatus(s);
  void refreshSnapshots();
}

export async function stop() {
  const s = await attempt(api.play.stop(), "Stop");
  if (s) useSession.getState().setStatus(s);
  void refreshSnapshots();
}

export async function togglePlay() {
  if (status()?.mode === "play") await stop();
  else await play();
}

export async function togglePause() {
  const st = status();
  if (!st) return;
  if (st.mode === "edit") {
    await play();
    await setPaused(true);
    return;
  }
  await setPaused(!st.paused);
}

export async function setPaused(pause: boolean) {
  const s = await attempt(api.time.control({ pause }), pause ? "Pause" : "Resume");
  if (s) useSession.getState().setStatus(s);
}

/** One tick. From Edit, starts Play paused first (as Unity's Step does). */
export async function stepTick(ticks = 1) {
  const st = status();
  if (!st) return;
  if (st.mode === "edit") {
    await play();
    await setPaused(true);
  } else if (!st.paused) {
    await setPaused(true);
  }
  await attempt(api.time.step({ ticks }), "Step");
}

export async function setSpeed(speed: number) {
  const s = await attempt(api.time.control({ speed }), "Speed");
  if (s) useSession.getState().setStatus(s);
}

export async function restoreSnapshot(tick: number) {
  const s = await attempt(api.snapshots.restore(tick), "Restore snapshot");
  if (s) useSession.getState().setStatus(s);
  void refreshSnapshots();
}

/** `debug.rewind`: the host restores the kept snapshot at or before `tick` and steps to it. */
export async function rewind(tick: number) {
  const r = await attempt(api.debug.rewind(tick), "Rewind");
  if (r) {
    const s = await attempt(api.time.control({}), "Rewind");
    if (s) useSession.getState().setStatus(s);
  }
  void refreshSnapshots();
}

export async function saveProject() {
  const r = await attempt(api.project.save(), "Save project");
  return r;
}
