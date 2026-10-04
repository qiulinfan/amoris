// A handle on the dock layout for code outside it (commands, the debugger): open or focus a panel,
// reset the layout. The layout component binds it when dockview is ready.

import type { DockviewApi } from "dockview-react";

interface DockBinding {
  api: DockviewApi;
  add(id: string): void;
  reset(): void;
}

let binding: DockBinding | null = null;
const listeners = new Set<() => void>();

export function bindDock(b: DockBinding | null) {
  binding = b;
  listeners.forEach((l) => l());
}

export function onDockChange(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

/** Shows a panel (adding it where it belongs when it was closed); `focus` makes it the active one. */
export function openPanel(id: string, focus = true) {
  if (!binding) return;
  const panel = binding.api.getPanel(id);
  if (!panel) {
    binding.add(id);
    return;
  }
  if (focus) panel.api.setActive();
  else if (!panel.api.isVisible) panel.api.setActive();
}

export function closePanel(id: string) {
  binding?.api.getPanel(id)?.api.close();
}

export function isPanelOpen(id: string): boolean {
  return !!binding?.api.getPanel(id);
}

export function togglePanel(id: string) {
  if (isPanelOpen(id)) closePanel(id);
  else openPanel(id);
}

export function resetLayout() {
  binding?.reset();
}

/** Maximizes the active panel's group, or restores the layout when one is maximized. */
export function toggleMaximize(id?: string) {
  if (!binding) return;
  const api = binding.api;
  if (api.hasMaximizedGroup()) {
    api.exitMaximizedGroup();
    return;
  }
  const panel = id ? api.getPanel(id) : api.activePanel;
  if (panel) api.maximizeGroup(panel);
}
