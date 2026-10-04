// One field of a component, edited by its kind (schema.ts): numbers scrub, vectors per axis,
// rotations as Euler degrees over a stored quaternion, colours with a picker, enums as selects,
// booleans as switches, entity references as pickers, objects and lists nested, the rest as JSON.

import { Minus, Plus } from "lucide-react";
import { useRef, useState } from "react";
import { defaultFor, readVector, writeVector, type FieldKind, type FieldSpec } from "../../host/schema";
import { eulerFromQuat, quatClose, quatFromEuler, type Quat, type Vec3 } from "../../viewport/math";
import { ColorField } from "../../ui/fields/ColorField";
import { EntityField } from "../../ui/fields/EntityField";
import { NumberField, type EditPhase } from "../../ui/fields/NumberField";
import { SelectField } from "../../ui/fields/SelectField";
import { TextField } from "../../ui/fields/TextField";
import { Toggle } from "../../ui/fields/Toggle";
import { VectorField } from "../../ui/fields/VectorField";
import { cx } from "../../ui/cx";

export type OnChange = (value: unknown, phase: EditPhase) => void;

function QuatField({ value, form, onChange, onCancel }: { value: unknown; form: "array" | "object"; onChange: OnChange; onCancel?(): void }) {
  const q = readVector(value, form, ["x", "y", "z", "w"]) as Quat;
  // Keep the angles the user typed while the stored quaternion still matches them, so dragging Y
  // past 90 degrees does not flip X and Z.
  const shown = useRef<{ q: Quat; e: Vec3 } | null>(null);
  if (!shown.current || !quatClose(shown.current.q, q)) shown.current = { q, e: eulerFromQuat(q) };
  const euler = shown.current.e;
  return (
    <VectorField
      values={euler}
      step={0.5}
      onCancel={onCancel}
      onChange={(next, phase) => {
        const nq = quatFromEuler(next as Vec3).map((v) => Math.round(v * 1e7) / 1e7) as Quat;
        shown.current = { q: nq, e: next as Vec3 };
        onChange(writeVector(nq, form, ["x", "y", "z", "w"]), phase);
      }}
    />
  );
}

function JsonField({ value, onChange }: { value: unknown; onChange: OnChange }) {
  const [text, setText] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const shown = text ?? JSON.stringify(value, null, 1);
  return (
    <div className="json-field">
      <textarea
        className={cx("json-input", error && "is-error")}
        value={shown}
        rows={Math.min(8, shown.split("\n").length)}
        spellCheck={false}
        onChange={(e) => setText(e.target.value)}
        onBlur={() => {
          if (text === null) return;
          try {
            const v = JSON.parse(text) as unknown;
            setError(null);
            setText(null);
            onChange(v, "commit");
          } catch (e) {
            setError(e instanceof Error ? e.message : "Invalid JSON");
          }
        }}
      />
      {error && <div className="field-error">{error}</div>}
    </div>
  );
}

export function KindEditor({ kind, value, onChange, onCancel, disabled }: { kind: FieldKind; value: unknown; onChange: OnChange; onCancel?(): void; disabled?: boolean }) {
  switch (kind.kind) {
    case "number":
      return (
        <NumberField
          value={typeof value === "number" ? value : 0}
          min={kind.min}
          max={kind.max}
          integer={kind.integer}
          unit={kind.unit}
          disabled={disabled}
          handle=""
          handleClass="scrub-strip"
          onChange={onChange}
          onCancel={onCancel}
        />
      );
    case "boolean":
      return <Toggle value={value === true} disabled={disabled} onChange={(v) => onChange(v, "commit")} />;
    case "string":
      return <TextField value={typeof value === "string" ? value : ""} maxLength={kind.maxLength} disabled={disabled} onCommit={(v) => onChange(v, "commit")} />;
    case "enum":
      return <SelectField value={String(value ?? "")} options={kind.options} disabled={disabled} onChange={(v) => onChange(v, "commit")} />;
    case "vector":
      return (
        <VectorField
          values={readVector(value, kind.form, kind.keys)}
          disabled={disabled}
          onCancel={onCancel}
          onChange={(v, phase) => onChange(writeVector(v, kind.form, kind.keys), phase)}
        />
      );
    case "quaternion":
      return <QuatField value={value} form={kind.form} onChange={onChange} onCancel={onCancel} />;
    case "color":
      return <ColorField values={readVector(value, kind.form, kind.keys)} disabled={disabled} onChange={(v, phase) => onChange(writeVector(v, kind.form, kind.keys), phase)} />;
    case "entity":
      return <EntityField value={typeof value === "number" ? value : null} disabled={disabled} onChange={(v) => onChange(v, "commit")} />;
    case "nullable":
      return (
        <div className="nullable-field">
          <Toggle value={value !== null && value !== undefined} label="Set" onChange={(on) => onChange(on ? defaultFor(kind.inner) : null, "commit")} />
          {value !== null && value !== undefined ? (
            <div className="nullable-inner">
              <KindEditor kind={kind.inner} value={value} onChange={onChange} onCancel={onCancel} disabled={disabled} />
            </div>
          ) : (
            <span className="none-label">None</span>
          )}
        </div>
      );
    case "object": {
      const obj = (value && typeof value === "object" ? value : {}) as Record<string, unknown>;
      return (
        <div className="nested">
          {kind.fields.map((f) => (
            <FieldRow key={f.name} spec={f} value={obj[f.name]} onChange={(v, phase) => onChange({ ...obj, [f.name]: v }, phase)} onCancel={onCancel} />
          ))}
        </div>
      );
    }
    case "variant": {
      const tag = typeof value === "string" ? value : value && typeof value === "object" ? Object.keys(value)[0] ?? "" : "";
      const variant = kind.variants.find((v) => v.name === tag);
      const payload = value && typeof value === "object" ? (value as Record<string, unknown>)[tag] : undefined;
      return (
        <div className="variant-field">
          <SelectField
            value={tag}
            options={kind.variants.map((v) => ({ value: v.name, doc: v.doc }))}
            onChange={(t) => {
              const v = kind.variants.find((x) => x.name === t);
              onChange(v?.payload ? { [t]: defaultFor(v.payload) } : t, "commit");
            }}
          />
          {variant?.payload && (
            <div className="nested">
              <KindEditor kind={variant.payload} value={payload} onChange={(p, phase) => onChange({ [tag]: p }, phase)} onCancel={onCancel} />
            </div>
          )}
        </div>
      );
    }
    case "list": {
      const list = Array.isArray(value) ? value : [];
      return (
        <div className="list-field">
          {list.map((item, i) => (
            <div key={i} className="list-item">
              <div className="list-head">
                <span className="list-index">{i}</span>
                <button type="button" className="mini-btn" aria-label="Remove item" onClick={() => onChange(list.filter((_, k) => k !== i), "commit")}>
                  <Minus size={11} />
                </button>
              </div>
              <KindEditor kind={kind.item} value={item} onChange={(v, phase) => onChange(list.map((x, k) => (k === i ? v : x)), phase)} onCancel={onCancel} />
            </div>
          ))}
          <button type="button" className="add-item" onClick={() => onChange([...list, defaultFor(kind.item)], "commit")}>
            <Plus size={12} /> Add item
          </button>
        </div>
      );
    }
    case "json":
      return <JsonField value={value} onChange={onChange} />;
  }
}

export function FieldRow({ spec, value, onChange, onCancel }: { spec: FieldSpec; value: unknown; onChange: OnChange; onCancel?(): void }) {
  const stacked = spec.kind.kind === "object" || spec.kind.kind === "list" || spec.kind.kind === "json" || (spec.kind.kind === "variant" && spec.kind.variants.some((v) => v.payload));
  return (
    <div className={cx("field-row", stacked && "is-stacked", spec.engineWritten && "is-engine")}>
      <label className="field-label" data-tip={spec.doc ? `${spec.name}: ${spec.doc}` : spec.name}>
        {spec.label}
        {spec.kind.kind === "quaternion" && <span className="label-unit" data-tip="Euler degrees (YXZ); stored as a quaternion">°</span>}
        {spec.engineWritten && <span className="engine-dot" />}
      </label>
      <div className="field-value">
        <KindEditor kind={spec.kind} value={value} onChange={onChange} onCancel={onCancel} />
      </div>
    </div>
  );
}
