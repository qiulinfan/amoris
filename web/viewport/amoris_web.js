// The editor's viewport module (editor/src/viewport/engine.ts `ViewportModule`), served by the host
// at /wasm/amoris_web.js: the engine's renderer compiled to WebAssembly, drawing the host's world
// from the render feed with WebGPU, behind the editor's `Viewport` interface. The editor owns the
// camera, the selection and the gizmo's geometry; this module draws what it is told.
import init, { Viewport } from "./pkg/amoris_viewport.js";

// One wasm instance per page, however many callers load it at once. wasm-bindgen's init returns
// early only once an instance exists: two calls before the first finishes (React's StrictMode
// mounts the viewport panel twice in development) each instantiate the module, and the glue then
// sends every call, the first instance's closures and frees included, to the second instance's
// memory: "closure invoked recursively or after being dropped", then a corrupted allocator
// (docs/spec/editor.md 5).
let ready = null;
export default function load() {
  ready ??= init({ module_or_path: new URL("./pkg/amoris_viewport_bg.wasm", import.meta.url) }).catch((e) => {
    ready = null;
    throw e;
  });
  return ready;
}

// The live viewports on each canvas, in the order they were created. A canvas has one WebGPU
// context, which every viewport configures with its own device when it is created, so the context
// belongs to the viewport created last, and that need not be the one the page keeps: StrictMode's
// two mounts both create one, the editor awaits its own requests before `createViewport`, and the
// mount it disposes can arrive second. The kept viewport's frames then use another device's
// texture, the GPU rejects every one ("cannot be used with [Device]") and the canvas stays blank.
// So disposing a viewport hands the context back to the newest one left on its canvas
// (docs/spec/editor.md 5).
const live = new WeakMap();

/** Splits the editor's gizmo shapes into packed line segments and triangles. */
function packGizmo(gizmo, dpr) {
  const lines = [];
  const tris = [];
  const seg = (a, b, c, w) => lines.push(a[0], a[1], a[2], b[0], b[1], b[2], c[0], c[1], c[2], c[3], w);
  for (const s of gizmo ? gizmo.shapes : []) {
    const width = (s.width_px ?? 2) * dpr;
    if (s.kind === "line") {
      for (let i = 0; i + 1 < s.points.length; i++) seg(s.points[i], s.points[i + 1], s.color, width);
    } else if (s.kind === "polygon" && s.points.length >= 3) {
      const p0 = s.points[0];
      for (let i = 1; i + 1 < s.points.length; i++) {
        for (const p of [p0, s.points[i], s.points[i + 1]]) tris.push(p[0], p[1], p[2], ...s.color);
      }
      if (s.outline) {
        for (let i = 0; i < s.points.length; i++) seg(s.points[i], s.points[(i + 1) % s.points.length], s.outline, width);
      }
    }
  }
  return [new Float32Array(lines), new Float32Array(tris)];
}

export async function createViewport(canvas, options) {
  if (!navigator.gpu) throw new Error("This browser has no WebGPU.");
  await load();
  let dpr = window.devicePixelRatio || 1;
  canvas.width = Math.max(1, Math.round(canvas.clientWidth * dpr));
  canvas.height = Math.max(1, Math.round(canvas.clientHeight * dpr));
  // options.gpuMinimal: device features to leave out ("first-instance": WebGPU's baseline draws).
  const vp = await Viewport.create(canvas, options.gpuMinimal || undefined);
  // Configures the canvas's context with this viewport's device again (the size is unchanged, so
  // the renderer keeps its targets).
  const reclaim = () => vp.resize(canvas.width, canvas.height);
  const peers = live.get(canvas) ?? [];
  live.set(canvas, peers);
  peers.push(reclaim);
  let disposed = false;
  let socket = null;
  const picks = [];

  const connect = () => {
    if (disposed || !options.renderUrl) return;
    socket = new WebSocket(options.renderUrl);
    socket.binaryType = "arraybuffer";
    socket.onmessage = (e) => {
      if (!disposed && e.data instanceof ArrayBuffer) vp.push_frame(new Uint8Array(e.data), performance.now());
    };
    socket.onclose = () => setTimeout(connect, 1000);
  };
  connect();

  const fetchAssets = () => {
    for (const path of vp.take_asset_requests()) {
      fetch(options.assetsUrl + path)
        .then((r) => (r.ok ? r.arrayBuffer() : Promise.reject(new Error(`${r.status} ${r.statusText}`))))
        .then((buf) => disposed || vp.deliver_asset(path, new Uint8Array(buf)))
        .catch((err) => disposed || vp.asset_failed(path, String(err)));
    }
  };

  // The renderer's stats of its last frame (JSON, parsed only when read: fields come and go with
  // the renderer's version) and the page's frame counters since the last `stats()` (the editor's
  // FrameMeter, editor/src/viewport/meter.ts).
  let rendered = "";
  const meter = { last: 0, frames: 0, sum: 0, max: 0, draws: 0, drawn: 0 };

  const frame = (now) => {
    if (disposed) return;
    fetchAssets();
    const t0 = performance.now();
    const s = vp.render(now);
    meter.drawn += performance.now() - t0;
    meter.draws++;
    if (meter.last > 0 && now > meter.last) {
      const dt = now - meter.last;
      meter.frames++;
      meter.sum += dt;
      meter.max = Math.max(meter.max, dt);
    }
    meter.last = now;
    if (s && s !== "{}") rendered = s;
    if (picks.length) {
      const r = vp.take_pick();
      if (r >= 0) {
        const p = picks.shift();
        p.resolve(r > 0 ? { entity: r } : null);
        if (picks.length) vp.request_pick(picks[0].x, picks[0].y);
      }
    }
    requestAnimationFrame(frame);
  };
  requestAnimationFrame(frame);

  return {
    setCamera(c) {
      const ortho = c.projection === "orthographic" ? c.ortho_height : 0;
      vp.set_camera_look(...c.position, ...c.target, ...c.up, c.fov_y_deg, c.near, ortho);
    },
    pick(x, y) {
      // A ray against the parts' oriented boxes, answered at once (the GPU id pass, whose
      // readback the browser delays by many frames, is kept for coverage questions).
      const r = vp.pick_ray(x * dpr, y * dpr);
      return Promise.resolve(r.length ? { entity: r[0], position: [r[1], r[2], r[3]] } : null);
    },
    resize(width, height, devicePixelRatio) {
      dpr = devicePixelRatio || 1;
      const w = Math.max(1, Math.round(width * dpr)), h = Math.max(1, Math.round(height * dpr));
      canvas.width = w;
      canvas.height = h;
      vp.resize(w, h);
    },
    setGizmo(g) {
      const [lines, tris] = packGizmo(g, dpr);
      vp.set_gizmo(lines, tris);
    },
    setSelection(ids, hovered) {
      vp.set_selection(new Float64Array(ids), hovered ?? 0);
    },
    setOverlays(o) {
      vp.set_overlays(!!o.grid, !!o.axes);
    },
    stats() {
      let r = {};
      try {
        r = rendered ? JSON.parse(rendered) : {};
      } catch {
        r = {};
      }
      const n = meter.frames;
      const passes = Array.isArray(r.passes) ? r.passes : [];
      const out = {
        frames: n,
        fps: n ? 1000 / (meter.sum / n) : null,
        frame_ms: n ? meter.sum / n : null,
        frame_max_ms: n ? meter.max : null,
        render_ms: meter.draws ? meter.drawn / meter.draws : null,
        // Without timestamp queries the renderer reports no passes and no GPU time.
        gpu_ms: passes.length && typeof r.gpu_ms === "number" ? r.gpu_ms : null,
        passes,
        renderer: r,
      };
      meter.frames = meter.sum = meter.max = meter.draws = meter.drawn = 0;
      return out;
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      if (socket) {
        socket.onclose = null;
        socket.close();
      }
      // The renderer's wasm memory now, not when the collector finds the wrapper; its GPU objects
      // still go with the collector (wgpu's WebGPU backend destroys nothing on drop). The frame
      // loop and the callbacks above check `disposed` first.
      vp.free();
      peers.splice(peers.indexOf(reclaim), 1);
      peers.at(-1)?.();
    },
  };
}
