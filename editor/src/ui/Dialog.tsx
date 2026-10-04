import { useEffect, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { cx } from "./cx";

export function Dialog({ onClose, children, className, top = false }: { onClose(): void; children: ReactNode; className?: string; top?: boolean }) {
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", key, true);
    return () => window.removeEventListener("keydown", key, true);
  }, [onClose]);
  return createPortal(
    <div className={cx("dialog-backdrop", top && "top")} onPointerDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className={cx("dialog", className)} role="dialog">
        {children}
      </div>
    </div>,
    document.body,
  );
}
