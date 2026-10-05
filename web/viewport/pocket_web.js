// The editor's viewport module (editor/src/viewport/engine.ts `ViewportModule`), served by the host
// at /wasm/pocket_web.js: the engine's renderer compiled to WebAssembly, drawing the host's world
// from the render feed with WebGPU, behind the editor's `Viewport` interface. The editor owns the
// camera, the selection and the gizmo's geometry; this module draws what it is told.
import init, { Viewport } from "./pkg/pocket_viewport.js";

export default async function load() {
  await init({ module_or_path: new URL("./pkg/pocket_viewport_bg.wasm", import.meta.url) });
}

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
  let dpr = window.devicePixelRatio || 1;
  canvas.width = Math.max(1, Math.round(canvas.clientWidth * dpr));
  canvas.height = Math.max(1, Math.round(canvas.clientHeight * dpr));
  const vp = await Viewport.create(canvas);
  let disposed = false;
  let socket = null;
  const picks = [];

  const connect = () => {
    if (disposed || !options.renderUrl) return;
    socket = new WebSocket(options.renderUrl);
    socket.binaryType = "arraybuffer";
    socket.onmessage = (e) => {
      if (e.data instanceof ArrayBuffer) vp.push_frame(new Uint8Array(e.data), performance.now());
    };
    socket.onclose = () => setTimeout(connect, 1000);
  };
  connect();

  const fetchAssets = () => {
    for (const path of vp.take_asset_requests()) {
      fetch(options.assetsUrl + path)
        .then((r) => (r.ok ? r.arrayBuffer() : Promise.reject(new Error(`${r.status} ${r.statusText}`))))
        .then((buf) => vp.deliver_asset(path, new Uint8Array(buf)))
        .catch((err) => vp.asset_failed(path, String(err)));
    }
  };

  const frame = (now) => {
    if (disposed) return;
    fetchAssets();
    vp.render(now);
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
    dispose() {
      disposed = true;
      if (socket) {
        socket.onclose = null;
        socket.close();
      }
    },
  };
}
