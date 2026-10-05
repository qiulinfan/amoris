// The host's clock and mode (`status` events) and the kept snapshots (`snapshots.list`).

import { create } from "zustand";
import type { SnapshotInfo, Status } from "../host/protocol";

interface SessionState {
  status: Status | null;
  /** performance.now() when the last status arrived (the viewport's wave clock runs on from it). */
  receivedAt: number;
  snapshots: SnapshotInfo[];
  setStatus(s: Status): void;
  setSnapshots(s: SnapshotInfo[]): void;
}

export const useSession = create<SessionState>((set) => ({
  status: null,
  receivedAt: 0,
  snapshots: [],
  setStatus: (status) => set({ status, receivedAt: performance.now() }),
  setSnapshots: (snapshots) => set({ snapshots }),
}));

export function isPlaying(): boolean {
  return useSession.getState().status?.mode === "play";
}
