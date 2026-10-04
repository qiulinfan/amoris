// The connection to the host and what it says about itself: the catalog of commands (the palette
// lists every one) and the project.

import { create } from "zustand";
import type { ConnectionInfo } from "../host/client";
import type { CatalogCommand, ProjectInfo } from "../host/protocol";

interface ConnectionState {
  info: ConnectionInfo;
  catalog: CatalogCommand[];
  project: ProjectInfo | null;
  /** `world.edit` takes `group` (its params schema lists it): drags stream to the host. */
  editGroups: boolean;
  /** Which renderer the viewport runs. */
  viewport: "wasm" | "fallback" | null;
  set(patch: Partial<Omit<ConnectionState, "set">>): void;
}

export const useConnection = create<ConnectionState>((set) => ({
  info: { state: "closed", retryAt: null, attempts: 0, lastError: null },
  catalog: [],
  project: null,
  editGroups: false,
  viewport: null,
  set: (patch) => set(patch),
}));
