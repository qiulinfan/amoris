// The context menu of entities, shared by the hierarchy and the viewport.

import { Bug, ClipboardCopy, Copy, Focus, FolderTree, Pencil, Plus, Trash, Unlink } from "lucide-react";
import { addComponent, canReparent, deleteEntities, duplicateEntities, PRIMITIVES, primitiveAvailable, reparent, spawnPrimitive } from "../../actions/world";
import { addDataWatch } from "../../actions/debug";
import type { EntityId, Vec3 } from "../../host/protocol";
import type { MenuEntry } from "../../state/ui";
import { useUi } from "../../state/ui";
import { useViewport } from "../../state/viewport";
import { useWorld } from "../../state/world";
import { componentIcon } from "../../ui/icons";

export function createMenu(opts: { at?: Vec3; parent?: EntityId | null } = {}): MenuEntry[] {
  const out: MenuEntry[] = [];
  PRIMITIVES.forEach((p, i) => {
    // Groups: empty | meshes | lights | camera.
    if (i === 1 || i === 5 || i === 8) out.push("separator");
    out.push({
      label: p.label,
      disabled: !primitiveAvailable(p.kind),
      run: () => void spawnPrimitive(p.kind, opts),
    });
  });
  return out;
}

export function entityMenu(ids: EntityId[], opts: { at?: Vec3 } = {}): MenuEntry[] {
  const w = useWorld.getState();
  if (!ids.length) return [{ label: "Create", icon: <Plus size={14} />, submenu: createMenu({ at: opts.at }) }];
  const single = ids.length === 1 ? ids[0]! : null;
  const node = single !== null ? w.nodes[single] : undefined;
  const present = new Set(ids.flatMap((id) => Object.keys(w.entities[id]?.components ?? {})));
  const addable = w.schemaList.filter((c) => c.name !== "Name" && !present.has(c.name));
  const items: MenuEntry[] = [
    { label: "Rename", icon: <Pencil size={14} />, keys: "F2", disabled: single === null, run: () => useUi.getState().set({ renaming: single }) },
    { label: "Duplicate", icon: <Copy size={14} />, keys: "Mod+D", run: () => void duplicateEntities(ids) },
    { label: "Delete", icon: <Trash size={14} />, keys: "Delete", danger: true, run: () => void deleteEntities(ids) },
    "separator",
    { label: "Frame in Viewport", icon: <Focus size={14} />, keys: "F", run: () => useViewport.getState().frame() },
    {
      label: "Copy ID",
      icon: <ClipboardCopy size={14} />,
      run: () => void navigator.clipboard?.writeText(ids.join(", ")),
    },
    "separator",
    { label: "Create Child", icon: <FolderTree size={14} />, disabled: single === null || !canReparent(), submenu: createMenu({ parent: single }) },
    {
      label: "Add Component",
      icon: <Plus size={14} />,
      disabled: addable.length === 0,
      submenu: addable.map((c) => {
        const Icon = componentIcon(c.name);
        return { label: c.name, icon: <Icon size={14} />, run: () => void addComponent(ids, c.name) };
      }),
    },
  ];
  if (node?.parent !== null && node?.parent !== undefined) {
    items.push({ label: "Unparent", icon: <Unlink size={14} />, run: () => void reparent(ids, null) });
  }
  if (single !== null) {
    const comps = Object.keys(w.entities[single]?.components ?? {}).filter((c) => c !== "Name");
    items.push("separator", {
      label: "Break When Written",
      icon: <Bug size={14} />,
      submenu: comps.map((c) => ({ label: c, run: () => void addDataWatch(single, c) })),
    });
  }
  return items;
}
