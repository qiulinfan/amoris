// The inspector: the selected entity's name and components, each a card generated from its JSON
// Schema. With several entities selected it shows the primary one and edits every selected entity
// that has the component.

import { SlidersHorizontal } from "lucide-react";
import { useMemo } from "react";
import { renameEntity } from "../../actions/world";
import { useSelection } from "../../state/selection";
import { useWorld } from "../../state/world";
import { entityKind } from "../../ui/icons";
import { EmptyState, PanelShell } from "../../ui/Panel";
import { TextField } from "../../ui/fields/TextField";
import { AddComponent } from "./AddComponent";
import { ComponentCard } from "./ComponentCard";
import "./inspector.css";

export function InspectorPanel() {
  const { ids, primary } = useSelection();
  const entity = useWorld((s) => (primary !== null ? s.entities[primary] : undefined));
  const preview = useWorld((s) => (primary !== null ? s.preview[primary] : undefined));
  const node = useWorld((s) => (primary !== null ? s.nodes[primary] : undefined));
  const schema = useWorld((s) => s.schema);
  const components = useMemo(() => ({ ...(entity?.components ?? {}), ...(preview ?? {}) }), [entity, preview]);
  const names = useMemo(() => Object.keys(components).filter((c) => c !== "Name").sort(order), [components]);
  const present = useMemo(() => new Set(names), [names]);

  if (primary === null) {
    return (
      <PanelShell>
        <EmptyState icon={SlidersHorizontal} title="Nothing selected">
          Select an entity in the hierarchy or click it in the viewport.
        </EmptyState>
      </PanelShell>
    );
  }
  if (!entity) {
    return (
      <PanelShell>
        <EmptyState icon={SlidersHorizontal} title="Loading…" />
      </PanelShell>
    );
  }
  const light = (components.Light as { kind?: string } | undefined)?.kind;
  const kind = entityKind(names, light);
  const Icon = kind.icon;
  const name = node?.name ?? entity.name ?? `#${entity.id}`;
  return (
    <PanelShell className="inspector">
      <div className="insp-head">
        <span className={`insp-icon ${kind.tone}`}>
          <Icon size={18} />
        </span>
        <div className="insp-name">
          <TextField value={name} maxLength={64} onCommit={(v) => void renameEntity(entity.id, v)} />
          <div className="insp-meta">
            <span className="mono">#{entity.id}</span>
            <span>{names.length} components</span>
            {ids.length > 1 && <span className="insp-multi">editing {ids.length} selected</span>}
          </div>
        </div>
      </div>
      {names.map((c) =>
        schema[c] ? (
          <ComponentCard key={c} info={schema[c]!} ids={ids.length > 1 ? ids : [entity.id]} value={components[c]} />
        ) : (
          <section key={c} className="card is-open">
            <header className="card-head">
              <span className="card-title">{c}</span>
              <span className="origin">unregistered</span>
            </header>
            <pre className="card-raw">{JSON.stringify(components[c], null, 2)}</pre>
          </section>
        ),
      )}
      <AddComponent ids={ids.length > 1 ? ids : [entity.id]} present={present} />
    </PanelShell>
  );
}

/** Transform first, then engine components, then project ones, alphabetically within each. */
const FIRST = ["Transform", "Model", "Light", "Camera"];
function order(a: string, b: string): number {
  const ia = FIRST.indexOf(a);
  const ib = FIRST.indexOf(b);
  if (ia >= 0 || ib >= 0) return (ia < 0 ? 99 : ia) - (ib < 0 ? 99 : ib);
  const sa = useWorld.getState().schema[a]?.origin === "Project" ? 1 : 0;
  const sb = useWorld.getState().schema[b]?.origin === "Project" ? 1 : 0;
  return sa - sb || a.localeCompare(b);
}
