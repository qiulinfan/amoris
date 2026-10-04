// A virtualized tree: only the rows in view are in the DOM, so a scene of tens of thousands of
// entities scrolls as fast as one of ten. Selection, expansion and renaming belong to the caller.

import { ChevronRight } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { cx } from "./cx";

export interface TreeRow {
  id: number;
  depth: number;
  label: string;
  icon?: ReactNode;
  hasChildren: boolean;
  expanded: boolean;
  dim?: boolean;
  trailing?: ReactNode;
  match?: [number, number];
}

interface TreeProps {
  rows: TreeRow[];
  selected: Set<number>;
  primary: number | null;
  rowHeight?: number;
  renaming: number | null;
  scrollToId?: number | null;
  dropTarget?: number | "root" | null;
  onToggle(id: number): void;
  onSelect(id: number, e: React.MouseEvent): void;
  onActivate?(id: number): void;
  onContextMenu?(id: number | null, e: React.MouseEvent): void;
  onRename(id: number, name: string | null): void;
  onKeyDown?(e: React.KeyboardEvent): void;
  onHover?(id: number | null): void;
  dragProps?(id: number): Partial<React.HTMLAttributes<HTMLDivElement>> & { draggable?: boolean };
  containerDrag?: Partial<React.HTMLAttributes<HTMLDivElement>>;
}

function Label({ row }: { row: TreeRow }) {
  if (!row.match) return <>{row.label}</>;
  const [a, b] = row.match;
  return (
    <>
      {row.label.slice(0, a)}
      <mark>{row.label.slice(a, b)}</mark>
      {row.label.slice(b)}
    </>
  );
}

function RenameInput({ initial, onDone }: { initial: string; onDone(v: string | null): void }) {
  const ref = useRef<HTMLInputElement>(null);
  const done = useRef(false);
  useEffect(() => {
    ref.current?.focus();
    ref.current?.select();
  }, []);
  const finish = (v: string | null) => {
    if (done.current) return;
    done.current = true;
    onDone(v);
  };
  return (
    <input
      ref={ref}
      className="tree-rename"
      defaultValue={initial}
      spellCheck={false}
      onClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Enter") finish(e.currentTarget.value);
        if (e.key === "Escape") finish(null);
      }}
      onBlur={(e) => finish(e.currentTarget.value)}
    />
  );
}

export function Tree(props: TreeProps) {
  const { rows, selected, primary, rowHeight = 24, renaming } = props;
  const ref = useRef<HTMLDivElement>(null);
  const [scroll, setScroll] = useState(0);
  const [height, setHeight] = useState(400);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setHeight(el.clientHeight));
    ro.observe(el);
    setHeight(el.clientHeight);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    if (props.scrollToId === null || props.scrollToId === undefined) return;
    const i = rows.findIndex((r) => r.id === props.scrollToId);
    const el = ref.current;
    if (i < 0 || !el) return;
    const top = i * rowHeight;
    if (top < el.scrollTop) el.scrollTop = top;
    else if (top + rowHeight > el.scrollTop + el.clientHeight) el.scrollTop = top + rowHeight - el.clientHeight;
  }, [props.scrollToId, rows, rowHeight]);

  const first = Math.max(0, Math.floor(scroll / rowHeight) - 4);
  const last = Math.min(rows.length, Math.ceil((scroll + height) / rowHeight) + 4);

  return (
    <div
      ref={ref}
      className={cx("tree", props.dropTarget === "root" && "drop-root")}
      tabIndex={0}
      onScroll={(e) => setScroll(e.currentTarget.scrollTop)}
      onKeyDown={props.onKeyDown}
      onContextMenu={(e) => props.onContextMenu?.(null, e)}
      onPointerLeave={() => props.onHover?.(null)}
      {...props.containerDrag}
    >
      <div className="tree-inner" style={{ height: rows.length * rowHeight }}>
        {rows.slice(first, last).map((row, k) => {
          const i = first + k;
          return (
            <div
              key={row.id}
              className={cx(
                "tree-row",
                selected.has(row.id) && "is-selected",
                primary === row.id && "is-primary",
                row.dim && "is-dim",
                props.dropTarget === row.id && "is-drop",
              )}
              style={{ top: i * rowHeight, height: rowHeight, paddingLeft: 6 + row.depth * 14 }}
              onClick={(e) => props.onSelect(row.id, e)}
              onDoubleClick={() => props.onActivate?.(row.id)}
              onContextMenu={(e) => props.onContextMenu?.(row.id, e)}
              onPointerEnter={() => props.onHover?.(row.id)}
              {...props.dragProps?.(row.id)}
            >
              <span
                className={cx("tree-twisty", row.hasChildren && "has-children", row.expanded && "is-open")}
                onClick={(e) => {
                  e.stopPropagation();
                  if (row.hasChildren) props.onToggle(row.id);
                }}
              >
                {row.hasChildren && <ChevronRight size={12} strokeWidth={2} />}
              </span>
              {row.icon && <span className="tree-icon">{row.icon}</span>}
              {renaming === row.id ? (
                <RenameInput initial={row.label} onDone={(v) => props.onRename(row.id, v)} />
              ) : (
                <span className="tree-label">
                  <Label row={row} />
                </span>
              )}
              {row.trailing && <span className="tree-trailing">{row.trailing}</span>}
            </div>
          );
        })}
      </div>
    </div>
  );
}
