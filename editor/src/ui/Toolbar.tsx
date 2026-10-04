import type { ReactNode } from "react";
import { cx } from "./cx";

export function Toolbar({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={cx("toolbar", className)}>{children}</div>;
}

export function ToolbarGroup({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={cx("toolbar-group", className)}>{children}</div>;
}

export function ToolbarSeparator() {
  return <div className="toolbar-sep" />;
}

export function Spacer() {
  return <div className="spacer" />;
}
