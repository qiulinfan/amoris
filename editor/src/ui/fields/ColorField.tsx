// A colour stored as linear RGB(A) 0..1 (the renderer's convention), shown and picked in sRGB.

import { NumberField, type EditPhase } from "./NumberField";

const toSrgb = (c: number) => (c <= 0.0031308 ? 12.92 * c : 1.055 * Math.pow(c, 1 / 2.4) - 0.055);
const toLinear = (c: number) => (c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4));

export function linearToHex(rgb: number[]): string {
  return (
    "#" +
    rgb
      .slice(0, 3)
      .map((c) => Math.round(Math.max(0, Math.min(1, toSrgb(c))) * 255).toString(16).padStart(2, "0"))
      .join("")
  );
}

export function hexToLinear(hex: string): number[] {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) return [1, 1, 1];
  const n = parseInt(m[1]!, 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((c) => Math.round(toLinear(c / 255) * 10000) / 10000);
}

export function ColorField({ values, onChange, disabled }: { values: number[]; onChange(v: number[], phase: EditPhase): void; disabled?: boolean }) {
  const hex = linearToHex(values);
  const alpha = values.length === 4 ? values[3]! : undefined;
  return (
    <div className="color-field">
      <label className="swatch" style={{ "--swatch": hex, "--alpha": alpha ?? 1 } as React.CSSProperties} data-tip="Pick a colour (sRGB; stored linear)">
        <input
          type="color"
          value={hex}
          disabled={disabled}
          onChange={(e) => onChange([...hexToLinear(e.target.value), ...(alpha !== undefined ? [alpha] : [])], "live")}
          onBlur={(e) => onChange([...hexToLinear(e.target.value), ...(alpha !== undefined ? [alpha] : [])], "commit")}
        />
      </label>
      <input
        className="hex-input"
        defaultValue={hex.toUpperCase()}
        key={hex}
        disabled={disabled}
        spellCheck={false}
        onKeyDown={(e) => {
          if (e.key === "Enter") e.currentTarget.blur();
        }}
        onBlur={(e) => {
          if (/^#?[0-9a-f]{6}$/i.test(e.target.value) && e.target.value.toLowerCase().replace("#", "") !== hex.slice(1)) {
            onChange([...hexToLinear(e.target.value.startsWith("#") ? e.target.value : `#${e.target.value}`), ...(alpha !== undefined ? [alpha] : [])], "commit");
          }
        }}
      />
      {alpha !== undefined && (
        <NumberField
          value={alpha}
          min={0}
          max={1}
          handle="A"
          handleClass="axis-w"
          disabled={disabled}
          onChange={(a, phase) => onChange([...values.slice(0, 3), a], phase)}
        />
      )}
    </div>
  );
}
