// The transport: Play forks the edit world, Stop discards the fork, Pause and Step drive time.

import { api } from "../host/api";
import { refreshSnapshots } from "../host/sync";
import { logLocal } from "../state/logs";
import { useSession } from "../state/session";
import { attempt } from "./report";
import { applyScripts } from "./scripts";
import { desktopEditHash, markDesktopSceneSaved } from "../desktop";

const status = () => useSession.getState().status;

export async function play() {
  const s = await attempt(api.play.start(), "Play");
  if (s) useSession.getState().setStatus(s);
  void refreshSnapshots();
}

/**
 * Stop. The host ends Play wherever the script debugger holds it: `play.stop` has the debugger pass
 * over the held tick's pause and any later one until the Stop lands (server.md 3.4), and
 * `time.control {}` answers while it is held. Stop returns to the edit world with the bundle it
 * had: scripts applied during Play are on disk, so they are applied to it again.
 */
export async function stop() {
  const played = await api.time.control({}).catch(() => undefined);
  const s = await attempt(api.play.stop(), "Stop");
  if (s) {
    useSession.getState().setStatus(s);
    if (played?.bundle && s.bundle && played.bundle !== s.bundle) {
      logLocal("info", `Play ran bundle ${played.bundle.slice(0, 8)}…, the edit world ${s.bundle.slice(0, 8)}…: applying the scripts on disk to the edit world.`);
      await applyScripts();
    }
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
  if (window.amorisDesktop) {
    const before = await api.time.control({}).catch(() => null);
    if (before) useSession.getState().setStatus(before);
  }
  const savingHash = desktopEditHash();
  const r = await attempt(api.project.save(), "Save project");
  if (r && window.amorisDesktop) {
    const s = await api.time.control({}).catch(() => null);
    if (s) {
      useSession.getState().setStatus(s);
      markDesktopSceneSaved(savingHash);
    }
  }
  return r;
}
