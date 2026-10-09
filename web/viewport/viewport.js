// The engine's renderer in a page (crates/pocket-viewport compiled to WebAssembly, drawing with
// WebGPU). The editor's viewport and spectator pages use it through createViewport:
//
//   const vp = await createViewport(canvas, { renderUrl: "ws://127.0.0.1:7878/render", assetsUrl: "/assets/",
//                                             gpuMinimal: "first-instance" /* optional */ });
//   vp.setCamera([x, y, z], [qx, qy, qz, qw], fovDeg);   // or vp.useSceneCamera()
//   vp.onStats((s) => ...);                               // {gpu_ms, instances, tick, passes, frame_ms,
//                                                         //  render_ms, draw_path, draw_calls}, per frame drawn
//   vp.pushFrame(bytes);                                  // a render-feed frame from elsewhere (a game worker)
//   vp.dispose();
//
// The page owns the WebSocket of the host's render feed, the asset fetches and the frame loop.
import init, { Viewport } from "./pkg/pocket_viewport.js";

let ready = null;
function load() {
  if (!ready) ready = init({ module_or_path: new URL("./pkg/pocket_viewport_bg.wasm", import.meta.url) });
  return ready;
}

export async function createViewport(canvas, options = {}) {
  if (!navigator.gpu) throw new Error("This browser has no WebGPU.");
  await load();
  const dpr = window.devicePixelRatio || 1;
  const size = () => [Math.max(1, Math.round(canvas.clientWidth * dpr)), Math.max(1, Math.round(canvas.clientHeight * dpr))];
  [canvas.width, canvas.height] = size();
  // gpuMinimal: device features to leave out, as POCKET_GPU_MINIMAL natively ("first-instance"
  // forces WebGPU's baseline draw path).
  const vp = await Viewport.create(canvas, options.gpuMinimal || undefined);
  const assetsUrl = options.assetsUrl || "/assets/";
  const listeners = [];
  let socket = null;
  let disposed = false;
  let frames = 0;
  let last = performance.now();
  const frameTimes = [];

  if (options.renderUrl) {
    const connect = () => {
      socket = new WebSocket(options.renderUrl);
      socket.binaryType = "arraybuffer";
      socket.onmessage = (e) => {
        if (e.data instanceof ArrayBuffer) vp.push_frame(new Uint8Array(e.data), performance.now());
      };
      socket.onclose = () => { if (!disposed) setTimeout(connect, 1000); };
    };
    connect();
  }

  async function fetchAssets() {
    for (const path of vp.take_asset_requests()) {
      fetch(assetsUrl + path)
        .then((r) => (r.ok ? r.arrayBuffer() : Promise.reject(new Error(`${r.status} ${r.statusText}`))))
        .then((buf) => vp.deliver_asset(path, new Uint8Array(buf)))
        .catch((err) => vp.asset_failed(path, String(err)));
    }
  }

  const observer = new ResizeObserver(() => {
    const [w, h] = size();
    if (w !== canvas.width || h !== canvas.height) {
      canvas.width = w;
      canvas.height = h;
      vp.resize(w, h);
    }
  });
  observer.observe(canvas);

  function tick(now) {
    if (disposed) return;
    if (options.beforeFrame) options.beforeFrame(api, frames);
    fetchAssets();
    const t0 = performance.now();
    const stats = JSON.parse(vp.render(now));
    // Under a frames-in-flight limit (`raw.set_max_frames_in_flight`) a call made while the GPU
    // is that far behind draws nothing: it is not a frame.
    if (stats.skipped) {
      requestAnimationFrame(tick);
      return;
    }
    // The CPU time of the render call (the renderer's own cpu_ms has no clock in wasm).
    stats.render_ms = performance.now() - t0;
    const dt = now - last;
    last = now;
    frameTimes.push(dt);
    if (frameTimes.length > 120) frameTimes.shift();
    stats.frame_ms = frameTimes.reduce((a, b) => a + b, 0) / frameTimes.length;
    frames++;
    for (const l of listeners) l(stats);
    requestAnimationFrame(tick);
  }
  requestAnimationFrame(tick);

  const api = {
    raw: vp,
    setCamera(pos, rot, fovDeg = 60) {
      vp.set_camera(pos[0], pos[1], pos[2], rot[0], rot[1], rot[2], rot[3], fovDeg);
    },
    useSceneCamera() { vp.use_scene_camera(); },
    camera() { return Array.from(vp.camera()); },
    onStats(f) { listeners.push(f); },
    pushFrame(bytes) { return vp.push_frame(bytes, performance.now()); },
    resize(w, h) { vp.resize(w, h); },
    demoCubes(count, dense) { vp.demo_cubes(count, dense, performance.now()); },
    demoMixed(n) { vp.demo_mixed(n, performance.now()); },
    demoCubesCamera(frame, dense) { vp.demo_cubes_camera(frame, dense); },
    demoOccluders() { vp.demo_occluders(performance.now()); },
    setOcclusion(mode) { vp.set_occlusion(mode); },
    demoLod(n, spacing, detail, t) { vp.demo_lod(n, spacing, detail, t, performance.now()); },
    demoLodCamera(n, spacing, t) { vp.demo_lod_camera(n, spacing, t); },
    setLod(mode) { vp.set_lod(mode); },
    dispose() {
      disposed = true;
      observer.disconnect();
      if (socket) socket.close();
    },
  };
  return api;
}
