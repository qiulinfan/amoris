// The asset browser: the project's assets as a grid of thumbnails (or a list), filtered by kind and
// name; drag one into the viewport or the hierarchy to place it; import files by picking them or
// dropping them on the panel (uploaded into the project, actions/assets.ts).

import { ClipboardCopy, Import, LayoutGrid, List, PackagePlus, RefreshCw } from "lucide-react";
import { useMemo, useState } from "react";
import { api } from "../../host/api";
import { chooseFiles, importFiles } from "../../actions/assets";
import { attempt } from "../../actions/report";
import { placeableKind, spawnAsset } from "../../actions/world";
import { ASSET_MIME, useAssets } from "../../state/assets";
import { Button, IconButton } from "../../ui/Button";
import { cx } from "../../ui/cx";
import { bytes } from "../../ui/format";
import { assetIcon } from "../../ui/icons";
import { contextMenu } from "../../ui/Menu";
import { EmptyState, PanelShell } from "../../ui/Panel";
import { SearchInput } from "../../ui/SearchInput";
import { Toolbar, ToolbarSeparator } from "../../ui/Toolbar";
import type { AssetInfo } from "../../host/protocol";
import "./assets.css";

const KINDS = [
  { id: "all", label: "All" },
  { id: "model", label: "Models" },
  { id: "texture", label: "Textures" },
  { id: "splat", label: "Splats" },
  { id: "neural", label: "Neural" },
  { id: "audio", label: "Audio" },
];

/** Import: the browser's file chooser, whose files are uploaded into the project. */
export function importAsset() {
  chooseFiles();
}

async function refreshAssets() {
  const list = await attempt(api.assets.list(), "assets.list");
  if (list) useAssets.getState().set(list);
}

const placeable = (a: AssetInfo) => placeableKind(a.kind);

function Thumb({ a, big }: { a: AssetInfo; big: boolean }) {
  const Icon = assetIcon(a.kind);
  if (a.thumbnail) return <img className="asset-img" src={a.thumbnail} alt="" draggable={false} />;
  return (
    <div className={`asset-placeholder kind-${a.kind}`}>
      <Icon size={big ? 30 : 14} strokeWidth={1.4} />
    </div>
  );
}

export function AssetsPanel() {
  const assets = useAssets((s) => s.assets);
  const [q, setQ] = useState("");
  const [kind, setKind] = useState("all");
  const [view, setView] = useState<"grid" | "list">(() => (localStorage.getItem("amoris.editor.assets.view") as "grid" | "list") ?? "grid");
  const [selected, setSelected] = useState<string | null>(null);
  const [dropping, setDropping] = useState(false);
  const shown = useMemo(
    () => assets.filter((a) => (kind === "all" || a.kind === kind) && (!q || a.path.toLowerCase().includes(q.toLowerCase()))),
    [assets, kind, q],
  );
  const total = shown.reduce((s, a) => s + a.bytes, 0);

  const menu = (e: React.MouseEvent, a: AssetInfo) => {
    setSelected(a.path);
    contextMenu(e, [
      { label: "Place in Scene", icon: <PackagePlus size={14} />, disabled: !placeable(a), run: () => void spawnAsset(a.path) },
      { label: "Copy Path", icon: <ClipboardCopy size={14} />, run: () => void navigator.clipboard?.writeText(a.path) },
    ]);
  };

  const card = (a: AssetInfo) => ({
    key: a.path,
    draggable: true,
    onDragStart: (e: React.DragEvent) => {
      // Only what can be placed is offered to the viewport and the hierarchy; any file's path can
      // still be dropped as text.
      if (placeable(a)) e.dataTransfer.setData(ASSET_MIME, a.path);
      e.dataTransfer.setData("text/plain", a.path);
      e.dataTransfer.effectAllowed = "copy";
    },
    onClick: () => setSelected(a.path),
    onDoubleClick: () => placeable(a) && void spawnAsset(a.path),
    onContextMenu: (e: React.MouseEvent) => menu(e, a),
    "data-tip": `${a.path} · ${a.kind} · ${bytes(a.bytes)}${placeable(a) ? " — drag into the viewport or hierarchy" : ""}`,
  });

  const counts = useMemo(() => {
    const c: Record<string, number> = { all: assets.length };
    for (const a of assets) c[a.kind] = (c[a.kind] ?? 0) + 1;
    return c;
  }, [assets]);

  const toolbar = (
    <>
    <Toolbar>
      <SearchInput value={q} onChange={setQ} placeholder="Search assets" />
      <IconButton icon={LayoutGrid} label="Grid" active={view === "grid"} onClick={() => (setView("grid"), localStorage.setItem("amoris.editor.assets.view", "grid"))} />
      <IconButton icon={List} label="List" active={view === "list"} onClick={() => (setView("list"), localStorage.setItem("amoris.editor.assets.view", "list"))} />
      <ToolbarSeparator />
      <IconButton icon={RefreshCw} label="Refresh" onClick={() => void refreshAssets()} />
      <Button size="sm" icon={Import} onClick={importAsset} data-tip="Upload files into the project's assets (or drop them on this panel)">
        Import
      </Button>
    </Toolbar>
    <div className="kind-chips">
      {KINDS.map((k) => (
        <button key={k.id} type="button" className={cx("kind-chip", kind === k.id && "is-on")} onClick={() => setKind(k.id)}>
          {k.label}
          {counts[k.id] ? <span className="kind-count">{counts[k.id]}</span> : null}
        </button>
      ))}
    </div>
    </>
  );

  // Files dropped from the system (not an asset card dragged out of this panel) are imported.
  const fromSystem = (e: React.DragEvent) => e.dataTransfer.types.includes("Files");
  return (
    <PanelShell
      toolbar={toolbar}
      className={cx("assets-panel", dropping && "is-dropping")}
      onDragOver={(e: React.DragEvent) => {
        if (!fromSystem(e)) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = "copy";
        setDropping(true);
      }}
      onDragLeave={(e: React.DragEvent) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDropping(false);
      }}
      onDrop={(e: React.DragEvent) => {
        setDropping(false);
        if (!fromSystem(e)) return;
        e.preventDefault();
        void importFiles([...e.dataTransfer.files]);
      }}
      footer={
        <div className="panel-footer">
          {shown.length} of {assets.length} assets · {bytes(total)}
        </div>
      }
    >
      {shown.length === 0 ? (
        <EmptyState icon={PackagePlus} title={assets.length ? "No asset matches" : "No assets"}>
          {assets.length ? "Clear the search or pick another kind." : "Import a glTF model, a texture or a splat, or drop files here."}
        </EmptyState>
      ) : view === "grid" ? (
        <div className="asset-grid">
          {shown.map((a) => {
            const { key, ...props } = card(a);
            return (
              <div key={key} className={cx("asset-card", selected === a.path && "is-selected")} {...props}>
                <div className="asset-thumb">
                  <Thumb a={a} big />
                  <span className={`asset-kind kind-${a.kind}`}>{a.kind}</span>
                </div>
                <div className="asset-name">{a.path.split("/").pop()}</div>
                <div className="asset-meta">{bytes(a.bytes)}</div>
              </div>
            );
          })}
        </div>
      ) : (
        <div className="asset-list">
          {shown.map((a) => {
            const { key, ...props } = card(a);
            return (
              <div key={key} className={cx("asset-row", selected === a.path && "is-selected")} {...props}>
                <span className="asset-row-thumb">
                  <Thumb a={a} big={false} />
                </span>
                <span className="asset-row-name">{a.path}</span>
                <span className={`asset-kind kind-${a.kind}`}>{a.kind}</span>
                <span className="asset-meta mono">{bytes(a.bytes)}</span>
              </div>
            );
          })}
        </div>
      )}
    </PanelShell>
  );
}
