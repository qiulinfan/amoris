// The transport: Play forks the edit world, Stop discards the fork, Pause and Step drive time.

import { api } from "../host/api";
import { refreshSnapshots } from "../host/sync";
import { useDebug } from "../state/debug";
import { logLocal } from "../state/logs";
import { useSession } from "../state/session";
import { continueEveryPause } from "./debug";
import { attempt } from "./report";
import { applyScripts } from "./scripts";

const status = () => useSession.getState().status;

export async function play() {
  const s = await attempt(api.play.start(), "Play");
  if (s) useSession.getState().setStatus(s);
  void refreshSnapshots();
}

/**
 * Stop. The game thread answers `play.stop` between ticks only, so a paused script is continued
 * first, and any pause later in that tick too (debugger.md 8). Stop returns to the edit world with
 * the bundle it had: scripts applied during Play are on disk, so they are applied to it again.
 */
export async function stop() {
  continueEveryPause(true);
  try {
    if (useDebug.getState().state.state === "paused") await api.debug.resume().catch(() => undefined);
    const played = await api.time.control({}).catch(() => undefined);
    const s = await attempt(api.play.stop(), "Stop");
    if (s) {
      useSession.getState().setStatus(s);
      if (played?.bundle && s.bundle && played.bundle !== s.bundle) {
        logLocal("info", `Play ran bundle ${played.bundle.slice(0, 8)}…, the edit world ${s.bundle.slice(0, 8)}…: applying the scripts on disk to the edit world.`);
        await applyScripts();
      }
    }
  } finally {
    continueEveryPause(false);
  }
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
