// Editor chrome state: the command palette, dialogs, toasts, the context menu and inline renames.

import { create } from "zustand";
import type { ReactNode } from "react";
import type { EntityId } from "../host/protocol";

export interface MenuItem {
  id?: string;
  label: string;
  icon?: ReactNode;
  keys?: string;
  run?: () => void;
  disabled?: boolean;
  checked?: boolean;
  danger?: boolean;
  submenu?: MenuEntry[];
}

export type MenuEntry = MenuItem | "separator" | { header: string };

export interface Toast {
  id: number;
  kind: "error" | "info" | "success" | "warn";
  title: string;
  body?: string;
}

export interface PromptRequest {
  title: string;
  label?: string;
  value: string;
  placeholder?: string;
  confirm?: string;
  resolve(value: string | null): void;
}

interface UiState {
  paletteOpen: boolean;
  paletteQuery: string;
  shortcutsOpen: boolean;
  aboutOpen: boolean;
  toasts: Toast[];
  contextMenu: { x: number; y: number; items: MenuEntry[] } | null;
  renaming: EntityId | null;
  prompt: PromptRequest | null;
  set(patch: Partial<Pick<UiState, "paletteOpen" | "paletteQuery" | "shortcutsOpen" | "aboutOpen" | "renaming" | "prompt">>): void;
  toast(t: Omit<Toast, "id">): void;
  dismiss(id: number): void;
  openContextMenu(x: number, y: number, items: MenuEntry[]): void;
  closeContextMenu(): void;
}

let toastId = 1;

export const useUi = create<UiState>((set, get) => ({
  paletteOpen: false,
  paletteQuery: "",
  shortcutsOpen: false,
  aboutOpen: false,
  toasts: [],
  contextMenu: null,
  renaming: null,
  prompt: null,
  set: (patch) => set(patch),
  toast: (t) => {
    const id = toastId++;
    set({ toasts: [...get().toasts.slice(-4), { ...t, id }] });
    setTimeout(() => get().dismiss(id), t.kind === "error" ? 8000 : 4000);
  },
  dismiss: (id) => set({ toasts: get().toasts.filter((t) => t.id !== id) }),
  openContextMenu: (x, y, items) => set({ contextMenu: { x, y, items } }),
  closeContextMenu: () => set({ contextMenu: null }),
}));

/** Asks for a line of text in a modal; resolves null when cancelled. */
export function promptText(req: Omit<PromptRequest, "resolve">): Promise<string | null> {
  return new Promise((resolve) => useUi.getState().set({ prompt: { ...req, resolve } }));
}
