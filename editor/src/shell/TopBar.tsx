import { Search } from "lucide-react";
import { formatKeys } from "../commands/keys";
import { useConnection } from "../state/connection";
import { useUi } from "../state/ui";
import { cx } from "../ui/cx";
import { MenuBar } from "./MenuBar";
import { Transport } from "./Transport";

function Logo() {
  return (
    <svg width="18" height="18" viewBox="0 0 24 24" className="logo" aria-hidden>
      <defs>
        <linearGradient id="lg" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#7aa8ff" />
          <stop offset="1" stopColor="#4f6dff" />
        </linearGradient>
      </defs>
      <path d="M12 2 21 7v10l-9 5-9-5V7z" fill="url(#lg)" />
      <path d="M12 2v20M3 7l9 5 9-5" stroke="#0d0f13" strokeOpacity=".35" strokeWidth="1.2" fill="none" />
    </svg>
  );
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
