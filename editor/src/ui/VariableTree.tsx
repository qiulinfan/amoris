// A variable and its children as an expandable tree (debugger scopes, watches, console results).

import { ChevronRight } from "lucide-react";
import { useState } from "react";
import type { Variable } from "../host/protocol";
import { cx } from "./cx";
import "./variables.css";

function valueClass(type: string): string {
  if (type === "number") return "v-num";
  if (type === "string") return "v-str";
  if (type === "boolean") return "v-bool";
  if (type === "null" || type === "undefined") return "v-null";
  return "v-obj";
}

export function VariableRow({ v, depth = 0, onRemove, label }: { v: Variable; depth?: number; onRemove?(): void; label?: string }) {
  const [open, setOpen] = useState(depth === 0 && !!v.children && v.children.length <= 6);
  const has = !!v.children?.length;
  return (
    <>
      <div className="var-row" style={{ paddingLeft: 8 + depth * 14 }} onClick={() => has && setOpen(!open)} data-tip={`${v.name}: ${v.type}`}>
        <span className={cx("var-twisty", has && "has", open && "open")}>{has && <ChevronRight size={11} />}</span>
        <span className="var-name">{label ?? v.name}</span>
        <span className="var-sep">:</span>
        <span className={cx("var-value", valueClass(v.type))}>{v.value}</span>
        {onRemove && (
          <button type="button" className="var-remove" aria-label="Remove" onClick={(e) => (e.stopPropagation(), onRemove())}>
            ×
          </button>
        )}
      </div>
      {open && v.children?.map((c, i) => <VariableRow key={`${c.name}-${i}`} v={c} depth={depth + 1} />)}
    </>
  );
}
