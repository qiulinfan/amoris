// Frame timing on the page, for a renderer's `stats()` (engine.ts `RenderStats`): the interval
// between animation frames and the time spent drawing, summed since the last read.
// web/viewport/amoris_web.js keeps the same counters for the wasm renderer.

import type { RenderStats } from "./engine";

export class FrameMeter {
  private last = 0;
  private frames = 0;
  private sum = 0;
  private max = 0;
  private draws = 0;
  private drawn = 0;

  /** One animation frame at `now` (its timestamp) that spent `drawMs` drawing. */
  frame(now: number, drawMs: number) {
    if (this.last > 0 && now > this.last) {
      const dt = now - this.last;
      this.frames++;
      this.sum += dt;
      this.max = Math.max(this.max, dt);
    }
    this.last = now;
    this.draws++;
    this.drawn += drawMs;
  }

  /** The frames since the last read; the counters start again. */
  read(): RenderStats {
    const n = this.frames;
    const out: RenderStats = {
      frames: n,
      fps: n ? 1000 / (this.sum / n) : null,
      frame_ms: n ? this.sum / n : null,
      frame_max_ms: n ? this.max : null,
      render_ms: this.draws ? this.drawn / this.draws : null,
    };
    this.frames = this.sum = this.max = this.draws = this.drawn = 0;
    return out;
  }
}
