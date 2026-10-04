// "Add Component": a searchable list of the registered components the selection lacks, with docs.

import { Plus } from "lucide-react";
import { useMemo, useState } from "react";
import { createPortal } from "react-dom";
import { addComponent } from "../../actions/world";
import type { EntityId } from "../../host/protocol";
import { useWorld } from "../../state/world";
import { componentIcon } from "../../ui/icons";
import { cx } from "../../ui/cx";

export function AddComponent({ ids, present }: { ids: EntityId[]; present: Set<string> }) {
  const list = useWorld((s) => s.schemaList);
  const [at, setAt] = useState<{ x: number; top?: number; bottom?: number; w: number } | null>(null);
  const [q, setQ] = useState("");
  const [active, setActive] = useState(0);
  const items = useMemo(
    () => list.filter((c) => c.name !== "Name" && !present.has(c.name) && (!q || c.name.toLowerCase().includes(q.toLowerCase()) || c.doc.toLowerCase().includes(q.toLowerCase()))),
    [list, present, q],
  );
  const pick = (name: string) => {
    setAt(null);
    void addComponent(ids, name);
  };
  return (
    <>
      <button
        type="button"
        className="add-component"
        onClick={(e) => {
          const r = e.currentTarget.getBoundingClientRect();
          // Open downward when there is room, upward otherwise (the button sits at the bottom).
          const room = window.innerHeight - r.bottom;
          setAt(room > 360 ? { x: r.left, top: r.bottom + 4, w: r.width } : { x: r.left, bottom: window.innerHeight - r.top + 4, w: r.width });
          setQ("");
          setActive(0);
        }}
      >
        <Plus size={14} /> Add Component
      </button>
      {at &&
        createPortal(
          <div className="menu-layer" onPointerDown={(e) => e.target === e.currentTarget && setAt(null)}>
            <div className="menu add-menu" style={{ left: at.x, top: at.top, bottom: at.bottom, width: at.w }}>
              <input
                autoFocus
                className="entity-search"
                placeholder="Search components…"
                value={q}
                onChange={(e) => {
                  setQ(e.target.value);
                  setActive(0);
                }}
                onKeyDown={(e) => {
                  if (e.key === "Escape") setAt(null);
                  if (e.key === "ArrowDown") setActive((a) => Math.min(items.length - 1, a + 1));
                  if (e.key === "ArrowUp") setActive((a) => Math.max(0, a - 1));
                  if (e.key === "Enter" && items[active]) pick(items[active]!.name);
                }}
              />
              <div className="entity-list">
                {items.length === 0 && <div className="menu-empty">No component matches</div>}
                {items.map((c, i) => {
                  const Icon = componentIcon(c.name);
                  return (
                    <div key={c.name} className={cx("menu-item add-item-row", i === active && "is-active")} onPointerEnter={() => setActive(i)} onClick={() => pick(c.name)}>
                      <span className="menu-icon">
                        <Icon size={14} />
                      </span>
                      <span className="add-text">
                        <span className="menu-label">{c.name}</span>
                        <span className="add-doc">{c.doc}</span>
                      </span>
                      <span className="menu-keys">{c.origin === "Project" ? "TS" : ""}</span>
                    </div>
                  );
                })}
              </div>
            </div>
          </div>,
          document.body,
        )}
    </>
  );
}
