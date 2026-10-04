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
}: {
  values: number[];
  onChange(values: number[], phase: EditPhase): void;
  onCancel?(): void;
  labels?: string[];
  step?: number;
  unit?: string;
  integer?: boolean;
  disabled?: boolean;
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
          handle={(labels?.[i] ?? AXES[i] ?? String(i)).toUpperCase()}
          handleClass={`axis-${AXES[i] ?? "w"}`}
          onCancel={onCancel}
          onChange={(nv, phase) => {
            const next = [...values];
            next[i] = nv;
            onChange(next, phase);
          }}
        />
      ))}
    </div>
  );
}
