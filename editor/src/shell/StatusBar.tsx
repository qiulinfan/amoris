import { Bot, Bug, CircleAlert, Cpu, Hash, MousePointer2, TriangleAlert } from "lucide-react";
import { useEffect, useState } from "react";
import { host } from "../host/api";
import { openPanel } from "../layout/dock";
import { useAgent } from "../state/agent";
import { useConnection } from "../state/connection";
import { useDebug } from "../state/debug";
import { useLogs } from "../state/logs";
import { useSelection } from "../state/selection";
import { useSession } from "../state/session";
import { cx } from "../ui/cx";

function Retry() {
  const info = useConnection((s) => s.info);
  const [, force] = useState(0);
  useEffect(() => {
    if (info.state !== "closed") return;
    const t = setInterval(() => force((n) => n + 1), 500);
    return () => clearInterval(t);
  }, [info.state]);
  if (info.state !== "closed" || !info.retryAt) return null;
  const s = Math.max(0, Math.ceil((info.retryAt - Date.now()) / 1000));
  return (
    <button type="button" className="sb-item link" onClick={() => host.retryNow()}>
      retry in {s}s · retry now
    </button>
  );
}

export function StatusBar() {
  const info = useConnection((s) => s.info);
  const project = useConnection((s) => s.project);
  const viewport = useConnection((s) => s.viewport);
  const status = useSession((s) => s.status);
  const selected = useSelection((s) => s.ids.length);
  const debug = useDebug((s) => s.state);
  const errors = useLogs((s) => s.lines.reduce((n, l) => n + (l.level === "error" ? 1 : 0), 0));
  const warnings = useLogs((s) => s.lines.reduce((n, l) => n + (l.level === "warn" ? 1 : 0), 0));
  const agentLive = useAgent((s) => {
    const last = s.feed.at(-1);
    return last ? Date.now() - last.ts < 15000 : false;
  });
  return (
    <footer className={cx("statusbar", `conn-${info.state}`, debug.state === "paused" && "is-paused")}>
      <span className="sb-item sb-conn">
        <span className="conn-dot" />
        {info.state === "open" ? `${host.endpoints.label}` : info.state === "connecting" ? `Connecting to ${host.endpoints.label}…` : `Offline: ${info.lastError ?? host.endpoints.label}`}
        {project?.host === "mock" && <span className="sb-tag">mock host</span>}
      </span>
      <Retry />
      {project && <span className="sb-item">{project.name}</span>}
      <button type="button" className="sb-item link" onClick={() => openPanel("console")} data-tip="Errors and warnings in the console">
        <CircleAlert size={12} /> {errors} <TriangleAlert size={12} /> {warnings}
      </button>
      {debug.state === "paused" && (
        <button type="button" className="sb-item sb-paused" onClick={() => openPanel("debug")}>
          <Bug size={12} /> Paused {debug.location ? `at ${debug.location.file.split("/").pop()}:${debug.location.line}` : ""}
        </button>
      )}
      <span className="sb-spacer" />
      {agentLive && (
        <button type="button" className="sb-item sb-agent" onClick={() => openPanel("agent")}>
          <Bot size={12} /> agent active
        </button>
      )}
      {selected > 0 && (
        <span className="sb-item">
          <MousePointer2 size={12} /> {selected} selected
        </span>
      )}
      {status && (
        <>
          <span className="sb-item">{status.entities} entities</span>
          <span className="sb-item mono" data-tip="World hash (canonical encoding)">
            <Hash size={12} /> {status.world_hash.slice(0, 8)}
          </span>
        </>
      )}
      <span className="sb-item" data-tip="Viewport renderer">
        <Cpu size={12} /> {viewport === "wasm" ? "WebGPU (wasm)" : viewport === "fallback" ? "Fallback view" : "…"}
      </span>
    </footer>
  );
}
