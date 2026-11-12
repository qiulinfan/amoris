// The game player (docs/spec/web-export.md): a first-person game's `player` seat in a page, as the
// native window plays it (`amoris play <project> --walk`, crates/amoris-app/src/window.rs). The game
// runs in a module Web Worker (amoris.js, game.js) at real time; the engine's renderer
// (viewport/viewport.js) draws its render feed with WebGPU; the keys, the mouse, the wheel, a gamepad
// and the touch controls go through the project's input map in the viewport (`seat_frame`,
// docs/spec/input.md) to `player.act` controls and pulses of the seat, sent as the local human
// player. The camera is the walker's, turned at once by the page's own look.
//
// The page plays the game's sound with WebAudio as the native window plays it with amoris-audio:
// the world's sound sources, the sound events (`sound`, or those the project's `[sounds]` names)
// at their subjects and the water's own sound, with the viewport's `sfx:` presets and water synth.
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

/** Mouse buttons by `MouseEvent.button`, as input names name them (docs/spec/input.md 2). */
const MOUSE = ["MouseLeft", "MouseMiddle", "MouseRight"];
/** The standard gamepad's buttons by index (input.md 2.1). */
const PAD_START = 9;
/** A touch that moves less than this (CSS pixels) is a tap; a touch drag looks at two counts a pixel. */
const TAP_PX = 8;
const TOUCH_LOOK = 2;
/** The touch stick's reach, CSS pixels. */
const STICK_PX = 60;

/** The standard-mapped gamepad that last changed (input.md 7), or null. */
function activePad() {
  let best = null;
  for (const g of (navigator.getGamepads ? navigator.getGamepads() : [])) {
    if (g && g.connected && g.mapping === "standard" && (!best || g.timestamp > best.timestamp)) best = g;
  }
  return best;
}

/** The devices since the last frame, by input name (input.md 2), as the native window gathers them:
 *  keys, mouse buttons and the wheel, the pointer's look (the mouse, or a touch drag), the gamepad,
 *  and the touch controls. The viewport's seat maps them (`seat_frame`); nothing is mapped here. */
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
    this.wheel = 0;
    this.mouse = new Set();
    this.pressed = [];
    this.last = performance.now();
    // The gamepad's buttons down at the last poll (for Start and starting play), and its name.
    this.padDown = new Set();
    this.padName = null;
    // Touch (input.md 5): on once a touch reaches the page; the touch stick's strengths, the touch
    // buttons held (`Touch:<input>`), and the touch looking (pointer id, last point, moved).
    this.touch = false;
    this.touchLayout = null;
    this.touchHeld = new Map();
    this.looker = null;
    this.onTouch = () => {};
    addEventListener("keydown", (e) => {
      if (e.code === "Escape" && this.unlocked) {
        this.release();
        e.preventDefault();
        return;
      }
      if (!this.captured()) return;
      if (!e.repeat) this.pressed.push(e.code);
      this.keys.add(e.code);
      if (e.code !== "Escape" && !e.metaKey && !e.ctrlKey) e.preventDefault();
    });
    addEventListener("keyup", (e) => this.keys.delete(e.code));
    addEventListener("mouseup", (e) => this.mouse.delete(MOUSE[e.button]));
    addEventListener("blur", () => {
      this.keys.clear();
      this.mouse.clear();
      this.touchHeld.clear();
      if (this.unlocked) this.release();
    });
    document.addEventListener("pointerlockchange", () => {
      this.unlocked = false;
      if (!this.captured()) this.keys.clear();
      this.onChange();
    });
    canvas.addEventListener("mousedown", (e) => {
      if (!this.captured()) return;
      const name = MOUSE[e.button];
      if (name) {
        this.mouse.add(name);
        // Without the lock a left press acts when it is let go without a drag (a click), so a drag
        // only looks around.
        if (!(this.unlocked && e.button === 0)) this.pressed.push(name);
      }
      if (e.button === 0) this.dragged = false;
      e.preventDefault();
    });
    canvas.addEventListener("mouseup", (e) => {
      if (this.unlocked && e.button === 0 && !this.dragged) this.pressed.push("MouseLeft");
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
      // A notch a press (Chrome's notch is 100 pixels): down is WheelDown.
      this.wheel += e.deltaMode === 1 ? e.deltaY : e.deltaY / 100;
      while (this.wheel >= 1) { this.wheel -= 1; this.pressed.push("WheelDown"); }
      while (this.wheel <= -1) { this.wheel += 1; this.pressed.push("WheelUp"); }
      e.preventDefault();
    }, { passive: false });
    // The first touch turns the touch controls on. A touch on the game itself is the pointer: its
    // drag looks and a tap clicks. Cancelling the touch's pointerdown keeps the browser from also
    // sending the mouse events it would make of it.
    addEventListener("pointerdown", (e) => {
      if (e.pointerType === "touch" && !this.touch) this.setTouch(true);
    }, { capture: true });
    canvas.addEventListener("pointerdown", (e) => {
      if (e.pointerType !== "touch") return;
      e.preventDefault();
      if (!this.captured() || this.looker) return;
      this.looker = { id: e.pointerId, x: e.clientX, y: e.clientY, moved: 0 };
    });
    canvas.addEventListener("pointermove", (e) => {
      const l = this.looker;
      if (!l || e.pointerId !== l.id) return;
      const dx = e.clientX - l.x, dy = e.clientY - l.y;
      l.x = e.clientX;
      l.y = e.clientY;
      l.moved += Math.abs(dx) + Math.abs(dy);
      if (this.touchLayout?.look !== false) {
        this.dx += dx * TOUCH_LOOK;
        this.dy += dy * TOUCH_LOOK;
      }
    });
    const lift = (e) => {
      const l = this.looker;
      if (!l || e.pointerId !== l.id) return;
      this.looker = null;
      if (e.type === "pointerup" && l.moved < TAP_PX && this.captured()) this.pressed.push("MouseLeft");
    };
    canvas.addEventListener("pointerup", lift);
    canvas.addEventListener("pointercancel", lift);
  }

  captured() {
    return this.forced || this.unlocked || document.pointerLockElement === this.canvas;
  }

  async capture() {
    // Touch screens have no pointer lock: a tap starts play as the keyboard mode does.
    if (this.touch) return this.play();
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
      this.play();
    }
  }

  /** Plays without pointer lock: keyboard, drag-to-look, the gamepad and touch. */
  play() {
    this.unlocked = true;
    this.onChange();
  }

  release() {
    this.unlocked = false;
    this.keys.clear();
    this.touchHeld.clear();
    this.onChange();
  }

  /** Pauses as Escape does: the lock (or the keyboard mode) goes, and the veil comes back. */
  pause() {
    if (document.pointerLockElement === this.canvas) document.exitPointerLock();
    this.forced = false;
    this.release();
  }

  setTouch(on) {
    this.touch = on;
    this.onTouch(on);
  }

  /** The gamepad now in the standard layout, and the buttons pressed since the last poll. */
  pollPad() {
    const g = activePad();
    this.padName = g ? g.id : null;
    if (!g) {
      this.padDown.clear();
      return { pad: null, pressed: [] };
    }
    const buttons = Array.from(g.buttons, (b) => b.value || (b.pressed ? 1 : 0));
    const pressed = [];
    buttons.forEach((v, i) => {
      if (v >= 0.5 && !this.padDown.has(i)) pressed.push(i);
    });
    this.padDown = new Set(buttons.flatMap((v, i) => (v >= 0.5 ? [i] : [])));
    return { pad: { buttons, axes: Array.from(g.axes).slice(0, 4) }, pressed };
  }

  /** This frame's devices (amoris_assets::input::DeviceFrame); presses and look are taken. */
  take(pad) {
    const captured = this.captured();
    const now = performance.now();
    const held = {};
    if (captured) {
      for (const k of this.keys) held[k] = 1;
      for (const m of this.mouse) held[m] = 1;
      for (const [k, v] of this.touchHeld) held[k] = v;
    }
    const frame = {
      held,
      pressed: captured ? this.pressed.splice(0) : (this.pressed.length = 0, []),
      look: [this.dx, this.dy],
      pad,
      dt_s: Math.min(0.1, Math.max(0, (now - this.last) / 1000)),
    };
    this.last = now;
    this.dx = this.dy = 0;
    return { frame, captured };
  }
}

/** The page's touch controls (input.md 5): the move stick, the layout's buttons and pause. They
 *  write `TouchStick…` strengths and `Touch:<input>` into the input's held touches. */
function touchControls(input, layout, onPause) {
  const root = $("touch");
  const zone = $("stick-zone"), base = $("stick"), knob = $("knob");
  zone.hidden = !layout.stick;
  let grip = null;
  const home = () => {
    base.style.left = base.style.top = "";
    knob.style.transform = "";
    for (const k of ["TouchStickLeft", "TouchStickRight", "TouchStickUp", "TouchStickDown"]) input.touchHeld.delete(k);
  };
  zone.addEventListener("pointerdown", (e) => {
    e.preventDefault();
    if (grip) return;
    zone.setPointerCapture(e.pointerId);
    const r = zone.getBoundingClientRect();
    grip = { id: e.pointerId, x: e.clientX, y: e.clientY };
    base.style.left = `${e.clientX - r.left}px`;
    base.style.top = `${e.clientY - r.top}px`;
  });
  zone.addEventListener("pointermove", (e) => {
    if (!grip || e.pointerId !== grip.id) return;
    let x = (e.clientX - grip.x) / STICK_PX, y = (e.clientY - grip.y) / STICK_PX;
    const r = Math.hypot(x, y);
    if (r > 1) { x /= r; y /= r; }
    knob.style.transform = `translate(${x * STICK_PX}px, ${y * STICK_PX}px)`;
    const set = (k, v) => (v > 0 ? input.touchHeld.set(k, v) : input.touchHeld.delete(k));
    set("TouchStickLeft", -x); set("TouchStickRight", x); set("TouchStickUp", -y); set("TouchStickDown", y);
  });
  const letGo = (e) => {
    if (!grip || e.pointerId !== grip.id) return;
    grip = null;
    home();
  };
  zone.addEventListener("pointerup", letGo);
  zone.addEventListener("pointercancel", letGo);
  const buttons = $("touch-buttons");
  buttons.replaceChildren();
  for (const b of layout.buttons) {
    const el = document.createElement("button");
    el.className = "touch-button";
    el.textContent = b.label;
    el.dataset.input = b.input;
    const name = `Touch:${b.input}`;
    el.addEventListener("pointerdown", (e) => {
      e.preventDefault();
      el.setPointerCapture(e.pointerId);
      if (!input.captured()) return;
      input.touchHeld.set(name, 1);
      input.pressed.push(name);
      el.classList.add("down");
    });
    const up = () => { input.touchHeld.delete(name); el.classList.remove("down"); };
    el.addEventListener("pointerup", up);
    el.addEventListener("pointercancel", up);
    buttons.append(el);
  }
  // On the click, after the tap: pausing on its pointerdown put the veil under the finger, and the
  // tap's click then landed on the veil and started play again.
  $("touch-pause").addEventListener("click", (e) => { e.preventDefault(); e.stopPropagation(); onPause(); });
  return {
    show(on) {
      root.hidden = !on;
      if (!on) { grip = null; home(); }
    },
  };
}

/** The seat's sender: the viewport's `player.act` actions, sent as the local human player. */
class Seat {
  constructor(amoris) {
    this.amoris = amoris;
    this.acts = 0;
    this.refused = 0;
  }

  send(actions) {
    this.acts++;
    this.amoris.command("player.act", { actions }, PLAYER).catch((e) => {
      this.refused++;
      console.warn("player.act refused:", e && e.message);
    });
  }
}

/** An event's data as plain JSON: the worker streams amoris-sim's `PlainData` in its serde form
 *  (`"Null"`, `{"Number": 1}`, `{"Object": [[key, value], ...]}`). */
function plain(d) {
  if (d === null || typeof d !== "object") return null;
  if ("Object" in d) return Object.fromEntries(d.Object.map(([k, v]) => [k, plain(v)]));
  if ("Array" in d) return d.Array.map(plain);
  return d.Bool ?? d.Number ?? d.String ?? null;
}

/** The game's sound (crates/amoris-audio's `Audio`, in WebAudio). Silent until the first capture,
 *  a user gesture, starts its context. */
class Sound {
  constructor(base, sounds) {
    this.base = base;
    this.sounds = sounds || {};
    this.ctx = null;
    this.clips = new Map(); // clip -> Promise<AudioBuffer | null>
    this.sources = new Map(); // AudioSource id -> {clip, looped, node, gain, panner}
    this.queue = [];
    this.waterAt = 0;
    this.played = 0;
  }

  start() {
    if (!this.ctx) {
      try {
        this.ctx = new AudioContext({ latencyHint: "interactive" });
      } catch (e) {
        console.warn("no audio:", e.message);
        return;
      }
      this.out = this.ctx.createDynamicsCompressor();
      this.out.connect(this.ctx.destination);
    }
    this.ctx.resume().catch(() => {});
  }

  suspend() { this.ctx?.suspend().catch(() => {}); }

  /** The clip an event plays: `sound`'s own, or the `[sounds]` entry for its name or `prefix.*`. */
  clipOf(name, data) {
    if (name === "sound") return typeof data?.clip === "string" ? data.clip : null;
    if (this.sounds[name]) return this.sounds[name];
    for (const [k, v] of Object.entries(this.sounds)) {
      if (k.endsWith(".*") && name.startsWith(k.slice(0, -1))) return v;
    }
    return null;
  }

  buffer(raw, clip) {
    if (!this.clips.has(clip)) {
      let made;
      if (clip.startsWith("sfx:")) {
        const samples = raw.constructor.sfx(clip.slice(4));
        let b = null;
        if (samples.length) {
          b = this.ctx.createBuffer(1, samples.length, raw.constructor.sfx_rate());
          b.copyToChannel(samples, 0);
        }
        made = Promise.resolve(b);
      } else {
        made = fetch(new URL(clip, this.base))
          .then((r) => { if (!r.ok) throw new Error(`${r.status}`); return r.arrayBuffer(); })
          .then((bytes) => this.ctx.decodeAudioData(bytes));
      }
      this.clips.set(clip, made.catch((e) => { console.warn(`sound ${clip}:`, e.message); return null; }));
    }
    return this.clips.get(clip);
  }

  /** A gain into the mix, behind a panner when the sound has a place: kira's spatial track, heard
   *  fully within 2 m and fading linearly to nothing at 120 m. */
  voice(volume, at) {
    const gain = this.ctx.createGain();
    gain.gain.value = Math.max(0, volume);
    let panner = null;
    if (at) {
      panner = new PannerNode(this.ctx, {
        panningModel: "HRTF", distanceModel: "linear", refDistance: 2, maxDistance: 120, rolloffFactor: 1,
        positionX: at[0], positionY: at[1], positionZ: at[2],
      });
      gain.connect(panner).connect(this.out);
    } else gain.connect(this.out);
    return { gain, panner };
  }

  events(records) {
    for (const r of records) {
      const e = r.event || {};
      const data = plain(e.data) || {};
      const clip = this.clipOf(e.kind, data);
      if (clip) this.queue.push({ clip, subject: e.subject ?? null, volume: data.volume ?? 1, pitch: data.pitch ?? 1 });
    }
  }

  /** Each drawn frame: the listener at the camera, the sources in line with the world's, the
   *  events since, and the water's sound scheduled ahead. */
  frame(raw) {
    const ctx = this.ctx;
    if (!ctx || ctx.state !== "running") { this.queue.length = 0; return; }
    const view = JSON.parse(raw.audio());
    const l = ctx.listener;
    const t = ctx.currentTime;
    l.positionX.setValueAtTime(view.position[0], t);
    l.positionY.setValueAtTime(view.position[1], t);
    l.positionZ.setValueAtTime(view.position[2], t);
    l.forwardX.setValueAtTime(view.forward[0], t);
    l.forwardY.setValueAtTime(view.forward[1], t);
    l.forwardZ.setValueAtTime(view.forward[2], t);
    l.upX.setValueAtTime(view.up[0], t);
    l.upY.setValueAtTime(view.up[1], t);
    l.upZ.setValueAtTime(view.up[2], t);
    const live = new Map(view.sources.filter((s) => s.playing).map((s) => [s.id, s]));
    for (const [id, p] of this.sources) {
      const s = live.get(id);
      if (s && s.clip === p.clip && s.looped === p.looped) continue;
      p.node?.stop();
      p.gain.disconnect();
      this.sources.delete(id);
    }
    for (const [id, s] of live) {
      let p = this.sources.get(id);
      if (!p) {
        p = { clip: s.clip, looped: s.looped, node: null, ...this.voice(s.volume, s.spatial ? s.position : null) };
        this.sources.set(id, p);
        this.buffer(raw, s.clip).then((b) => {
          if (!b || this.sources.get(id) !== p) return;
          p.node = new AudioBufferSourceNode(ctx, { buffer: b, loop: s.looped, playbackRate: Math.max(0.01, s.pitch) });
          p.node.connect(p.gain);
          p.node.start();
        });
      }
      p.gain.gain.setTargetAtTime(Math.max(0, s.volume), t, 0.05);
      p.node?.playbackRate.setTargetAtTime(Math.max(0.01, s.pitch), t, 0.05);
      if (p.panner) {
        p.panner.positionX.setValueAtTime(s.position[0], t);
        p.panner.positionY.setValueAtTime(s.position[1], t);
        p.panner.positionZ.setValueAtTime(s.position[2], t);
      }
    }
    for (const q of this.queue.splice(0)) {
      const at = q.subject == null ? null : raw.position_of(q.subject);
      const v = this.voice(q.volume, at && at.length ? at : null);
      this.buffer(raw, q.clip).then((b) => {
        if (!b) return v.gain.disconnect();
        const node = new AudioBufferSourceNode(ctx, { buffer: b, playbackRate: Math.max(0.01, q.pitch) });
        node.connect(v.gain);
        node.onended = () => v.gain.disconnect();
        node.start();
        this.played++;
      });
    }
    // The water: 2048-frame stereo blocks, kept 0.1 to 0.2 s ahead of the audio clock.
    const n = 2048, ahead = n / ctx.sampleRate;
    if (this.waterAt < t + 0.02) this.waterAt = t + 0.05;
    while (this.waterAt < t + 0.1 + ahead) {
      const pcm = raw.water_sound(n, ctx.sampleRate);
      const b = ctx.createBuffer(2, n, ctx.sampleRate);
      const left = b.getChannelData(0), right = b.getChannelData(1);
      for (let i = 0; i < n; i++) { left[i] = pcm[2 * i]; right[i] = pcm[2 * i + 1]; }
      const node = new AudioBufferSourceNode(ctx, { buffer: b });
      node.connect(this.out);
      node.start(this.waterAt);
      this.waterAt += ahead;
    }
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
  const { name, player_camera: camera, player_controls: controls, player_input: inputMap, sounds } =
    JSON.parse(new TextDecoder().decode(pkg));
  const gameCamera = camera === "game";
  if (controls) $("help").textContent = controls;
  document.title = name;
  $("name").textContent = name;
  $("status").textContent = "Starting…";
  const amoris = start(module, pkg, { pacing: gameCamera ? "stepped" : { real_time: { speed: 1 } } });
  const canvas = $("view");
  const input = new Input(canvas);
  const seat = new Seat(amoris);
  const sound = new Sound(new URL(".", packageUrl), sounds);
  amoris.onEvents((records) => sound.events(records));
  const state = (window.amorisPlayer = {
    loaded_ms: 0, ready_ms: 0, frames: 0, look: null, stats: null, samples: [],
    amoris, input, seat, gameCamera, sound, pad: null, touch: false,
    /** Captures without pointer lock (the check page, where headless Chrome has no lock). */
    forceCapture(on = true) {
      input.forced = on;
      if (on) sound.start();
      else sound.suspend();
      return captureChanged();
    },
  });
  let ready = false;
  let viewReady = false;
  // The touch controls, once the viewport has the input map (input.md 5).
  let touch = null;
  // The world's halt (a poisoned tick, such as GPU water that failed or timed out): the worker
  // keeps running and says so in its status, which the page shows as it shows a stopped game.
  let halted = null;
  const veil = () => {
    // A compiling water job freezes the game even after the page was ready: the veil says so.
    const compiling = viewReady && amoris.waterStarting;
    $("veil").hidden = ready && input.captured() && !compiling && !halted;
    touch?.show(input.touch && $("veil").hidden);
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
    $("input-mode").hidden = !input.unlocked || input.touch;
    $("input-mode").textContent = gameCamera ? "Drag to look · Esc pauses" : "Drag to look · Esc menu";
    if (!gameCamera || !ready) return Promise.resolve();
    pacing = pacing.then(() => amoris.command("player.pacing", {
      pacing: input.captured() ? { pacing: "real_time", speed: 1 } : { pacing: "stepped" },
    }));
    return pacing;
  };
  input.onChange = () => {
    if (input.captured()) sound.start();
    else sound.suspend();
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
  input.onTouch = (on) => {
    state.touch = on;
    veil();
  };
  const vp = await createViewport(canvas, {
    assetsUrl: new URL(".", packageUrl).href,
    gpuMinimal: params.get("gpu_minimal") || undefined,
    beforeFrame(api) {
      // The gamepad is read while the veil is up too: the seat takes its presses then, so the
      // press that starts play does not also act (input.md 3).
      const { pad, pressed } = input.pollPad();
      state.pad = input.padName;
      const { frame, captured } = input.take(pad);
      const reply = JSON.parse(api.raw.seat_frame(JSON.stringify(frame), captured, gameCamera));
      state.look = reply.look;
      sound.frame(api.raw);
      if (reply.actions.length) seat.send(reply.actions);
      // Start pauses; any button starts play once the game is ready (no pointer lock: a gamepad
      // press is no user gesture).
      if (pressed.length) {
        if (captured) {
          if (pressed.includes(PAD_START)) input.pause();
        } else if (ready && !halted) input.play();
      }
    },
  });
  state.viewport = vp;
  if (inputMap) vp.raw.set_input_map(JSON.stringify(inputMap));
  input.touchLayout = JSON.parse(vp.raw.input_map()).touch;
  touch = touchControls(input, input.touchLayout, () => input.pause());
  if (params.get("touch") === "1") input.setTouch(true);
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
