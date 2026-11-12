import { cx } from "../cx";

/** A switch; `mixed` (values that differ across a multi-selection) shows it half-way, and a click
 * turns it on. */
export function Toggle({ value, onChange, disabled, label, mixed }: { value: boolean; onChange(v: boolean): void; disabled?: boolean; label?: string; mixed?: boolean }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={mixed ? "mixed" : value}
      aria-label={label}
      disabled={disabled}
      className={cx("toggle", mixed ? "is-mixed" : value && "is-on")}
      onClick={() => onChange(mixed ? true : !value)}
    >
      <span className="toggle-knob" />
    </button>
  );
}
