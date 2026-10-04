// The world as the editor last read it from the host: the hierarchy (`world.tree`), the component
// values of every entity (`world.get`, refreshed on `world.changed`), the component registry
// (`world.schema`) and local previews that show a drag before the host has answered.

import { create } from "zustand";
import type { ComponentInfo, EntityData, EntityId, TreeNode } from "../host/protocol";

export interface FlatNode {
  id: EntityId;
  name: string;
  components: string[];
  parent: EntityId | null;
  children: EntityId[];
  depth: number;
}

interface WorldState {
  tree: TreeNode[];
  nodes: Record<EntityId, FlatNode>;
  /** Every entity in hierarchy order (depth first). */
  order: EntityId[];
  entities: Record<EntityId, EntityData>;
  schema: Record<string, ComponentInfo>;
  schemaList: ComponentInfo[];
  /** Component values shown instead of the host's while a drag is in flight. */
  preview: Record<EntityId, Record<string, unknown>>;
  setTree(tree: TreeNode[]): void;
  setEntity(e: EntityData): void;
  removeEntities(ids: EntityId[]): void;
  setSchema(list: ComponentInfo[]): void;
  setPreview(id: EntityId, component: string, value: unknown): void;
  clearPreview(ids?: EntityId[]): void;
  reset(): void;
}

function flatten(tree: TreeNode[]) {
  const nodes: Record<EntityId, FlatNode> = {};
  const order: EntityId[] = [];
  const walk = (n: TreeNode, parent: EntityId | null, depth: number) => {
    nodes[n.id] = { id: n.id, name: n.name, components: n.components, parent, children: n.children.map((c) => c.id), depth };
    order.push(n.id);
    n.children.forEach((c) => walk(c, n.id, depth + 1));
  };
  tree.forEach((n) => walk(n, null, 0));
  return { nodes, order };
}

export const useWorld = create<WorldState>((set) => ({
  tree: [],
  nodes: {},
  order: [],
  entities: {},
  schema: {},
  schemaList: [],
  preview: {},
  setTree: (tree) =>
    set((s) => {
      const { nodes, order } = flatten(tree);
      const entities: Record<EntityId, EntityData> = {};
      for (const id of order) if (s.entities[id]) entities[id] = s.entities[id]!;
      return { tree, nodes, order, entities };
    }),
  setEntity: (e) => set((s) => ({ entities: { ...s.entities, [e.id]: e } })),
  removeEntities: (ids) =>
    set((s) => {
      const entities = { ...s.entities };
      for (const id of ids) delete entities[id];
      return { entities };
    }),
  setSchema: (list) =>
    set({
      schemaList: [...list].sort((a, b) => a.name.localeCompare(b.name)),
      schema: Object.fromEntries(list.map((c) => [c.name, c])),
    }),
  setPreview: (id, component, value) =>
    set((s) => ({ preview: { ...s.preview, [id]: { ...(s.preview[id] ?? {}), [component]: value } } })),
  clearPreview: (ids) =>
    set((s) => {
      if (!ids) return { preview: {} };
      const preview = { ...s.preview };
      for (const id of ids) delete preview[id];
      return { preview };
    }),
  reset: () => set({ tree: [], nodes: {}, order: [], entities: {}, preview: {} }),
}));

/** An entity's component value, preview first. */
export function componentValue(id: EntityId, component: string): unknown {
  const s = useWorld.getState();
  return s.preview[id]?.[component] ?? s.entities[id]?.components[component];
}

export function entityName(id: EntityId | null | undefined): string {
  if (id === null || id === undefined) return "None";
  const s = useWorld.getState();
  return s.nodes[id]?.name ?? s.entities[id]?.name ?? `#${id}`;
}

export function hasComponent(name: string): boolean {
  return name in useWorld.getState().schema;
}
