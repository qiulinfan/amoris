import { FolderOpen, Search } from "lucide-react";
import amorisIcon from "../../../assets/branding/amoris-icon-morandi.svg";
import { formatKeys } from "../commands/keys";
import { useConnection } from "../state/connection";
import { useUi } from "../state/ui";
import { cx } from "../ui/cx";
import { MenuBar } from "./MenuBar";
import { Transport } from "./Transport";

function Logo() {
  return <img src={amorisIcon} width="22" height="22" className="logo" alt="" />;
}

export function TopBar() {
  const project = useConnection((s) => s.project);
  const info = useConnection((s) => s.info);
  return (
    <header className="topbar">
      <div className="topbar-left">
        <Logo />
        <span className="brand">Amoris</span>
        <span className="project" data-tip={project ? `${project.root}` : "No project"}>
          {project?.name ?? "—"}
        </span>
        {window.amorisDesktop && <button type="button" className="btn btn-ghost" aria-label="Open Project" data-tip="Open Project (Cmd/Ctrl+O)" onClick={() => void window.amorisDesktop!.openProject()}><FolderOpen size={15} /></button>}
        <MenuBar />
      </div>
      <Transport />
      <div className="topbar-right">
        <button type="button" className="palette-btn" onClick={() => useUi.getState().set({ paletteOpen: true, paletteQuery: "" })}>
          <Search size={13} />
          <span>Search commands, entities, host methods</span>
          <kbd className="kbd">{formatKeys("Mod+K")}</kbd>
        </button>
        <span className={cx("conn-pill", `is-${info.state}`)} data-tip={info.lastError ?? "Connected"}>
          <span className="conn-dot" />
          {info.state === "open" ? "Connected" : info.state === "connecting" ? "Connecting" : "Offline"}
        </span>
      </div>
    </header>
  );
}
