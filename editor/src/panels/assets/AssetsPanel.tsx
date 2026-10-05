// The asset browser: the project's assets as a grid of thumbnails (or a list), filtered by kind and
// name; drag one into the viewport or the hierarchy to place it, import a file by path.

import { ClipboardCopy, Import, LayoutGrid, List, PackagePlus, RefreshCw } from "lucide-react";
import { useMemo, useState } from "react";
import { api } from "../../host/api";
import { attempt } from "../../actions/report";
import { spawnAsset } from "../../actions/world";
import { ASSET_MIME, useAssets } from "../../state/assets";
import { useUi, promptText } from "../../state/ui";
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

export async function importAsset() {
  const path = await promptText({ title: "Import asset", label: "File to import (a path the host can read)", value: "", placeholder: "~/Downloads/lighthouse.glb", confirm: "Import" });
  if (!path) return;
  const r = await attempt(api.assets.import(path), "Import");
  if (r) {
    useUi.getState().toast({ kind: "success", title: "Imported", body: `${r.asset} (${r.meshes} meshes, ${r.materials} materials)` });
    await refreshAssets();
  }
}

async function refreshAssets() {
  const list = await attempt(api.assets.list(), "assets.list");
  if (list) useAssets.getState().set(list);
}

const placeable = (a: AssetInfo) => a.kind === "model" || a.kind === "splat";

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
  const [view, setView] = useState<"grid" | "list">(() => (localStorage.getItem("aipocket2.editor.assets.view") as "grid" | "list") ?? "grid");
  const [selected, setSelected] = useState<string | null>(null);
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
      { label: "Reimport", icon: <RefreshCw size={14} />, run: () => void attempt(api.assets.import(a.path), "Reimport") },
    ]);
  };

  const card = (a: AssetInfo) => ({
    key: a.path,
    draggable: true,
    onDragStart: (e: React.DragEvent) => {
      e.dataTransfer.setData(ASSET_MIME, a.path);
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
      <IconButton icon={LayoutGrid} label="Grid" active={view === "grid"} onClick={() => (setView("grid"), localStorage.setItem("aipocket2.editor.assets.view", "grid"))} />
      <IconButton icon={List} label="List" active={view === "list"} onClick={() => (setView("list"), localStorage.setItem("aipocket2.editor.assets.view", "list"))} />
      <ToolbarSeparator />
      <IconButton icon={RefreshCw} label="Refresh" onClick={() => void refreshAssets()} />
      <Button size="sm" icon={Import} onClick={() => void importAsset()} data-tip="Import a file into the project's assets">
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

  return (
    <PanelShell
      toolbar={toolbar}
      className="assets-panel"
      footer={
        <div className="panel-footer">
          {shown.length} of {assets.length} assets · {bytes(total)}
        </div>
      }
    >
      {shown.length === 0 ? (
        <EmptyState icon={PackagePlus} title={assets.length ? "No asset matches" : "No assets"}>
          {assets.length ? "Clear the search or pick another kind." : "Import a glTF model, a texture or a splat."}
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
