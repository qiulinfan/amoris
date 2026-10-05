// A number input whose label scrubs: drag it sideways to change the value (Shift x10, Alt x0.1),
// as in Unity, Blender and Figma. Typing accepts arithmetic ("2*3+1"); arrows step; Esc reverts.

import { useEffect, useRef, useState, type ReactNode } from "react";
import { cx } from "../cx";
import { num } from "../format";
import { dragState } from "../../state/viewport";

export type EditPhase = "live" | "commit";

export interface NumberFieldProps {
  value: number;
  onChange(v: number, phase: EditPhase): void;
  onCancel?(): void;
  step?: number;
  min?: number;
  max?: number;
  integer?: boolean;
  handle?: ReactNode;
  handleClass?: string;
  unit?: string;
  precision?: number;
  disabled?: boolean;
  mixed?: boolean;
  className?: string;
}

function evaluate(text: string): number | null {
  const t = text.trim();
  if (!t) return null;
  if (!/^[-+*/().\d\seE]+$/.test(t)) return null;
  try {
    const v = Function(`"use strict"; return (${t});`)() as unknown;
    return typeof v === "number" && Number.isFinite(v) ? v : null;
  } catch {
    return null;
  }
}

function clamp(v: number, min?: number, max?: number) {
  if (min !== undefined && v < min) return min;
  if (max !== undefined && v > max) return max;
  return v;
}

export function NumberField({
  value,
  onChange,
  onCancel,
  step,
  min,
  max,
  integer,
  handle,
  handleClass,
  unit,
  precision = 3,
  disabled,
  mixed,
  className,
}: NumberFieldProps) {
  const [text, setText] = useState<string | null>(null);
  const [scrubbing, setScrubbing] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  const drag = useRef<{ start: number; acc: number; moved: boolean; last: number } | null>(null);

  const perPixel = step ?? (integer ? 0.1 : min !== undefined && max !== undefined && max - min <= 2 ? (max - min) / 300 : 0.02);
  const keyStep = step ?? (integer ? 1 : min !== undefined && max !== undefined && max - min <= 2 ? 0.05 : 0.1);
  const fix = (v: number) => clamp(integer ? Math.round(v) : Math.round(v * 1e6) / 1e6, min, max);

  useEffect(() => {
    if (!scrubbing) return;
    const move = (e: PointerEvent) => {
      const d = drag.current;
      if (!d) return;
      const mult = e.shiftKey ? 10 : e.altKey ? 0.1 : 1;
      d.acc += e.movementX * perPixel * mult;
      if (Math.abs(d.acc) > 0 || d.moved) {
        d.moved = true;
        const v = fix(d.start + d.acc);
        if (v !== d.last) {
          d.last = v;
          onChange(v, "live");
        }
      }
    };
    const up = () => {
      const d = drag.current;
      drag.current = null;
      dragState.active = false;
      setScrubbing(false);
      if (document.pointerLockElement) document.exitPointerLock();
      if (d?.moved) onChange(d.last, "commit");
      else {
        input.current?.focus();
        input.current?.select();
      }
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape" && drag.current) {
        drag.current = null;
        dragState.active = false;
        setScrubbing(false);
        if (document.pointerLockElement) document.exitPointerLock();
        onCancel?.();
      }
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("keydown", key, true);
    return () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("keydown", key, true);
    };
  });

  const startScrub = (e: React.PointerEvent) => {
    if (disabled || e.button !== 0) return;
    e.preventDefault();
    drag.current = { start: value, acc: 0, moved: false, last: value };
    dragState.active = true;
    setScrubbing(true);
    (e.currentTarget as HTMLElement).requestPointerLock?.()?.catch?.(() => undefined);
  };

  const commitText = () => {
    if (text === null) return;
    const v = evaluate(text);
    setText(null);
    if (v !== null && v !== value) onChange(fix(v), "commit");
  };

  // Fewer decimals for larger numbers, so vectors fit narrow columns; the input shows all on focus.
  const digits = Math.abs(value) >= 1000 ? 0 : Math.abs(value) >= 100 ? 1 : Math.abs(value) >= 10 ? 2 : precision;
  const shown = mixed ? "—" : num(value, Math.min(precision, digits));

  return (
    <div className={cx("num-field", scrubbing && "is-scrubbing", disabled && "is-disabled", className)}>
      {handle !== undefined && (
        <span className={cx("num-handle", handleClass)} onPointerDown={startScrub}>
          {handle}
        </span>
      )}
      <input
        ref={input}
        className="num-input"
        value={text ?? shown}
        disabled={disabled}
        spellCheck={false}
        inputMode="decimal"
        onFocus={(e) => {
          setText(mixed ? "" : String(Math.round(value * 1e6) / 1e6));
          requestAnimationFrame(() => e.target.select());
        }}
        onChange={(e) => setText(e.target.value)}
        onBlur={commitText}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            commitText();
            e.currentTarget.blur();
          } else if (e.key === "Escape") {
            setText(null);
            e.currentTarget.blur();
          } else if (e.key === "ArrowUp" || e.key === "ArrowDown") {
            e.preventDefault();
            const mult = e.shiftKey ? 10 : e.altKey ? 0.1 : 1;
            const v = fix(value + (e.key === "ArrowUp" ? 1 : -1) * keyStep * mult);
            setText(String(v));
            onChange(v, "commit");
          }
        }}
      />
      {unit && <span className="num-unit">{unit}</span>}
    </div>
  );
}
