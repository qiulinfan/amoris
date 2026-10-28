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
    this.unlocked = false;
    this.dragged = false;
    this.onChange = () => {};
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
      if (e.code === "Escape" && this.unlocked) {
        this.release();
        e.preventDefault();
        return;
      }
      if (!this.captured()) return;
      this.keys.add(e.code);
      if (e.code === "Space" && !e.repeat) this.jump = true;
      const digit = /^Digit([1-9])$/.exec(e.code);
      if (digit) this.slot = Number(digit[1]) - 1;
      if (e.code !== "Escape" && !e.metaKey && !e.ctrlKey) e.preventDefault();
    });
    addEventListener("keyup", (e) => this.keys.delete(e.code));
    addEventListener("blur", () => {
      this.keys.clear();
      if (this.unlocked) this.release();
    });
    document.addEventListener("pointerlockchange", () => {
      this.unlocked = false;
      if (!this.captured()) this.keys.clear();
      this.onChange();
    });
    canvas.addEventListener("mousedown", (e) => {
      if (!this.captured()) return;
      if (e.button === 0) {
        this.dragged = false;
        if (!this.unlocked) this.primary = true;
      }
      else if (e.button === 2) this.secondary = true;
      e.preventDefault();
    });
    canvas.addEventListener("mouseup", (e) => {
      if (this.unlocked && e.button === 0 && !this.dragged) this.primary = true;
    });
    addEventListener("contextmenu", (e) => e.preventDefault());
    addEventListener("mousemove", (e) => {
      if (!this.captured()) return;
      // Embedded browsers can refuse pointer lock. Keyboard play and drag-to-look still work;
      // a stationary click remains the primary action, while a drag only looks around.
      if (this.unlocked && !(e.buttons & 1)) return;
      if (this.unlocked && (e.movementX || e.movementY)) this.dragged = true;
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
    return this.forced || this.unlocked || document.pointerLockElement === this.canvas;
  }

  async capture() {
    this.canvas.focus({ preventScroll: true });
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
    try {
      await lock({ unadjustedMovement: true }).catch(() => lock());
    } catch {
      this.unlocked = true;
      this.onChange();
    }
  }

  release() {
    this.unlocked = false;
    this.keys.clear();
    this.onChange();
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
  const { name, player_camera: camera, player_controls: controls } = JSON.parse(new TextDecoder().decode(pkg));
  const gameCamera = camera === "game";
  if (controls) $("help").textContent = controls;
  document.title = name;
  $("name").textContent = name;
  $("status").textContent = "Starting…";
  const amoris = start(module, pkg, { pacing: gameCamera ? "stepped" : { real_time: { speed: 1 } } });
  const canvas = $("view");
  const input = new Input(canvas);
  const seat = new Seat(amoris);
  const state = (window.amorisPlayer = {
    loaded_ms: 0, ready_ms: 0, frames: 0, look: null, stats: null, samples: [],
    amoris, input, seat, gameCamera,
    /** Captures without pointer lock (the check page, where headless Chrome has no lock). */
    forceCapture(on = true) { input.forced = on; return captureChanged(); },
  });
  let ready = false;
  let viewReady = false;
  // The world's halt (a poisoned tick, such as GPU water that failed or timed out): the worker
  // keeps running and says so in its status, which the page shows as it shows a stopped game.
  let halted = null;
  const veil = () => {
    // A compiling water job freezes the game even after the page was ready: the veil says so.
    const compiling = viewReady && amoris.waterStarting;
    $("veil").hidden = ready && input.captured() && !compiling && !halted;
    if (halted) {
      $("status").textContent = "The game halted.";
      $("error").hidden = false;
      $("error").textContent = halted.error?.message || "the world was poisoned";
    } else if (compiling) $("status").textContent = "Preparing the water…";
    else if (ready) $("status").textContent = "Click to play";
  };
  // Ready once the view is and the game ticks: a new GPU water device compiles its pipelines as its
  // first ticks need them (seconds each in Windows Chrome) and the game waits for each, so the
  // game must have run a second (60 ticks) since the view was ready and since the last such job.
  let firstTick = null;
  const settle = () => {
    const tick = amoris.latest()?.tick ?? 0;
    if (viewReady && (firstTick === null || amoris.waterStarting)) firstTick = tick;
    if (ready || halted || !viewReady || amoris.waterStarting || (!gameCamera && tick < firstTick + 60)) return veil();
    ready = true;
    state.ready_ms = performance.now() - t0;
    progress(1);
    veil();
  };
  amoris.onWater(settle);
  amoris.onStatus((s) => {
    halted = s.state === "halted" ? s : null;
    if (!halted) $("error").hidden = true;
    settle();
  });
  // Game-controlled cameras commonly belong to timed games: don't spend a run while loading,
  // waiting for the first click, or paused at the browser's pointer-lock veil.
  let pacing = Promise.resolve();
  const captureChanged = () => {
    veil();
    $("input-mode").hidden = !input.unlocked;
    $("input-mode").textContent = gameCamera ? "Drag to look · Esc pauses" : "Drag to look · Esc menu";
    if (!gameCamera || !ready) return Promise.resolve();
    pacing = pacing.then(() => amoris.command("player.pacing", {
      pacing: input.captured() ? { pacing: "real_time", speed: 1 } : { pacing: "stepped" },
    }));
    return pacing;
  };
  input.onChange = () => {
    captureChanged().catch((e) => console.warn("player pacing:", e.message));
  };
  const capture = () => {
    if (!ready || input.captured()) return;
    input.capture().catch(() => {
      $("error").hidden = false;
      $("error").textContent = "Mouse capture failed. Click again, or open this page in a browser window.";
    });
  };
  $("veil").addEventListener("click", capture);
  canvas.addEventListener("click", capture);
  const vp = await createViewport(canvas, {
    assetsUrl: new URL(".", packageUrl).href,
    gpuMinimal: params.get("gpu_minimal") || undefined,
    beforeFrame(api) {
      const frame = input.take();
      const look = api.raw.first_person(frame.dx, frame.dy, frame.captured, gameCamera);
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
    // The view is ready once the walker's camera is known and the first models and textures are in.
    if (!viewReady && state.look && s.pending_assets === 0 && s.instances > 0) viewReady = true;
    if (!ready) settle();
  });
  state.loaded_ms = performance.now() - t0;
  await amoris.ready;
  // One completed tick poses a game's camera and publishes its assets; hold there for the click.
  if (gameCamera) await amoris.command("time.step", { ticks: 1 });
  const stopped = () => {
    if (!amoris.stopped) return requestAnimationFrame(stopped);
    $("veil").hidden = false;
    $("status").textContent = "The game stopped.";
    $("error").hidden = false;
    $("error").textContent = amoris.stopped.message;
  };
  requestAnimationFrame(stopped);
}
