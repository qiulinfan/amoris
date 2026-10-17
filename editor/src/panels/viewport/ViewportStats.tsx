import { Navigation2 } from "lucide-react";
import { useConnection } from "../../state/connection";
import { useRender } from "../../state/render";
import { useSession } from "../../state/session";
import { useViewport } from "../../state/viewport";
import { useWorld } from "../../state/world";
import { ms } from "../../ui/format";

export function ViewportStats() {
  const status = useSession((s) => s.status);
  const render = useRender((s) => s.stats);
  const kind = useConnection((s) => s.viewport);
  const show = useViewport((s) => s.stats);
  const wind = useWorld((s) => {
    for (const id of s.order) {
      const w = s.entities[id]?.components.Wind as { from_deg?: number; speed?: number } | undefined;
      if (w) return `${w.from_deg ?? 0}|${w.speed ?? 0}`;
    }
    return null;
  });
  const [from, speed] = wind ? wind.split("|").map(Number) : [0, 0];
  return (
    <>
      {show && (
        <div className="vp-stats">
          <span className={`vp-renderer ${kind === "wasm" ? "is-wasm" : ""}`} data-tip={kind === "wasm" ? "The engine's renderer (wasm, WebGPU)" : "Fallback view (Canvas 2D) until the host serves /wasm/amoris_web.js"}>
            {kind === "wasm" ? "WebGPU" : kind === "fallback" ? "Fallback" : "…"}
          </span>
          {render && (
            <span
              className="vp-frame"
              data-tip={`The viewport: ${render.frames} frames in the last quarter second, ${render.frame_ms != null ? ms(render.frame_ms) : "–"} apart on average (longest ${render.frame_max_ms != null ? ms(render.frame_max_ms) : "–"}), ${render.render_ms != null ? ms(render.render_ms) : "–"} in the draw call${render.gpu_ms != null ? `, ${ms(render.gpu_ms)} on the GPU` : ""}`}
            >
              {render.fps != null ? render.fps.toFixed(0) : "–"} fps
            </span>
          )}
          {render?.gpu_ms != null && <span>GPU {ms(render.gpu_ms)}</span>}
          {status && (
            <>
              <span data-tip="Simulation ticks per second">{status.tps != null ? status.tps.toFixed(0) : "–"} tps</span>
              <span>tick {status.tick}</span>
              <span>{status.entities} entities</span>
            </>
          )}
        </div>
      )}
      {wind && (
        <div className="vp-wind" data-tip={`Wind from ${from}° at ${speed} m/s`}>
          <Navigation2 size={14} style={{ transform: `rotate(${(from ?? 0) + 180}deg)` }} />
          <span>{speed} m/s</span>
        </div>
      )}
    </>
  );
}
