// Play / Pause / Step / Stop with the host's clock: tick, time, frame rate and tick cost.

import { ChevronDown, Pause, Play, Square, StepForward } from "lucide-react";
import { useState } from "react";
import { setSpeed, stepTick, togglePause, togglePlay } from "../actions/time";
import { useConnection } from "../state/connection";
import { useSession } from "../state/session";
import { IconButton } from "../ui/Button";
import { PopupMenu } from "../ui/Menu";
import { clock } from "../ui/format";
import { cx } from "../ui/cx";

export function Transport() {
  const status = useSession((s) => s.status);
  const online = useConnection((s) => s.info.state === "open");
  const [speedMenu, setSpeedMenu] = useState<{ x: number; y: number } | null>(null);
  const playing = status?.mode === "play";
  const paused = !!status?.paused;
  const speed = status?.speed ?? 1;
  return (
    <div className="transport">
      <div className={cx("transport-buttons", playing && "is-playing")}>
        <IconButton
          icon={playing ? Square : Play}
          label={playing ? "Stop (discard the Play world)" : "Play (fork the edit world)"}
          keys="Mod+P"
          size="lg"
          tone="play"
          active={playing}
          disabled={!online}
          onClick={() => void togglePlay()}
        />
        <IconButton icon={Pause} label={paused && playing ? "Resume" : "Pause"} keys="Mod+Alt+P" size="lg" active={playing && paused} disabled={!online} onClick={() => void togglePause()} />
        <IconButton icon={StepForward} label="Step one tick" keys="Mod+Alt+." size="lg" disabled={!online} onClick={() => void stepTick()} />
      </div>
      <div className="transport-readout" data-tip="Tick · simulated time · frame rate · cost of the last tick">
        <span className="ro">
          <span className="ro-label">tick</span>
          <span className="ro-value">{status?.tick ?? "–"}</span>
        </span>
        <span className="ro">
          <span className="ro-value">{clock(status?.t_s ?? 0)}</span>
        </span>
        <span className="ro">
          <span className="ro-value">{status ? status.fps.toFixed(0) : "–"}</span>
          <span className="ro-label">fps</span>
        </span>
        <span className="ro">
          <span className="ro-value">{status ? status.tick_ms.toFixed(2) : "–"}</span>
          <span className="ro-label">ms</span>
        </span>
      </div>
      <button
        type="button"
        className="speed-btn"
        data-tip="Simulation speed"
        disabled={!online}
        onClick={(e) => {
          const r = e.currentTarget.getBoundingClientRect();
          setSpeedMenu({ x: r.left, y: r.bottom + 4 });
        }}
      >
        {speed}× <ChevronDown size={11} />
      </button>
      {speedMenu && (
        <PopupMenu
          x={speedMenu.x}
          y={speedMenu.y}
          onClose={() => setSpeedMenu(null)}
          items={[0.25, 0.5, 1, 2, 4].map((s) => ({ label: `${s}×`, checked: s === speed, run: () => void setSpeed(s) }))}
        />
      )}
      <span className={cx("mode-badge", playing ? "is-play" : "is-edit")} data-tip={playing ? "Play: a fork of the edit world; Stop discards it" : "Edit: changes go to the edit world and its undo history"}>
        <span className="mode-dot" />
        {playing ? (paused ? "PAUSED" : "PLAY") : "EDIT"}
      </span>
    </div>
  );
}
