// Viewport settings the user keeps between sessions (tool, space, snapping, grid) and the camera
// requests other panels make (frame the selection, look along an axis).

import { create } from "zustand";
import { persist } from "zustand/middleware";
import type { Vec3 } from "../host/protocol";

export type Tool = "select" | "translate" | "rotate" | "scale";
export type Space = "world" | "local";

export interface Snap {
  enabled: boolean;
  translate: number;
  rotate: number;
  scale: number;
}

interface ViewportState {
  tool: Tool;
  space: Space;
  snap: Snap;
  grid: boolean;
  stats: boolean;
  /** Where the camera looks; new entities spawn here. */
  target: Vec3;
  /** Bumped to ask the viewport to frame the selection. */
  frameNonce: number;
  /** A requested view direction (from the axis gizmo or the View menu). */
  view: { dir: "+x" | "-x" | "+y" | "-y" | "+z" | "-z" | "persp"; nonce: number } | null;
  set(patch: Partial<Pick<ViewportState, "tool" | "space" | "grid" | "stats" | "target">>): void;
  setSnap(patch: Partial<Snap>): void;
  frame(): void;
  look(dir: NonNullable<ViewportState["view"]>["dir"]): void;
}

export const useViewport = create<ViewportState>()(
  persist(
    (set, get) => ({
      tool: "translate",
      space: "world",
      snap: { enabled: false, translate: 0.5, rotate: 15, scale: 0.1 },
      grid: true,
      stats: true,
      target: [0, 0, 0],
      frameNonce: 0,
      view: null,
      set: (patch) => set(patch),
      setSnap: (patch) => set({ snap: { ...get().snap, ...patch } }),
      frame: () => set({ frameNonce: get().frameNonce + 1 }),
      look: (dir) => set({ view: { dir, nonce: (get().view?.nonce ?? 0) + 1 } }),
    }),
    {
      name: "amoris.editor.viewport.v1",
      partialize: (s) => ({ tool: s.tool, space: s.space, snap: s.snap, grid: s.grid, stats: s.stats }),
    },
  ),
);

/** A pointer drag is in progress somewhere (gizmo, scrubbed field): Escape cancels it, not the selection. */
export const dragState = { active: false };
