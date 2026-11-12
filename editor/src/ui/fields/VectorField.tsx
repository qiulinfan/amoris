import { NumberField, type EditPhase } from "./NumberField";

const AXES = ["x", "y", "z", "w"];

export function VectorField({
  values,
  onChange,
  onCancel,
  labels,
  step,
  unit,
  integer,
  disabled,
  mixed,
}: {
  values: number[];
  /** `axis` is the one that changed. */
  onChange(values: number[], phase: EditPhase, axis: number): void;
  onCancel?(): void;
  labels?: string[];
  step?: number;
  unit?: string;
  integer?: boolean;
  disabled?: boolean;
  /** The axes whose values differ across a multi-selection: shown as a dash. */
  mixed?: boolean[];
}) {
  return (
    <div className={`vec-field vec-${values.length}`}>
      {values.map((v, i) => (
        <NumberField
          key={i}
          value={v}
          step={step}
          unit={unit}
          integer={integer}
          disabled={disabled}
          mixed={mixed?.[i]}
          handle={(labels?.[i] ?? AXES[i] ?? String(i)).toUpperCase()}
          handleClass={`axis-${AXES[i] ?? "w"}`}
          onCancel={onCancel}
          onChange={(nv, phase) => {
            const next = [...values];
            next[i] = nv;
            onChange(next, phase, i);
          }}
        />
      ))}
    </div>
  );
}
