// Menus: one list renderer (keyboard navigation, submenus, checks, shortcuts) used by the menu bar,
// dropdowns and context menus. Context menus open through the UI store, so any panel can open one.

import { ChevronRight, Check } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { formatKeys } from "../commands/keys";
import { useUi, type MenuEntry, type MenuItem } from "../state/ui";
import { cx } from "./cx";

function isItem(e: MenuEntry): e is MenuItem {
  return typeof e === "object" && "label" in e;
}

export function MenuList({
  items,
  onClose,
  x,
  y,
  autoFocus = true,
  onLeftEdge,
  onRightEdge,
}: {
  items: MenuEntry[];
  onClose(): void;
  x: number;
  y: number;
  autoFocus?: boolean;
  onLeftEdge?(): void;
  onRightEdge?(): void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ x, y });
  const [active, setActive] = useState(-1);
  const [sub, setSub] = useState<{ index: number; x: number; y: number } | null>(null);
  const actionable = items.map((e, i) => (isItem(e) && !e.disabled ? i : -1)).filter((i) => i >= 0);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    setPos({
      x: Math.max(4, Math.min(x, window.innerWidth - r.width - 4)),
      y: Math.max(4, Math.min(y, window.innerHeight - r.height - 4)),
    });
  }, [x, y]);

  useEffect(() => {
    if (autoFocus) ref.current?.focus();
  }, [autoFocus]);

  const openSub = (i: number) => {
    const el = ref.current?.querySelector<HTMLElement>(`[data-index="${i}"]`);
    if (!el) return;
    const r = el.getBoundingClientRect();
    setSub({ index: i, x: r.right - 2, y: r.top - 4 });
  };

  const activate = (i: number) => {
    const e = items[i];
    if (!e || !isItem(e) || e.disabled) return;
    if (e.submenu) {
      openSub(i);
      return;
    }
    onClose();
    e.run?.();
  };

  const onKeyDown = (ev: React.KeyboardEvent) => {
    const pos = actionable.indexOf(active);
    if (ev.key === "ArrowDown") {
      setActive(actionable[(pos + 1) % actionable.length] ?? -1);
    } else if (ev.key === "ArrowUp") {
      setActive(actionable[(pos - 1 + actionable.length) % actionable.length] ?? -1);
    } else if (ev.key === "Enter" || ev.key === " ") {
      if (active >= 0) activate(active);
    } else if (ev.key === "ArrowRight") {
      const e = items[active];
      if (e && isItem(e) && e.submenu) openSub(active);
      else onRightEdge?.();
    } else if (ev.key === "ArrowLeft") {
      onLeftEdge?.();
    } else if (ev.key === "Escape") {
      onClose();
    } else return;
    ev.preventDefault();
    ev.stopPropagation();
  };

  const subItems = sub ? (items[sub.index] as MenuItem).submenu : undefined;

  return (
    <>
      <div ref={ref} className="menu" style={{ left: pos.x, top: pos.y }} tabIndex={-1} role="menu" onKeyDown={onKeyDown} onContextMenu={(e) => e.preventDefault()}>
        {items.map((e, i) => {
          if (e === "separator") return <div key={i} className="menu-sep" />;
          if (!isItem(e)) return <div key={i} className="menu-header">{e.header}</div>;
          return (
            <div
              key={i}
              data-index={i}
              role="menuitem"
              aria-disabled={e.disabled}
              className={cx("menu-item", i === active && "is-active", e.disabled && "is-disabled", e.danger && "is-danger")}
              onPointerEnter={() => {
                setActive(i);
                if (e.submenu) openSub(i);
                else setSub(null);
              }}
              onClick={() => activate(i)}
            >
              <span className="menu-icon">{e.checked ? <Check size={13} /> : e.icon}</span>
              <span className="menu-label">{e.label}</span>
              {e.keys && <span className="menu-keys">{formatKeys(e.keys)}</span>}
              {e.submenu && <ChevronRight size={13} className="menu-chev" />}
            </div>
          );
        })}
      </div>
      {sub && subItems && (
        <MenuList
          items={subItems}
          x={sub.x}
          y={sub.y}
          autoFocus={false}
          onClose={onClose}
          onLeftEdge={() => {
            setSub(null);
            ref.current?.focus();
          }}
        />
      )}
    </>
  );
}

/** A menu anchored at a point that closes on outside clicks. */
export function PopupMenu({ items, x, y, onClose, onLeftEdge, onRightEdge }: { items: MenuEntry[]; x: number; y: number; onClose(): void; onLeftEdge?(): void; onRightEdge?(): void }) {
  useEffect(() => {
    const down = (e: PointerEvent) => {
      if (!(e.target as HTMLElement).closest(".menu")) onClose();
    };
    const blur = () => onClose();
    window.addEventListener("pointerdown", down, true);
    window.addEventListener("blur", blur);
    window.addEventListener("resize", blur);
    return () => {
      window.removeEventListener("pointerdown", down, true);
      window.removeEventListener("blur", blur);
      window.removeEventListener("resize", blur);
    };
  }, [onClose]);
  return createPortal(
    <div className="menu-layer passthrough">
      <MenuList items={items} x={x} y={y} onClose={onClose} onLeftEdge={onLeftEdge} onRightEdge={onRightEdge} />
    </div>,
    document.body,
  );
}

export function ContextMenuHost() {
  const menu = useUi((s) => s.contextMenu);
  const close = useUi((s) => s.closeContextMenu);
  if (!menu) return null;
  return <PopupMenu items={menu.items} x={menu.x} y={menu.y} onClose={close} />;
}

/** Opens a context menu at the pointer. */
export function contextMenu(e: React.MouseEvent, items: MenuEntry[]) {
  e.preventDefault();
  e.stopPropagation();
  useUi.getState().openContextMenu(e.clientX, e.clientY, items);
}
