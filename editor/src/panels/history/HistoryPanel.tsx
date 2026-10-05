// The undo history the host keeps: every edit, the editor's and agents' alike (agents' labels start
// with "agent:"). Click an entry to undo or redo up to it.

import { Bot, CircleDot, Pencil, Redo2, Undo2 } from "lucide-react";
import { useState } from "react";
import { api } from "../../host/api";
import { reportError } from "../../actions/report";
import { redo, undo } from "../../actions/world";
import { isAgentLabel, useHistory } from "../../state/history";
import { IconButton } from "../../ui/Button";
import { cx } from "../../ui/cx";
import { PanelShell } from "../../ui/Panel";
import { Spacer, Toolbar } from "../../ui/Toolbar";
import "./history.css";

export function HistoryPanel() {
  const { undo: undos, redo: redos } = useHistory();
  const [busy, setBusy] = useState(false);

  const travel = async (undoCount: number, redoCount: number) => {
    if (busy) return;
    setBusy(true);
    try {
      for (let i = 0; i < undoCount; i++) await api.history.undo();
      for (let i = 0; i < redoCount; i++) await api.history.redo();
    } catch (e) {
      reportError(e, "History");
    } finally {
      setBusy(false);
    }
  };

  const Entry = ({ label, state, onClick }: { label: string; state: "done" | "current" | "undone"; onClick(): void }) => {
    const agent = isAgentLabel(label);
    const Icon = agent ? Bot : Pencil;
    return (
      <div className={cx("hist-row", `is-${state}`, agent && "is-agent")} onClick={onClick}>
        <span className="hist-rail" />
        <Icon size={13} className="hist-icon" />
        <span className="hist-label">{agent ? label.replace(/^agent:\s*/i, "") : label}</span>
        {agent && <span className="hist-badge">agent</span>}
      </div>
    );
  };

  const toolbar = (
    <Toolbar>
      <IconButton icon={Undo2} label="Undo" keys="Mod+Z" disabled={!undos.length} onClick={() => void undo()} />
      <IconButton icon={Redo2} label="Redo" keys="Mod+Shift+Z" disabled={!redos.length} onClick={() => void redo()} />
      <Spacer />
      <span className="toolbar-note">
        {undos.length} edits{redos.length ? ` · ${redos.length} undone` : ""}
      </span>
    </Toolbar>
  );

  return (
    <PanelShell toolbar={toolbar} className={cx("history-panel", busy && "is-busy")}>
      <div className={cx("hist-row", "is-root", undos.length === 0 && "is-current")} onClick={() => void travel(undos.length, 0)}>
        <span className="hist-rail" />
        <CircleDot size={13} className="hist-icon" />
        <span className="hist-label">Opened scene</span>
      </div>
      {undos.map((label, i) => (
        <Entry key={`u${i}`} label={label} state={i === undos.length - 1 ? "current" : "done"} onClick={() => void travel(undos.length - 1 - i, 0)} />
      ))}
      {redos.map((label, j) => (
        <Entry key={`r${j}`} label={label} state="undone" onClick={() => void travel(0, j + 1)} />
      ))}
    </PanelShell>
  );
}
