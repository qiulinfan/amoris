// Every panel of the editor: its id (also the dockview component name), title, icon and where it
// goes when opened while closed. One module per panel under panels/.

import {
  Activity,
  Bot,
  Box,
  Bug,
  FileCode,
  FolderOpen,
  ListRestart,
  ListTree,
  SlidersHorizontal,
  SquareTerminal,
  Zap,
  ClockArrowDown,
  type LucideIcon,
} from "lucide-react";
import type { FunctionComponent } from "react";
import { AgentPanel } from "../panels/agent/AgentPanel";
import { AssetsPanel } from "../panels/assets/AssetsPanel";
import { ConsolePanel } from "../panels/console/ConsolePanel";
import { DebugPanel } from "../panels/debug/DebugPanel";
import { EventsPanel } from "../panels/events/EventsPanel";
import { HierarchyPanel } from "../panels/hierarchy/HierarchyPanel";
import { HistoryPanel } from "../panels/history/HistoryPanel";
import { InspectorPanel } from "../panels/inspector/InspectorPanel";
import { ProfilerPanel } from "../panels/profiler/ProfilerPanel";
import { ScriptsPanel } from "../panels/scripts/ScriptsPanel";
import { TimelinePanel } from "../panels/timeline/TimelinePanel";
import { ViewportPanel } from "../panels/viewport/ViewportPanel";

export type Area = "center" | "left" | "left-bottom" | "right" | "right-bottom" | "bottom";

export interface PanelDef {
  id: string;
  title: string;
  icon: LucideIcon;
  component: FunctionComponent;
  area: Area;
  /** Keep the DOM while hidden (the viewport's canvas, Monaco). */
  keepAlive?: boolean;
}

export const PANELS: PanelDef[] = [
  { id: "viewport", title: "Viewport", icon: Box, component: ViewportPanel, area: "center", keepAlive: true },
  { id: "scripts", title: "Scripts", icon: FileCode, component: ScriptsPanel, area: "center", keepAlive: true },
  { id: "hierarchy", title: "Hierarchy", icon: ListTree, component: HierarchyPanel, area: "left" },
  { id: "assets", title: "Assets", icon: FolderOpen, component: AssetsPanel, area: "left-bottom" },
  { id: "inspector", title: "Inspector", icon: SlidersHorizontal, component: InspectorPanel, area: "right" },
  { id: "history", title: "History", icon: ListRestart, component: HistoryPanel, area: "right-bottom" },
  { id: "agent", title: "Agent", icon: Bot, component: AgentPanel, area: "right-bottom" },
  { id: "console", title: "Console", icon: SquareTerminal, component: ConsolePanel, area: "bottom" },
  { id: "events", title: "Events", icon: Zap, component: EventsPanel, area: "bottom" },
  { id: "timeline", title: "Timeline", icon: ClockArrowDown, component: TimelinePanel, area: "bottom" },
  { id: "profiler", title: "Profiler", icon: Activity, component: ProfilerPanel, area: "bottom" },
  { id: "debug", title: "Debug", icon: Bug, component: DebugPanel, area: "bottom" },
];

export const PANEL_BY_ID: Record<string, PanelDef> = Object.fromEntries(PANELS.map((p) => [p.id, p]));
