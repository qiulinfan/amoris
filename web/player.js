// The game player (docs/spec/web-export.md): a first-person game's `player` seat in a page, as the
// native window plays it (`amoris play <project> --walk`, crates/amoris-app/src/window.rs). The game
// runs in a module Web Worker (amoris.js, game.js) at real time; the engine's renderer
// (viewport/viewport.js) draws its render feed with WebGPU; the keys, the mouse and the wheel become
// `player.act` controls and pulses of the seat, sent as the local human player. The camera is the
// walker's, turned at once by the page's own look (the viewport's `first_person`).
//
// `cargo xtask export-web <project>` copies this page as index.html beside the module, the viewport
// and the project's web package in game/; `?package=URL` loads another package.
import { loadModule, start } from "./amoris.js";
import { createViewport } from "./viewport/viewport.js";

const $ = (id) => document.getElementById(id);
const PLAYER = { source: "player" };

function progress(share) {
  $("progress").style.width = `${Math.round(Math.min(1, share) * 100)}%`;
}

/** Fetches a file, reporting its bytes as they come. */
async function fetchBytes(url, onBytes) {
  const r = await fetch(url);
  if (!r.ok) throw new Error(`${url}: ${r.status} ${r.statusText}`);
  if (!r.body) return new Uint8Array(await r.arrayBuffer());
  const reader = r.body.getReader();
  const parts = [];
  let n = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    parts.push(value);
    n += value.length;
    onBytes(n);
  }
  const out = new Uint8Array(n);
  let at = 0;
  for (const p of parts) { out.set(p, at); at += p.length; }
  return out;
}

const NO_WEBGPU = `This game is drawn with WebGPU, which this browser does not offer.
Play it in a current Chrome or Edge on Windows, macOS or ChromeOS (Chrome on Android 12+ too, without
mouse controls). Safari 26 and Firefox 141+ on Windows have WebGPU as well; on Linux, Chrome needs
chrome://flags/#enable-unsafe-webgpu.`;

/** Keys, buttons and the wheel since the last frame, as the native window gathers them. */
class Input {
  constructor(canvas) {
    this.canvas = canvas;
    this.keys = new Set();
    this.forced = false; // the check page's capture without pointer lock
    this.dx = 0;
    this.dy = 0;
    this.jump = false;
    this.primary = false;
    this.secondary = false;
    this.slot = null;
    this.wheel = 0;
    this.slotSteps = 0;
    addEventListener("keydown", (e) => {
      if (!this.captured()) return;
      this.keys.add(e.code);
      if (e.code === "Space" && !e.repeat) this.jump = true;
      const digit = /^Digit([1-9])$/.exec(e.code);
      if (digit) this.slot = Number(digit[1]) - 1;
      if (e.code !== "Escape" && !e.metaKey && !e.ctrlKey) e.preventDefault();
    });
    addEventListener("keyup", (e) => this.keys.delete(e.code));
    addEventListener("blur", () => this.keys.clear());
    document.addEventListener("pointerlockchange", () => { if (!this.captured()) this.keys.clear(); });
    canvas.addEventListener("mousedown", (e) => {
      if (!this.captured()) return;
      if (e.button === 0) this.primary = true;
      else if (e.button === 2) this.secondary = true;
      e.preventDefault();
    });
    addEventListener("contextmenu", (e) => e.preventDefault());
    addEventListener("mousemove", (e) => {
      if (!this.captured()) return;
      this.dx += e.movementX || 0;
      this.dy += e.movementY || 0;
    });
    addEventListener("wheel", (e) => {
      if (!this.captured()) return;
      // Down moves the hotbar right, a notch a slot (Chrome's notch is 100 pixels).
      this.wheel += e.deltaMode === 1 ? e.deltaY : e.deltaY / 100;
      while (this.wheel >= 1) { this.wheel -= 1; this.slotSteps += 1; }
      while (this.wheel <= -1) { this.wheel += 1; this.slotSteps -= 1; }
      e.preventDefault();
    }, { passive: false });
  }

  captured() {
    return this.forced || document.pointerLockElement === this.canvas;
  }

  capture() {
    const lock = (options) => {
      try {
        return Promise.resolve(this.canvas.requestPointerLock(options));
      } catch (e) {
        return Promise.reject(e);
      }
    };
    // Raw mouse movement where the browser has it (as the native window reads the device), else
    // the plain lock. Chrome refuses a lock for about a second after Escape released one: the
    // next click tries again.
    lock({ unadjustedMovement: true })
      .catch(() => lock())
      .catch((e) => console.warn("pointer lock refused:", e && e.message));
  }

  key(code) { return this.keys.has(code); }

  /** This frame's input; edges and movement are taken. */
  take() {
    const captured = this.captured();
    const x = Number(this.key("KeyD")) - Number(this.key("KeyA"));
    const z = Number(this.key("KeyW")) - Number(this.key("KeyS"));
    const length = Math.max(1, Math.hypot(x, z));
    const input = {
      captured,
      motion: captured ? [x / length, z / length] : [0, 0],
      fast: captured && (this.key("ShiftLeft") || this.key("ShiftRight")),
      rise: captured && this.key("Space"),
      jump: this.jump, primary: this.primary, secondary: this.secondary,
      slot: this.slot, slotSteps: this.slotSteps, dx: this.dx, dy: this.dy,
    };
    this.jump = this.primary = this.secondary = false;
    this.slot = null;
    this.slotSteps = 0;
    this.dx = this.dy = 0;
    return input;
  }
}

/** The seat: each frame's input as `player.act` actions, sent when something changed. */
class Seat {
  constructor(amoris) {
    this.amoris = amoris;
    this.sent = null;
    this.slot = 0;
    this.acts = 0;
    this.refused = 0;
  }

  frame(input, look) {
    const [yaw, pitch] = look;
    const actions = [];
    const now = [input.motion[1], input.motion[0], input.fast, yaw, pitch, input.rise];
    if (!this.sent || now.some((v, i) => v !== this.sent[i])) {
      actions.push({ do: "set", controls: { forward: now[0], strafe: now[1], run: now[2], yaw, pitch, rise: now[5] } });
      this.sent = now;
    }
    let slot = this.slot;
    if (input.slot !== null) slot = input.slot;
    else if (input.slotSteps) slot = (((this.slot + input.slotSteps) % 9) + 9) % 9;
    if (slot !== this.slot) {
      this.slot = slot;
      actions.push({ do: "set", controls: { slot: String(slot) } });
    }
    if (input.captured) {
      for (const [on, control] of [[input.jump, "jump"], [input.primary, "primary"], [input.secondary, "secondary"]]) {
        if (on) actions.push({ do: "pulse", control });
      }
    }
    if (!actions.length) return;
    this.acts++;
    this.amoris.command("player.act", { actions }, PLAYER).catch((e) => {
      this.refused++;
      console.warn("player.act refused:", e && e.message);
    });
  }
}

export async function main() {
  const params = new URLSearchParams(location.search);
  const packageUrl = new URL(params.get("package") || "game/package.json", location.href);
  if (!navigator.gpu) throw new Error(NO_WEBGPU);
  const t0 = performance.now();
  // The export writes the package's size on the page, for the bar.
  const total = Number(document.documentElement.dataset.packageBytes || 0);
  const [{ module }, pkg] = await Promise.all([
    loadModule(),
    fetchBytes(packageUrl, (n) => { if (total) progress(n / total); }),
  ]);
  const { name } = JSON.parse(new TextDecoder().decode(pkg));
  document.title = name;
  $("name").textContent = name;
  $("status").textContent = "Starting…";
  const amoris = start(module, pkg, { pacing: { real_time: { speed: 1 } } });
  const canvas = $("view");
  const input = new Input(canvas);
  const seat = new Seat(amoris);
  const state = (window.amorisPlayer = {
    loaded_ms: 0, ready_ms: 0, frames: 0, look: null, stats: null, samples: [],
    amoris, input, seat,
    /** Captures without pointer lock (the check page, where headless Chrome has no lock). */
    forceCapture(on = true) { input.forced = on; veil(); },
  });
  let ready = false;
  const veil = () => {
    $("veil").hidden = ready && input.captured();
    if (ready) $("status").textContent = "Click to play";
  };
  document.addEventListener("pointerlockchange", veil);
  $("veil").addEventListener("click", () => { if (ready) input.capture(); });
  canvas.addEventListener("click", () => { if (ready && !input.captured()) input.capture(); });
  const vp = await createViewport(canvas, {
    assetsUrl: new URL(".", packageUrl).href,
    gpuMinimal: params.get("gpu_minimal") || undefined,
    beforeFrame(api) {
      const frame = input.take();
      const look = api.raw.first_person(frame.dx, frame.dy, frame.captured);
      state.look = look.length ? Array.from(look) : null;
      if (!state.look) return;
      seat.frame(frame, state.look);
    },
  });
  state.viewport = vp;
  amoris.onRender((bytes) => vp.pushFrame(bytes));
  vp.onStats((s) => {
    state.frames++;
    state.stats = s;
    state.samples.push(s.frame_ms);
    if (state.samples.length > 600) state.samples.shift();
    // Ready once the walker's camera is known and the first models and textures are in.
    if (!ready && state.look && s.pending_assets === 0 && s.instances > 0) {
      ready = true;
      state.ready_ms = performance.now() - t0;
      progress(1);
      veil();
    }
  });
  state.loaded_ms = performance.now() - t0;
  await amoris.ready;
  const stopped = () => {
    if (!amoris.stopped) return requestAnimationFrame(stopped);
    $("veil").hidden = false;
    $("status").textContent = "The game stopped.";
    $("error").hidden = false;
    $("error").textContent = amoris.stopped.message;
  };
  requestAnimationFrame(stopped);
}
