// The hierarchy: every entity as a tree (children by `Parent`), with icons by kind, search (by name,
// `t:Component` or `#id`), multi-select (Cmd/Ctrl toggles, Shift extends), inline rename (F2), drag to
// reparent, asset drops, a create menu and the entity context menu.

import { ChevronsDownUp, ChevronsUpDown, Plus } from "lucide-react";
import { useMemo, useState } from "react";
import { canReparent, reparent, spawnAsset, renameEntity } from "../../actions/world";
import { ASSET_MIME, ENTITY_MIME } from "../../state/assets";
import { useSelection } from "../../state/selection";
import { useUi } from "../../state/ui";
import { useViewport } from "../../state/viewport";
import { useWorld, type FlatNode } from "../../state/world";
import { IconButton } from "../../ui/Button";
import { entityKind } from "../../ui/icons";
import { contextMenu, PopupMenu } from "../../ui/Menu";
import { EmptyState, PanelShell } from "../../ui/Panel";
import { SearchInput } from "../../ui/SearchInput";
import { Toolbar } from "../../ui/Toolbar";
import { Tree, type TreeRow } from "../../ui/Tree";
import { createMenu, entityMenu } from "./entityMenu";
import { ListTree } from "lucide-react";
import "./hierarchy.css";

function matcher(q: string): ((n: FlatNode) => [number, number] | true | false) | null {
  const t = q.trim();
  if (!t) return null;
  if (t.startsWith("t:")) {
    const c = t.slice(2).toLowerCase();
    return (n) => n.components.some((x) => x.toLowerCase().startsWith(c));
  }
  if (t.startsWith("#")) return (n) => String(n.id) === t.slice(1);
  const lower = t.toLowerCase();
  return (n) => {
    const i = n.name.toLowerCase().indexOf(lower);
    return i >= 0 ? [i, i + lower.length] : false;
  };
}

export function HierarchyPanel() {
  const tree = useWorld((s) => s.tree);
  const nodes = useWorld((s) => s.nodes);
  const order = useWorld((s) => s.order);
  const entities = useWorld((s) => s.entities);
  const { ids, primary } = useSelection();
  const renaming = useUi((s) => s.renaming);
  const [filter, setFilter] = useState("");
  const [collapsed, setCollapsed] = useState<Set<number>>(() => new Set());
  const [dropTarget, setDropTarget] = useState<number | "root" | null>(null);
  const [createAt, setCreateAt] = useState<{ x: number; y: number } | null>(null);

  const rows = useMemo(() => {
    const match = matcher(filter);
    const out: TreeRow[] = [];
    const keep = new Set<number>();
    const hits = new Map<number, [number, number] | true>();
    if (match) {
      for (const id of order) {
        const n = nodes[id]!;
        const m = match(n);
        if (m) {
          hits.set(id, m);
          for (let p: number | null = id; p !== null; p = nodes[p]?.parent ?? null) keep.add(p);
        }
      }
    }
    const walk = (id: number) => {
      const n = nodes[id];
      if (!n) return;
      if (match && !keep.has(id)) return;
      const light = (entities[id]?.components.Light as { kind?: string } | undefined)?.kind;
      const kind = entityKind(n.components, light);
      const Icon = kind.icon;
      const expanded = match ? true : !collapsed.has(id);
      const hit = hits.get(id);
      out.push({
        id,
        depth: n.depth,
        label: n.name,
        icon: <Icon size={14} className={kind.tone} />,
        hasChildren: n.children.length > 0,
        expanded,
        dim: !!match && !hit,
        match: Array.isArray(hit) ? hit : undefined,
        trailing: <span className="row-id">#{id}</span>,
      });
      if (expanded) n.children.forEach(walk);
    };
    tree.forEach((r) => walk(r.id));
    return out;
  }, [tree, nodes, order, entities, filter, collapsed]);

  const selected = useMemo(() => new Set(ids), [ids]);
  const visibleOrder = useMemo(() => rows.map((r) => r.id), [rows]);

  const select = (id: number, e: React.MouseEvent | React.KeyboardEvent) => {
    const s = useSelection.getState();
    if (e.shiftKey) s.range(id, visibleOrder);
    else if (e.metaKey || e.ctrlKey) s.toggle(id);
    else s.set([id]);
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    const i = primary !== null ? visibleOrder.indexOf(primary) : -1;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const next = visibleOrder[Math.max(0, Math.min(visibleOrder.length - 1, i + (e.key === "ArrowDown" ? 1 : -1)))];
      if (next !== undefined) select(next, e);
    } else if (e.key === "ArrowRight" && primary !== null) {
      e.preventDefault();
      setCollapsed((c) => {
        const n = new Set(c);
        n.delete(primary);
        return n;
      });
    } else if (e.key === "ArrowLeft" && primary !== null) {
      e.preventDefault();
      if (nodes[primary]?.children.length && !collapsed.has(primary)) setCollapsed((c) => new Set(c).add(primary));
      else if (nodes[primary]?.parent) useSelection.getState().set([nodes[primary]!.parent!]);
    } else if (e.key === "Enter" && primary !== null) {
      e.preventDefault();
      useUi.getState().set({ renaming: primary });
    }
  };

  const dragIds = (id: number) => (selected.has(id) ? ids : [id]);

  const acceptDrop = (e: React.DragEvent) => e.dataTransfer.types.includes(ENTITY_MIME) || e.dataTransfer.types.includes(ASSET_MIME);

  const drop = (e: React.DragEvent, target: number | null) => {
    setDropTarget(null);
    const asset = e.dataTransfer.getData(ASSET_MIME);
    if (asset) {
      e.preventDefault();
      void spawnAsset(asset, undefined, canReparent() ? target : null);
      return;
    }
    const raw = e.dataTransfer.getData(ENTITY_MIME);
    if (!raw) return;
    e.preventDefault();
    void reparent(JSON.parse(raw) as number[], target);
  };

  const toolbar = (
    <Toolbar>
      <IconButton icon={Plus} label="Create entity" onClick={(e) => {
        const r = e.currentTarget.getBoundingClientRect();
        setCreateAt({ x: r.left, y: r.bottom + 4 });
      }} />
      <SearchInput value={filter} onChange={setFilter} placeholder="Search  (t:Light, #12)" />
      <IconButton
        icon={collapsed.size ? ChevronsUpDown : ChevronsDownUp}
        label={collapsed.size ? "Expand all" : "Collapse all"}
        onClick={() => setCollapsed(collapsed.size ? new Set() : new Set(order.filter((id) => nodes[id]?.children.length)))}
      />
    </Toolbar>
  );

  return (
    <PanelShell toolbar={toolbar} scroll={false} className="hierarchy">
      {order.length === 0 ? (
        <EmptyState icon={ListTree} title="No entities">Create one with + or drop an asset here.</EmptyState>
      ) : (
        <Tree
          rows={rows}
          selected={selected}
          primary={primary}
          renaming={renaming}
          scrollToId={primary}
          dropTarget={dropTarget}
          onToggle={(id) =>
            setCollapsed((c) => {
              const n = new Set(c);
              if (n.has(id)) n.delete(id);
              else n.add(id);
              return n;
            })
          }
          onSelect={select}
          onActivate={() => useViewport.getState().frame()}
          onHover={(id) => useSelection.getState().hover(id)}
          onContextMenu={(id, e) => {
            if (id !== null && !selected.has(id)) useSelection.getState().set([id]);
            const target = id === null ? [] : selected.has(id) ? ids : [id];
            contextMenu(e, entityMenu(target));
          }}
          onRename={(id, name) => {
            useUi.getState().set({ renaming: null });
            if (name !== null) void renameEntity(id, name);
          }}
          onKeyDown={onKeyDown}
          dragProps={(id) => ({
            draggable: renaming !== id,
            onDragStart: (e) => {
              e.dataTransfer.setData(ENTITY_MIME, JSON.stringify(dragIds(id)));
              e.dataTransfer.effectAllowed = "move";
            },
            onDragOver: (e) => {
              if (!acceptDrop(e)) return;
              e.preventDefault();
              e.stopPropagation();
              setDropTarget(id);
            },
            onDrop: (e) => {
              e.stopPropagation();
              drop(e, id);
            },
          })}
          containerDrag={{
            onDragOver: (e) => {
              if (!acceptDrop(e)) return;
              e.preventDefault();
              setDropTarget("root");
            },
            onDragLeave: (e) => {
              if (e.currentTarget === e.target) setDropTarget(null);
            },
            onDrop: (e) => drop(e, null),
          }}
        />
      )}
      {createAt && <PopupMenu items={createMenu()} x={createAt.x} y={createAt.y} onClose={() => setCreateAt(null)} />}
    </PanelShell>
  );
}
