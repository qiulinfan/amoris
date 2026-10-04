import { useState } from "react";
import { PopupMenu } from "../ui/Menu";
import { cx } from "../ui/cx";
import { buildMenu, MENUS } from "./menus";

export function MenuBar() {
  const [open, setOpen] = useState<number | null>(null);
  const [anchor, setAnchor] = useState({ x: 0, y: 0 });
  const show = (i: number) => {
    const el = document.querySelector<HTMLElement>(`[data-menu="${i}"]`);
    if (!el) return;
    const r = el.getBoundingClientRect();
    setAnchor({ x: r.left, y: r.bottom + 3 });
    setOpen(i);
  };
  return (
    <nav className="menubar" role="menubar">
      {MENUS.map((m, i) => (
        <button
          key={m.label}
          type="button"
          data-menu={i}
          className={cx("menubar-item", open === i && "is-open")}
          onPointerDown={(e) => {
            e.preventDefault();
            if (open === i) setOpen(null);
            else show(i);
          }}
          onPointerEnter={() => open !== null && open !== i && show(i)}
        >
          {m.label}
        </button>
      ))}
      {open !== null && (
        <PopupMenu
          key={open}
          items={buildMenu(MENUS[open]!.items)}
          x={anchor.x}
          y={anchor.y}
          onClose={() => setOpen(null)}
          onLeftEdge={() => show((open - 1 + MENUS.length) % MENUS.length)}
          onRightEdge={() => show((open + 1) % MENUS.length)}
        />
      )}
    </nav>
  );
}
