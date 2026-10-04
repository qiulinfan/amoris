import { cx } from "../cx";

export function Toggle({ value, onChange, disabled, label }: { value: boolean; onChange(v: boolean): void; disabled?: boolean; label?: string }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={value}
      aria-label={label}
      disabled={disabled}
      className={cx("toggle", value && "is-on")}
      onClick={() => onChange(!value)}
    >
      <span className="toggle-knob" />
    </button>
  );
}
