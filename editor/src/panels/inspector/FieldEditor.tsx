// One field of a component, edited by its kind (schema.ts): numbers scrub, vectors per axis,
// rotations as Euler degrees over a stored quaternion, colours with a picker, enums as selects,
// booleans as switches, entity references as pickers, objects and lists nested, the rest as JSON.
// With several entities selected a value that differs among them shows as mixed (multi.ts).

import { Minus, Plus } from "lucide-react";
import { useRef, useState } from "react";
import { defaultFor, readVector, type FieldKind, type FieldSpec } from "../../host/schema";
import { eulerFromQuat, quatClose, type Quat, type Vec3 } from "../../viewport/math";
import { ColorField } from "../../ui/fields/ColorField";
import { EntityField } from "../../ui/fields/EntityField";
import { NumberField, type EditPhase } from "../../ui/fields/NumberField";
import { SelectField } from "../../ui/fields/SelectField";
import { TextField } from "../../ui/fields/TextField";
import { Toggle } from "../../ui/fields/Toggle";
import { VectorField } from "../../ui/fields/VectorField";
import { cx } from "../../ui/cx";
import { fieldOf, inField, isMixed, mixedAngles, mixedAxes, withAngle, withAxis, withRgb, type Update } from "./multi";

/** An edit of the field: what it makes of each selected entity's old value, and its phase. */
export type OnChange = (update: Update, phase: EditPhase) => void;

const to = (v: unknown): Update => () => v;

function QuatField({ values, form, onChange, onCancel }: { values: unknown[]; form: "array" | "object"; onChange: OnChange; onCancel?(): void }) {
  const q = readVector(values[0], form, ["x", "y", "z", "w"]) as Quat;
  // Keep the angles the user typed while the stored quaternion still matches them, so dragging Y
  // past 90 degrees does not flip X and Z.
  const shown = useRef<{ q: Quat; e: Vec3 } | null>(null);
  if (!shown.current || !quatClose(shown.current.q, q)) shown.current = { q, e: eulerFromQuat(q) };
  const euler = shown.current.e;
  return (
    <VectorField
      values={euler}
      step={0.5}
      mixed={values.length > 1 ? mixedAngles(values, form) : undefined}
      onCancel={onCancel}
      onChange={(next, phase, axis) => {
        const update = withAngle(form, axis, next[axis]!, shown.current);
        // The primary's new angles are the ones shown from now on.
        const nq = readVector(update(values[0]), form, ["x", "y", "z", "w"]) as Quat;
        shown.current = { q: nq, e: [...euler.slice(0, axis), next[axis]!, ...euler.slice(axis + 1)] as Vec3 };
        onChange(update, phase);
      }}
    />
  );
}

function JsonField({ value, mixed, onChange }: { value: unknown; mixed: boolean; onChange: OnChange }) {
  const [text, setText] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const shown = text ?? (mixed ? "" : JSON.stringify(value, null, 1));
  return (
    <div className="json-field">
      <textarea
        className={cx("json-input", error && "is-error")}
        value={shown}
        placeholder={mixed ? "— (differs across the selection; JSON typed here sets every one)" : undefined}
        rows={Math.max(mixed ? 2 : 1, Math.min(8, shown.split("\n").length))}
        spellCheck={false}
        onChange={(e) => setText(e.target.value)}
        onBlur={() => {
          if (text === null) return;
          try {
            const v = JSON.parse(text) as unknown;
            setError(null);
            setText(null);
            onChange(to(v), "commit");
          } catch (e) {
            setError(e instanceof Error ? e.message : "Invalid JSON");
          }
        }}
      />
      {error && <div className="field-error">{error}</div>}
    </div>
  );
}

/**
 * The editor of one field's kind. `values` holds the field's value on each selected entity that has
 * the component, the primary's first (one value with one entity selected); a value that differs
 * across them shows as mixed, and an edit is an `Update` each entity's value goes through.
 */
export function KindEditor({ kind, values, onChange, onCancel, disabled }: { kind: FieldKind; values: unknown[]; onChange: OnChange; onCancel?(): void; disabled?: boolean }) {
  const value = values[0];
  const mixed = isMixed(values);
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
          mixed={mixed}
          handle=""
          handleClass="scrub-strip"
          onChange={(v, phase) => onChange(to(v), phase)}
          onCancel={onCancel}
        />
      );
    case "boolean":
      return <Toggle value={value === true} mixed={mixed} disabled={disabled} onChange={(v) => onChange(to(v), "commit")} />;
    case "string":
      return (
        <TextField
          value={mixed ? "" : typeof value === "string" ? value : ""}
          placeholder={mixed ? "—" : undefined}
          maxLength={kind.maxLength}
          disabled={disabled}
          onCommit={(v) => onChange(to(v), "commit")}
        />
      );
    case "enum":
      return <SelectField value={mixed ? "" : String(value ?? "")} options={kind.options} disabled={disabled} onChange={(v) => onChange(to(v), "commit")} />;
    case "vector":
      return (
        <VectorField
          values={readVector(value, kind.form, kind.keys)}
          mixed={mixed ? mixedAxes(values.map((v) => readVector(v, kind.form, kind.keys))) : undefined}
          disabled={disabled}
          onCancel={onCancel}
          onChange={(v, phase, axis) => onChange(withAxis(kind.form, kind.keys, axis, v[axis]!), phase)}
        />
      );
    case "quaternion":
      return <QuatField values={values} form={kind.form} onChange={onChange} onCancel={onCancel} />;
    case "color": {
      const all = values.map((v) => readVector(v, kind.form, kind.keys));
      const axes = mixed ? mixedAxes(all) : [];
      return (
        <ColorField
          values={all[0]!}
          mixed={mixed ? { rgb: axes.slice(0, 3).some(Boolean), alpha: axes[3] === true } : undefined}
          disabled={disabled}
          onChange={(v, phase, part) => onChange(part === "rgb" ? withRgb(kind.form, kind.keys, v) : withAxis(kind.form, kind.keys, 3, v[3]!), phase)}
        />
      );
    }
    case "entity":
      return <EntityField value={typeof value === "number" ? value : null} mixed={mixed} disabled={disabled} onChange={(v) => onChange(to(v), "commit")} />;
    case "nullable": {
      const set = values.filter((v) => v !== null && v !== undefined);
      const some = set.length > 0 && set.length < values.length;
      return (
        <div className="nullable-field">
          <Toggle
            value={set.length > 0}
            mixed={some}
            label="Set"
            onChange={(on) => onChange((old) => (on ? (old ?? defaultFor(kind.inner)) : null), "commit")}
          />
          {set.length === values.length ? (
            <div className="nullable-inner">
              <KindEditor kind={kind.inner} values={values} onChange={onChange} onCancel={onCancel} disabled={disabled} />
            </div>
          ) : (
            <span className="none-label">{some ? "— (set on some)" : "None"}</span>
          )}
        </div>
      );
    }
    case "object":
      return (
        <div className="nested">
          {kind.fields.map((f) => (
            <FieldRow key={f.name} spec={f} values={fieldOf(values, f.name)} onChange={(u, phase) => onChange(inField(f.name, u), phase)} onCancel={onCancel} />
          ))}
        </div>
      );
    case "variant": {
      const tagOf = (v: unknown) => (typeof v === "string" ? v : v && typeof v === "object" ? (Object.keys(v)[0] ?? "") : "");
      const tags = values.map(tagOf);
      const tag = tags[0]!;
      const oneTag = tags.every((t) => t === tag);
      const variant = kind.variants.find((v) => v.name === tag);
      return (
        <div className="variant-field">
          <SelectField
            value={oneTag ? tag : ""}
            options={kind.variants.map((v) => ({ value: v.name, doc: v.doc }))}
            onChange={(t) => {
              const v = kind.variants.find((x) => x.name === t);
              // An entity already of that variant keeps its payload.
              onChange((old) => (tagOf(old) === t ? old : v?.payload ? { [t]: defaultFor(v.payload) } : t), "commit");
            }}
          />
          {oneTag && variant?.payload && (
            <div className="nested">
              <KindEditor kind={variant.payload} values={fieldOf(values, tag)} onChange={(u, phase) => onChange((old) => ({ [tag]: u(tagOf(old) === tag ? fieldOf([old], tag)[0] : undefined) }), phase)} onCancel={onCancel} />
            </div>
          )}
        </div>
      );
    }
    case "list": {
      if (mixed) {
        return <div className="card-note">— The lists differ across the selection; select one entity to edit its list.</div>;
      }
      const list = Array.isArray(value) ? value : [];
      return (
        <div className="list-field">
          {list.map((_, i) => (
            <div key={i} className="list-item">
              <div className="list-head">
                <span className="list-index">{i}</span>
                <button type="button" className="mini-btn" aria-label="Remove item" onClick={() => onChange(to(list.filter((_, k) => k !== i)), "commit")}>
                  <Minus size={11} />
                </button>
              </div>
              <KindEditor
                kind={kind.item}
                values={values.map((l) => (Array.isArray(l) ? l[i] : undefined))}
                onChange={(u, phase) => onChange((old) => (Array.isArray(old) ? old : list).map((x, k) => (k === i ? u(x) : x)), phase)}
                onCancel={onCancel}
              />
            </div>
          ))}
          <button type="button" className="add-item" onClick={() => onChange(to([...list, defaultFor(kind.item)]), "commit")}>
            <Plus size={12} /> Add item
          </button>
        </div>
      );
    }
    case "json":
      return <JsonField value={value} mixed={mixed} onChange={onChange} />;
  }
}

export function FieldRow({ spec, values, onChange, onCancel }: { spec: FieldSpec; values: unknown[]; onChange: OnChange; onCancel?(): void }) {
  const stacked = spec.kind.kind === "object" || spec.kind.kind === "list" || spec.kind.kind === "json" || (spec.kind.kind === "variant" && spec.kind.variants.some((v) => v.payload));
  return (
    <div className={cx("field-row", stacked && "is-stacked", spec.engineWritten && "is-engine", isMixed(values) && "is-mixed")}>
      <label className="field-label" data-tip={spec.doc ? `${spec.name}: ${spec.doc}` : spec.name}>
        {spec.label}
        {spec.kind.kind === "quaternion" && <span className="label-unit" data-tip="Euler degrees (YXZ); stored as a quaternion">°</span>}
        {spec.engineWritten && <span className="engine-dot" />}
      </label>
      <div className="field-value">
        <KindEditor kind={spec.kind} values={values} onChange={onChange} onCancel={onCancel} />
      </div>
    </div>
  );
}
