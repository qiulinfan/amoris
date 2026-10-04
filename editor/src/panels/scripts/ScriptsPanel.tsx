// The Scripts panel: the project's scripts with their problems on the left, the code editor (Monaco,
// loaded on first show) on the right, and Save/Apply in the tool bar.

import { FileCode, Hammer, Save } from "lucide-react";
import { lazy, Suspense } from "react";
import { applyScripts, saveActiveScript } from "../../actions/scripts";
import { useScripts } from "../../state/scripts";
import { Button } from "../../ui/Button";
import { cx } from "../../ui/cx";
import { formatKeys } from "../../commands/keys";
import { bytes } from "../../ui/format";
import { PanelShell } from "../../ui/Panel";
import { SplitPane } from "../../ui/SplitPane";
import { Spacer, Toolbar } from "../../ui/Toolbar";
import "./scripts.css";

const CodeEditor = lazy(() => import("./CodeEditor"));

function FileList() {
  const { files, active, dirty, diagnostics } = useScripts();
  return (
    <div className="file-list">
      <div className="file-list-head">scripts/</div>
      {files.map((f) => {
        const diags = diagnostics[f.path] ?? f.diagnostics ?? [];
        const errors = diags.filter((d) => d.severity === "error").length;
        const warnings = diags.filter((d) => d.severity === "warning").length;
        return (
          <div
            key={f.path}
            className={cx("file-row", f.path === active && "is-active")}
            onClick={() => useScripts.getState().openFile(f.path)}
            data-tip={`${f.path} · ${bytes(f.bytes)}`}
          >
            <FileCode size={14} className="file-icon" />
            <span className="file-name">{f.path.replace(/^scripts\//, "")}</span>
            {dirty[f.path] && <span className="dirty-dot" />}
            {errors > 0 && <span className="badge badge-error">{errors}</span>}
            {errors === 0 && warnings > 0 && <span className="badge badge-warn">{warnings}</span>}
          </div>
        );
      })}
    </div>
  );
}

export function ScriptsPanel() {
  const active = useScripts((s) => s.active);
  const dirty = useScripts((s) => (s.active ? s.dirty[s.active] : false));
  const toolbar = (
    <Toolbar>
      <Button size="sm" variant="ghost" icon={Save} disabled={!active} onClick={() => saveActiveScript()} data-tip={`Save and apply  ${formatKeys("Mod+S")}`}>
        Save{dirty ? " •" : ""}
      </Button>
      <Button size="sm" variant="ghost" icon={Hammer} onClick={() => void applyScripts()} data-tip="Type check, compile and hot swap all scripts">
        Apply
      </Button>
      <Spacer />
      <span className="toolbar-note">F9 breakpoint · click the gutter</span>
    </Toolbar>
  );
  return (
    <PanelShell toolbar={toolbar} scroll={false} className="scripts-panel">
      <SplitPane id="scripts" initial={180} min={120}>
        <FileList />
        <Suspense fallback={<div className="code-loading">Loading the code editor…</div>}>
          <CodeEditor />
        </Suspense>
      </SplitPane>
    </PanelShell>
  );
}
