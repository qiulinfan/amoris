// An entity reference: shows the target's icon and name; click to pick from a searchable list, or
// drop an entity from the hierarchy on it; a nullable reference can be cleared.

import { Crosshair, X } from "lucide-react";
import { useMemo, useState } from "react";
import { createPortal } from "react-dom";
import { ENTITY_MIME } from "../../state/assets";
import { useWorld } from "../../state/world";
import { entityKind } from "../icons";
import { cx } from "../cx";

export function EntityField({ value, onChange, nullable = true, disabled }: { value: number | null; onChange(v: number | null): void; nullable?: boolean; disabled?: boolean }) {
  const nodes = useWorld((s) => s.nodes);
  const order = useWorld((s) => s.order);
  const [open, setOpen] = useState<{ x: number; y: number; w: number } | null>(null);
  const [q, setQ] = useState("");
  const [over, setOver] = useState(false);
  const node = value !== null ? nodes[value] : undefined;
  const Icon = node ? entityKind(node.components).icon : Crosshair;
  const list = useMemo(
    () => order.map((id) => nodes[id]!).filter((n) => n && (!q || n.name.toLowerCase().includes(q.toLowerCase()) || String(n.id) === q)),
    [order, nodes, q],
  );
  return (
    <div
      className={cx("entity-field", over && "is-over", value !== null && !node && "is-missing")}
      onDragOver={(e) => {
        if (e.dataTransfer.types.includes(ENTITY_MIME)) {
          e.preventDefault();
          setOver(true);
        }
      }}
      onDragLeave={() => setOver(false)}
      onDrop={(e) => {
        setOver(false);
        const ids = JSON.parse(e.dataTransfer.getData(ENTITY_MIME) || "[]") as number[];
        if (ids[0] !== undefined) onChange(ids[0]);
      }}
    >
      <button
        type="button"
        className="entity-pick"
        disabled={disabled}
        onClick={(e) => {
          const r = e.currentTarget.getBoundingClientRect();
          setOpen({ x: r.left, y: r.bottom + 2, w: Math.max(220, r.width) });
          setQ("");
        }}
      >
        <Icon size={13} />
        <span className="entity-name">{value === null ? "None" : node ? node.name : `#${value} (missing)`}</span>
        {value !== null && <span className="entity-id">#{value}</span>}
      </button>
      {nullable && value !== null && !disabled && (
        <button type="button" className="entity-clear" aria-label="Clear" onClick={() => onChange(null)}>
          <X size={12} />
        </button>
      )}
      {open &&
        createPortal(
          <div className="menu-layer" onPointerDown={(e) => e.target === e.currentTarget && setOpen(null)}>
            <div className="menu entity-menu" style={{ left: open.x, top: open.y, width: open.w }}>
              <input
                autoFocus
                className="entity-search"
                placeholder="Find entity…"
                value={q}
                onChange={(e) => setQ(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Escape") setOpen(null);
                  if (e.key === "Enter" && list[0]) {
                    onChange(list[0].id);
                    setOpen(null);
                  }
                }}
              />
              <div className="entity-list">
                {nullable && (
                  <div className="menu-item" onClick={() => (onChange(null), setOpen(null))}>
                    <span className="menu-icon" />
                    <span className="menu-label">None</span>
                  </div>
                )}
                {list.slice(0, 200).map((n) => {
                  const I = entityKind(n.components).icon;
                  return (
                    <div key={n.id} className={cx("menu-item", n.id === value && "is-active")} onClick={() => (onChange(n.id), setOpen(null))}>
                      <span className="menu-icon">
                        <I size={13} />
                      </span>
                      <span className="menu-label">{n.name}</span>
                      <span className="menu-keys">#{n.id}</span>
                    </div>
                  );
                })}
              </div>
            </div>
          </div>,
          document.body,
        )}
    </div>
  );
}
