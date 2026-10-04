// The selection: entity ids in the order they were picked; the last is the primary one the
// inspector and the gizmo follow.

import { create } from "zustand";
import type { EntityId } from "../host/protocol";

interface SelectionState {
  ids: EntityId[];
  primary: EntityId | null;
  anchor: EntityId | null;
  hovered: EntityId | null;
  set(ids: EntityId[], primary?: EntityId | null): void;
  toggle(id: EntityId): void;
  range(id: EntityId, order: EntityId[]): void;
  clear(): void;
  prune(exists: (id: EntityId) => boolean): void;
  hover(id: EntityId | null): void;
}

export const useSelection = create<SelectionState>((set, get) => ({
  ids: [],
  primary: null,
  anchor: null,
  hovered: null,
  set: (ids, primary) => set({ ids, primary: primary === undefined ? (ids.at(-1) ?? null) : primary, anchor: ids.at(-1) ?? null }),
  toggle: (id) => {
    const ids = get().ids.includes(id) ? get().ids.filter((x) => x !== id) : [...get().ids, id];
    set({ ids, primary: ids.includes(id) ? id : (ids.at(-1) ?? null), anchor: id });
  },
  range: (id, order) => {
    const anchor = get().anchor ?? id;
    const a = order.indexOf(anchor);
    const b = order.indexOf(id);
    if (a < 0 || b < 0) return set({ ids: [id], primary: id, anchor: id });
    const [lo, hi] = a < b ? [a, b] : [b, a];
    set({ ids: order.slice(lo, hi + 1), primary: id });
  },
  clear: () => set({ ids: [], primary: null }),
  prune: (exists) => {
    const ids = get().ids.filter(exists);
    if (ids.length !== get().ids.length) {
      const primary = get().primary !== null && exists(get().primary!) ? get().primary : (ids.at(-1) ?? null);
      set({ ids, primary });
    }
  },
  hover: (id) => {
    if (get().hovered !== id) set({ hovered: id });
  },
}));
