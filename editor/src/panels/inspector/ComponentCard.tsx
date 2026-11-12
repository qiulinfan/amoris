// A component in the inspector: a collapsible card built from the component's JSON Schema, whose
// edits are `world.edit`s (drags stream as one grouped edit; see LiveEdit).

import { Bug, ChevronRight, ClipboardCopy, EllipsisVertical, Info, RotateCcw, Trash } from "lucide-react";
import { useRef, useState } from "react";
import { addDataWatch } from "../../actions/debug";
import { LiveEdit, removeComponent, setFields, setFieldsEach } from "../../actions/world";
import type { ComponentInfo, EntityId } from "../../host/protocol";
import { componentFields, defaultComponent } from "../../host/schema";
import { componentIcon } from "../../ui/icons";
import { contextMenu } from "../../ui/Menu";
import { cx } from "../../ui/cx";
import { componentValue, entityName } from "../../state/world";
import { FieldRow } from "./FieldEditor";
import { fieldOf, type Update } from "./multi";
import type { EditPhase } from "../../ui/fields/NumberField";

const FOLD_KEY = "amoris.editor.inspector.folded";

function loadFolded(): Set<string> {
  try {
    return new Set(JSON.parse(localStorage.getItem(FOLD_KEY) ?? "[]") as string[]);
  } catch {
    return new Set();
  }
}

const folded = loadFolded();

/**
 * `ids` are the selected entities that have the component, the primary first, and `values` their
 * values of it in the same order; `selected` is how many entities are selected in all.
 */
export function ComponentCard({ info, ids, values, selected }: { info: ComponentInfo; ids: EntityId[]; values: unknown[]; selected: number }) {
  const [open, setOpen] = useState(() => !folded.has(info.name));
  const live = useRef(new Map<string, LiveEdit>());
  const fields = componentFields(info);
  const Icon = componentIcon(info.name);
  const target = ids.length === 1 ? entityName(ids[0]) : `${ids.length} entities`;
  const value = values[0];

  const toggle = () => {
    const next = !open;
    setOpen(next);
    if (next) folded.delete(info.name);
    else folded.add(info.name);
    localStorage.setItem(FOLD_KEY, JSON.stringify([...folded]));
  };

  // Each entity's new value of the field: the update applied to its own value, shown or in flight.
  const patches = (field: string, update: Update) =>
    ids.map((id): [EntityId, Record<string, unknown>] => [id, { [field]: update(fieldOf([componentValue(id, info.name)], field)[0]) }]);

  const change = (field: string, update: Update, phase: EditPhase) => {
    const label = `Set ${info.name}.${field} on ${target}`;
    if (phase === "live") {
      let edit = live.current.get(field);
      if (!edit) {
        edit = new LiveEdit(label);
        live.current.set(field, edit);
      }
      for (const [id, patch] of patches(field, update)) edit.update(id, info.name, patch);
      return;
    }
    const edit = live.current.get(field);
    if (edit) {
      live.current.delete(field);
      for (const [id, patch] of patches(field, update)) edit.update(id, info.name, patch);
      void edit.commit();
    } else {
      void setFieldsEach(info.name, patches(field, update), label);
    }
  };

  const cancel = (field: string) => {
    const edit = live.current.get(field);
    live.current.delete(field);
    void edit?.cancel();
  };

  const menu = (e: React.MouseEvent) =>
    contextMenu(e, [
      { label: "Reset to Defaults", icon: <RotateCcw size={14} />, run: () => void setFields(ids, info.name, defaultComponent(info) as Record<string, unknown>, `Reset ${info.name} on ${target}`) },
      { label: "Copy as JSON", icon: <ClipboardCopy size={14} />, run: () => void navigator.clipboard?.writeText(JSON.stringify({ [info.name]: value }, null, 2)) },
      {
        label: "Break When Written",
        icon: <Bug size={14} />,
        disabled: ids.length !== 1,
        submenu: [
          { label: `Any field of ${info.name}`, run: () => void addDataWatch(ids[0]!, info.name) },
          "separator",
          ...(fields ?? []).map((f) => ({ label: f.name, run: () => void addDataWatch(ids[0]!, info.name, f.name) })),
        ],
      },
      "separator",
      { label: `Remove ${info.name}`, icon: <Trash size={14} />, danger: true, run: () => void removeComponent(ids, info.name) },
    ]);

  return (
    <section className={cx("card", open && "is-open")} onContextMenu={menu}>
      <header className="card-head" onClick={toggle}>
        <ChevronRight size={13} className="card-chev" />
        <Icon size={14} className="card-icon" />
        <span className="card-title">{info.name}</span>
        {selected > 1 && ids.length < selected && (
          <span className="card-some" data-tip={`Only ${ids.length} of the ${selected} selected entities have ${info.name}; edits change those`}>
            {ids.length} of {selected}
          </span>
        )}
        <span className={cx("origin", info.origin === "Project" ? "is-project" : "is-engine")} data-tip={info.origin === "Project" ? "Declared by the project in TypeScript" : "An engine component (Rust)"}>
          {info.origin === "Project" ? "TS" : "Engine"}
        </span>
        <span className="card-doc" data-tip={info.doc} onClick={(e) => e.stopPropagation()}>
          <Info size={13} />
        </span>
        <button
          type="button"
          className="card-menu"
          aria-label="Component menu"
          onClick={(e) => {
            e.stopPropagation();
            menu(e);
          }}
        >
          <EllipsisVertical size={14} />
        </button>
      </header>
      {open && (
        <div className="card-body">
          {!fields ? (
            <div className="card-note">This component's value is not an object.</div>
          ) : fields.length === 0 ? (
            <div className="card-note">This component has no fields: it marks the entity.</div>
          ) : (
            fields.map((f) => (
              <FieldRow key={f.name} spec={f} values={fieldOf(values, f.name)} onChange={(u, phase) => change(f.name, u, phase)} onCancel={() => cancel(f.name)} />
            ))
          )}
        </div>
      )}
    </section>
  );
}
