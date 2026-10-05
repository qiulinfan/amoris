// The dock: dockview with the editor's panels, a default layout like Godot's (hierarchy and assets
// on the left, viewport and scripts in the middle over console/events/timeline/profiler/debug,
// inspector, history and agent on the right), persisted in localStorage and resettable.

import { DockviewReact, type DockviewApi, type DockviewReadyEvent, type IDockviewPanelHeaderProps, type IDockviewPanelProps, type DockviewTheme } from "dockview-react";
import { X } from "lucide-react";
import { useMemo } from "react";
import { bindDock } from "./dock";
import { PANEL_BY_ID, PANELS, type Area } from "./panels";
import { logLocal } from "../state/logs";

const LAYOUT_KEY = "amoris.editor.layout.v1";

const theme: DockviewTheme = {
  name: "pocket",
  className: "dockview-theme-pocket",
  colorScheme: "dark",
  gap: 4,
  dndOverlayMounting: "absolute",
  dndPanelOverlay: "group",
  tabAnimation: "smooth",
};

function PanelTab(props: IDockviewPanelHeaderProps) {
  const def = PANEL_BY_ID[props.api.id];
  const Icon = def?.icon;
  return (
    <div className="dock-tab" onAuxClick={(e) => e.button === 1 && props.api.close()}>
      {Icon && <Icon size={13} strokeWidth={1.9} className="dock-tab-icon" />}
      <span className="dock-tab-title">{props.api.title}</span>
      <button
        type="button"
        className="dock-tab-close"
        aria-label={`Close ${props.api.title}`}
        onPointerDown={(e) => e.stopPropagation()}
        onClick={(e) => {
          e.stopPropagation();
          props.api.close();
        }}
      >
        <X size={11} />
      </button>
    </div>
  );
}

function addPanel(api: DockviewApi, id: string, position?: Parameters<DockviewApi["addPanel"]>[0]["position"], extra: { inactive?: boolean; initialWidth?: number; initialHeight?: number } = {}) {
  const def = PANEL_BY_ID[id];
  if (!def || api.getPanel(id)) return;
  api.addPanel({
    id,
    component: id,
    title: def.title,
    renderer: def.keepAlive ? "always" : "onlyWhenVisible",
    ...(position ? { position } : {}),
    ...extra,
  });
}

export function buildDefaultLayout(api: DockviewApi) {
  addPanel(api, "viewport");
  addPanel(api, "scripts", { referencePanel: "viewport", direction: "within" }, { inactive: true });
  addPanel(api, "hierarchy", { referencePanel: "viewport", direction: "left" }, { initialWidth: 270 });
  addPanel(api, "inspector", { referencePanel: "viewport", direction: "right" }, { initialWidth: 340 });
  addPanel(api, "console", { referencePanel: "viewport", direction: "below" }, { initialHeight: 260 });
  for (const id of ["events", "timeline", "profiler", "debug"]) addPanel(api, id, { referencePanel: "console", direction: "within" }, { inactive: true });
  addPanel(api, "assets", { referencePanel: "hierarchy", direction: "below" }, { initialHeight: 300 });
  addPanel(api, "history", { referencePanel: "inspector", direction: "below" }, { initialHeight: 290 });
  addPanel(api, "agent", { referencePanel: "history", direction: "within" }, { inactive: true });
  api.getPanel("viewport")?.api.setActive();
}

/** Where a closed panel goes back: next to a panel of its area if one is open. */
const AREA_FRIENDS: Record<Area, { near: string[]; fallback: "left" | "right" | "below" | "above" }> = {
  center: { near: ["viewport", "scripts"], fallback: "right" },
  left: { near: ["hierarchy", "assets"], fallback: "left" },
  "left-bottom": { near: ["assets", "hierarchy"], fallback: "left" },
  right: { near: ["inspector", "history", "agent"], fallback: "right" },
  "right-bottom": { near: ["history", "agent", "inspector"], fallback: "right" },
  bottom: { near: ["console", "events", "timeline", "profiler", "debug"], fallback: "below" },
};

function reopen(api: DockviewApi, id: string) {
  const def = PANEL_BY_ID[id];
  if (!def) return;
  const friend = AREA_FRIENDS[def.area].near.find((p) => p !== id && api.getPanel(p));
  if (friend) addPanel(api, id, { referencePanel: friend, direction: "within" });
  else if (def.area === "bottom" && api.getPanel("viewport")) addPanel(api, id, { referencePanel: "viewport", direction: "below" }, { initialHeight: 260 });
  else addPanel(api, id, { direction: AREA_FRIENDS[def.area].fallback });
  api.getPanel(id)?.api.setActive();
}

export function DockLayout() {
  const components = useMemo(
    () =>
      Object.fromEntries(
        PANELS.map((p) => {
          const C = p.component;
          const Wrapped = (_props: IDockviewPanelProps) => (
            <div className={`dock-panel panel-${p.id}`}>
              <C />
            </div>
          );
          Wrapped.displayName = `Panel(${p.id})`;
          return [p.id, Wrapped];
        }),
      ),
    [],
  );

  const onReady = (e: DockviewReadyEvent) => {
    const api = e.api;
    let restored = false;
    try {
      const saved = localStorage.getItem(LAYOUT_KEY);
      if (saved) {
        api.fromJSON(JSON.parse(saved));
        restored = api.panels.length > 0 && api.panels.every((p) => !!PANEL_BY_ID[p.id]);
        if (!restored) api.clear();
      }
    } catch (err) {
      logLocal("warn", `The saved layout could not be restored (${err instanceof Error ? err.message : String(err)}); using the default.`);
      api.clear();
    }
    if (!restored) buildDefaultLayout(api);
    let timer: ReturnType<typeof setTimeout> | null = null;
    api.onDidLayoutChange(() => {
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => {
        try {
          localStorage.setItem(LAYOUT_KEY, JSON.stringify(api.toJSON()));
        } catch {
          // storage unavailable
        }
      }, 300);
    });
    bindDock({
      api,
      add: (id) => reopen(api, id),
      reset: () => {
        api.clear();
        buildDefaultLayout(api);
        localStorage.removeItem(LAYOUT_KEY);
      },
    });
  };

  return <DockviewReact className="dock" components={components} defaultTabComponent={PanelTab} onReady={onReady} theme={theme} />;
}
