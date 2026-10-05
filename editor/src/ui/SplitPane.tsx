// Two panes with a draggable divider; the first pane's size is kept in localStorage under `id`.

import { useRef, useState, type ReactNode } from "react";
import { cx } from "./cx";

export function SplitPane({
  id,
  direction = "row",
  initial,
  min = 120,
  children,
  className,
}: {
  id: string;
  direction?: "row" | "column";
  initial: number;
  min?: number;
  children: [ReactNode, ReactNode];
  className?: string;
}) {
  const key = `aipocket2.editor.split.${id}`;
  const [size, setSize] = useState(() => Number(localStorage.getItem(key)) || initial);
  const ref = useRef<HTMLDivElement>(null);
  const onDown = (e: React.PointerEvent) => {
    e.preventDefault();
    const el = ref.current!;
    const r = el.getBoundingClientRect();
    const total = direction === "row" ? r.width : r.height;
    const move = (ev: PointerEvent) => {
      const at = direction === "row" ? ev.clientX - r.left : ev.clientY - r.top;
      const next = Math.max(min, Math.min(total - min, at));
      setSize(next);
      localStorage.setItem(key, String(Math.round(next)));
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      document.body.classList.remove("resizing");
    };
    document.body.classList.add("resizing");
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };
  return (
    <div ref={ref} className={cx("split", `split-${direction}`, className)}>
      <div className="split-a" style={direction === "row" ? { width: size } : { height: size }}>
        {children[0]}
      </div>
      <div className="split-handle" onPointerDown={onDown} />
      <div className="split-b">{children[1]}</div>
    </div>
  );
}
