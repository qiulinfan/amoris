import type { ReactNode } from "react";
import type { LucideIcon } from "lucide-react";
import { cx } from "./cx";

/** A panel's frame: an optional toolbar, a scrolling body and an optional footer. */
export function PanelShell({
  toolbar,
  footer,
  children,
  className,
  bodyClassName,
  scroll = true,
}: {
  toolbar?: ReactNode;
  footer?: ReactNode;
  children: ReactNode;
  className?: string;
  bodyClassName?: string;
  scroll?: boolean;
}) {
  return (
    <div className={cx("panel", className)}>
      {toolbar}
      <div className={cx("panel-body", scroll && "scroll", bodyClassName)}>{children}</div>
      {footer}
    </div>
  );
}

export function EmptyState({ icon: Icon, title, children }: { icon?: LucideIcon; title: string; children?: ReactNode }) {
  return (
    <div className="empty-state">
      {Icon && <Icon size={28} strokeWidth={1.4} />}
      <div className="empty-title">{title}</div>
      {children && <div className="empty-body">{children}</div>}
    </div>
  );
}

export function Section({
  title,
  open,
  onToggle,
  actions,
  children,
  count,
}: {
  title: string;
  open: boolean;
  onToggle(): void;
  actions?: ReactNode;
  children: ReactNode;
  count?: number;
}) {
  return (
    <div className={cx("section", open && "is-open")}>
      <div className="section-head" onClick={onToggle}>
        <span className="chev">{open ? "▾" : "▸"}</span>
        <span className="section-title">{title}</span>
        {count !== undefined && <span className="count">{count}</span>}
        <div className="section-actions" onClick={(e) => e.stopPropagation()}>
          {actions}
        </div>
      </div>
      {open && <div className="section-body">{children}</div>}
    </div>
  );
}
