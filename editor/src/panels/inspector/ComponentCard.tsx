// A component in the inspector: a collapsible card built from the component's JSON Schema, whose
// edits are `world.edit`s (drags stream as one grouped edit; see LiveEdit).

import { Bug, ChevronRight, ClipboardCopy, EllipsisVertical, Info, RotateCcw, Trash } from "lucide-react";
import { useRef, useState } from "react";
import { addDataWatch } from "../../actions/debug";
import { LiveEdit, removeComponent, setFields } from "../../actions/world";
import type { ComponentInfo, EntityId } from "../../host/protocol";
import { componentFields, defaultComponent } from "../../host/schema";
import { componentIcon } from "../../ui/icons";
import { contextMenu } from "../../ui/Menu";
import { cx } from "../../ui/cx";
import { entityName } from "../../state/world";
import { FieldRow } from "./FieldEditor";
import type { EditPhase } from "../../ui/fields/NumberField";

const FOLD_KEY = "aipocket2.editor.inspector.folded";

function loadFolded(): Set<string> {
  try {
    return new Set(JSON.parse(localStorage.getItem(FOLD_KEY) ?? "[]") as string[]);
  } catch {
    return new Set();
  }
}

const folded = loadFolded();

export function ComponentCard({ info, ids, value }: { info: ComponentInfo; ids: EntityId[]; value: unknown }) {
  const [open, setOpen] = useState(() => !folded.has(info.name));
  const live = useRef(new Map<string, LiveEdit>());
  const fields = componentFields(info);
  const Icon = componentIcon(info.name);
  const target = ids.length === 1 ? entityName(ids[0]) : `${ids.length} entities`;
  const obj = (value && typeof value === "object" ? value : {}) as Record<string, unknown>;

  const toggle = () => {
    const next = !open;
    setOpen(next);
    if (next) folded.delete(info.name);
    else folded.add(info.name);
    localStorage.setItem(FOLD_KEY, JSON.stringify([...folded]));
  };

  const change = (field: string, v: unknown, phase: EditPhase) => {
    const label = `Set ${info.name}.${field} on ${target}`;
    if (phase === "live") {
      let edit = live.current.get(field);
      if (!edit) {
        edit = new LiveEdit(label);
        live.current.set(field, edit);
      }
      for (const id of ids) edit.update(id, info.name, { [field]: v });
      return;
    }
    const edit = live.current.get(field);
    if (edit) {
      live.current.delete(field);
      for (const id of ids) edit.update(id, info.name, { [field]: v });
      void edit.commit();
    } else {
      void setFields(ids, info.name, { [field]: v }, label);
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
          {fields ? (
            fields.map((f) => (
              <FieldRow key={f.name} spec={f} value={obj[f.name]} onChange={(v, phase) => change(f.name, v, phase)} onCancel={() => cancel(f.name)} />
            ))
          ) : (
            <div className="card-note">This component's value is not an object.</div>
          )}
        </div>
      )}
    </section>
  );
}
