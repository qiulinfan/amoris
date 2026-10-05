// A variable and its children as an expandable tree (debugger scopes, watches, console results).
// With `onEdit`, a value that has a `path` can be edited in place: double-click it, type a
// JavaScript expression, Enter assigns it (Escape cancels).

import { ChevronRight } from "lucide-react";
import { useState } from "react";
import type { Variable } from "../host/protocol";
import { cx } from "./cx";
import "./variables.css";

function valueClass(type: string): string {
  if (type === "number" || type === "bigint") return "v-num";
  if (type === "string") return "v-str";
  if (type === "boolean") return "v-bool";
  if (type === "null" || type === "undefined") return "v-null";
  if (type === "function") return "v-fn";
  return "v-obj";
}

interface RowProps {
  v: Variable;
  depth?: number;
  onRemove?(): void;
  label?: string;
  /** Assigns an expression to the value at `v.path`; resolves true when it was set. */
  onEdit?(v: Variable, text: string): Promise<boolean>;
}

export function VariableRow({ v, depth = 0, onRemove, label, onEdit }: RowProps) {
  const [open, setOpen] = useState(depth === 0 && !!v.children && v.children.length <= 6);
  const [editing, setEditing] = useState(false);
  const has = !!v.children?.length;
  const editable = !!onEdit && !!v.path && !has;
  return (
    <>
      <div
        className={cx("var-row", editable && "is-editable")}
        style={{ paddingLeft: 8 + depth * 14 }}
        onClick={() => has && !editing && setOpen(!open)}
        data-tip={editable ? `${v.path}: ${v.type} (double-click to set)` : `${label ?? v.name}: ${v.type}`}
      >
        <span className={cx("var-twisty", has && "has", open && "open")}>{has && <ChevronRight size={11} />}</span>
        <span className="var-name">{label ?? v.name}</span>
        <span className="var-sep">:</span>
        {editing ? (
          <input
            autoFocus
            className="var-input"
            defaultValue={v.type === "undefined" ? "" : v.value}
            onClick={(e) => e.stopPropagation()}
            onKeyDown={(e) => {
              e.stopPropagation();
              if (e.key === "Enter") {
                const text = e.currentTarget.value;
                void onEdit!(v, text).then((ok) => ok && setEditing(false));
              }
              if (e.key === "Escape") setEditing(false);
            }}
            onBlur={() => setEditing(false)}
          />
        ) : (
          <span
            className={cx("var-value", valueClass(v.type))}
            onDoubleClick={(e) => {
              if (!editable) return;
              e.stopPropagation();
              setEditing(true);
            }}
          >
            {v.value}
          </span>
        )}
        {onRemove && (
          <button type="button" className="var-remove" aria-label="Remove" onClick={(e) => (e.stopPropagation(), onRemove())}>
            ×
          </button>
        )}
      </div>
      {open && v.children?.map((c, i) => <VariableRow key={`${c.name}-${i}`} v={c} depth={depth + 1} onEdit={onEdit} />)}
    </>
  );
}
