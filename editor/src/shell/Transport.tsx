// Play / Pause / Step / Stop with the host's clock: tick, time, frame rate and tick cost.

import { ChevronDown, Pause, Play, Square, StepForward } from "lucide-react";
import { useState } from "react";
import { setSpeed, stepTick, togglePause, togglePlay } from "../actions/time";
import { haltedOf, speedOf } from "../host/api";
import { useConnection } from "../state/connection";
import { useProfile } from "../state/profile";
import { useSession } from "../state/session";
import { IconButton } from "../ui/Button";
import { PopupMenu } from "../ui/Menu";
import { clock } from "../ui/format";
import { cx } from "../ui/cx";

export function Transport() {
  const status = useSession((s) => s.status);
  // The cost of a tick: the status's own, else the host profile's last quarter second.
  const profiled = useProfile((s) => s.samples.at(-1));
  const tickMs = status?.tick_ms ?? (profiled && profiled.ticks > 0 ? profiled.tick_ms : null);
  // The host's simulation rate: `tps` (older hosts sent it only as `fps`). The viewport's own
  // frame rate is the stats overlay's.
  const tps = status?.tps ?? status?.fps;
  const online = useConnection((s) => s.info.state === "open");
  const [speedMenu, setSpeedMenu] = useState<{ x: number; y: number } | null>(null);
  const playing = status?.mode === "play";
  const paused = !!status?.paused;
  // A poisoned world: the host runs no tick of it, so Pause, Resume and Step have nothing to do;
  // Stop (in Play) or a restore is the way out.
  const halted = haltedOf(status);
  const haltedTip = playing ? "Halted: a tick stopped halfway, so this Play world runs no further tick. Stop returns to the edit world" : "Halted: a tick stopped halfway, so this world runs no further tick. Restore a snapshot to run it again";
  // The real-time speed of the world shown (each world has its own); null while it is stepped.
  const speed = speedOf(status);
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
        <IconButton icon={Pause} label={paused && playing ? "Resume" : "Pause"} keys="Mod+Alt+P" size="lg" active={playing && paused} disabled={!online || halted} onClick={() => void togglePause()} />
        <IconButton icon={StepForward} label="Step one tick" keys="Mod+Alt+." size="lg" disabled={!online || halted} onClick={() => void stepTick()} />
      </div>
      <div className="transport-readout" data-tip="Tick · simulated time · ticks per second · cost of a tick">
        <span className="ro">
          <span className="ro-label">tick</span>
          <span className="ro-value">{status?.tick ?? "–"}</span>
        </span>
        <span className="ro">
          <span className="ro-value">{clock(status?.t_s ?? 0)}</span>
        </span>
        <span className="ro">
          <span className="ro-value">{tps != null ? tps.toFixed(0) : "–"}</span>
          <span className="ro-label">tps</span>
        </span>
        <span className="ro">
          <span className="ro-value">{tickMs != null ? tickMs.toFixed(2) : "–"}</span>
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
        {speed === null ? "stepped" : `${speed}×`} <ChevronDown size={11} />
      </button>
      {speedMenu && (
        <PopupMenu
          x={speedMenu.x}
          y={speedMenu.y}
          onClose={() => setSpeedMenu(null)}
          items={[0.25, 0.5, 1, 2, 4].map((s) => ({ label: `${s}×`, checked: s === speed, run: () => void setSpeed(s) }))}
        />
      )}
      <span
        className={cx("mode-badge", halted ? "is-halted" : playing ? "is-play" : "is-edit")}
        data-tip={halted ? haltedTip : playing ? "Play: a fork of the edit world; Stop discards it" : "Edit: changes go to the edit world and its undo history"}
      >
        <span className="mode-dot" />
        {halted ? "HALTED" : playing ? (paused ? "PAUSED" : "PLAY") : "EDIT"}
      </span>
    </div>
  );
}
