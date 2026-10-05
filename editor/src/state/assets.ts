// The project's assets (`assets.list`).

import { create } from "zustand";
import type { AssetInfo } from "../host/protocol";

interface AssetsState {
  assets: AssetInfo[];
  set(assets: AssetInfo[]): void;
}

export const useAssets = create<AssetsState>((set) => ({
  assets: [],
  set: (assets) => set({ assets }),
}));

export const ASSET_MIME = "application/x-pocket-asset";
export const ENTITY_MIME = "application/x-pocket-entities";
