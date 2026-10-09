// The browser-local run form (charter 5.1; docs/spec/threads.md 7): the sailing game in a Web
// Worker at real time, this page drawing each snapshot it rebuilds from above, and the helm sending
// `world_edit`s as the local human player, on the same path an agent's commands take. With WebGPU
// the engine's renderer (web/viewport) draws the worker's render feed; otherwise, or with
// `?render=2d`, the page draws each rebuilt snapshot as a map from above.
import { loadModule, start } from "./pocket.js";

const $ = (id) => document.getElementById(id);
const fmt = (x, d = 2) => (typeof x === "number" ? x.toFixed(d) : "-");

function xyz(v) {
  if (!v) return null;
  if (Array.isArray(v)) return { x: v[0], y: v[1], z: v[2] };
  return { x: v.x, y: v.y, z: v.z };
}

async function packageUrl(params) {
  if (params.get("package")) return params.get("package");
  try {
    const expected = await (await fetch("expected.json", { cache: "no-store" })).json();
    return expected.projects[0].package;
  } catch {
    return "projects/samples-sailing/package.json";
  }
}

function css(name) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

function draw(canvas, view, fit) {
  const ctx = canvas.getContext("2d");
  const W = canvas.width, H = canvas.height;
  ctx.fillStyle = css("--sea");
  ctx.fillRect(0, 0, W, H);
  const bodies = view.entities.map((e) => ({ e, p: xyz(e.Transform && e.Transform.position) })).filter((b) => b.p);
  if (!bodies.length) return;
  // Fit every body with a margin, easing the frame so it does not jump.
  let [x0, x1, z0, z1] = [Infinity, -Infinity, Infinity, -Infinity];
  for (const { p } of bodies) { x0 = Math.min(x0, p.x); x1 = Math.max(x1, p.x); z0 = Math.min(z0, p.z); z1 = Math.max(z1, p.z); }
  const target = { cx: (x0 + x1) / 2, cz: (z0 + z1) / 2, span: Math.max(x1 - x0 + 16, (z1 - z0 + 16) * W / H, 30) };
  if (!fit.span) Object.assign(fit, target);
  for (const k of ["cx", "cz", "span"]) fit[k] += (target[k] - fit[k]) * 0.05;
  const scale = W / fit.span;
  const sx = (x) => W / 2 + (x - fit.cx) * scale;
  const sy = (z) => H / 2 + (z - fit.cz) * scale;
  ctx.strokeStyle = css("--sea-line");
  ctx.lineWidth = 1;
  const grid = 10;
  for (let gx = Math.floor((fit.cx - fit.span) / grid) * grid; gx < fit.cx + fit.span; gx += grid) {
    ctx.beginPath(); ctx.moveTo(sx(gx), 0); ctx.lineTo(sx(gx), H); ctx.stroke();
  }
  for (let gz = Math.floor((fit.cz - fit.span) / grid) * grid; gz < fit.cz + fit.span; gz += grid) {
    ctx.beginPath(); ctx.moveTo(0, sy(gz)); ctx.lineTo(W, sy(gz)); ctx.stroke();
  }
  for (const { e, p } of bodies) {
    const x = sx(p.x), y = sy(p.z);
    if (e.Boat) {
      const h = (e.Boat.heading_deg * Math.PI) / 180;
      const len = Math.max(4.5 * scale, 14);
      const fx = Math.sin(h), fy = -Math.cos(h);
      ctx.fillStyle = css("--boat");
      ctx.beginPath();
      ctx.moveTo(x + fx * len * 0.6, y + fy * len * 0.6);
      ctx.lineTo(x - fx * len * 0.4 - fy * len * 0.22, y - fy * len * 0.4 + fx * len * 0.22);
      ctx.lineTo(x - fx * len * 0.4 + fy * len * 0.22, y - fy * len * 0.4 - fx * len * 0.22);
      ctx.closePath();
      ctx.fill();
    } else if (e.Cargo) {
      const s = Math.max(0.6 * scale, 6);
      ctx.fillStyle = css("--crate");
      ctx.fillRect(x - s / 2, y - s / 2, s, s);
    }
    ctx.fillStyle = css("--ink");
    ctx.font = "13px system-ui, sans-serif";
    ctx.fillText(e.name, x + 8, y - 8);
  }
  const wind = view.entities.find((e) => e.Wind);
  if (wind) {
    const to = ((wind.Wind.from_deg + 180) * Math.PI) / 180;
    const ax = 40, ay = 40, len = 28;
    ctx.strokeStyle = css("--accent");
    ctx.lineWidth = 3;
    const [dx, dy] = [Math.sin(to), -Math.cos(to)];
    ctx.beginPath();
    ctx.moveTo(ax - dx * len, ay - dy * len);
    ctx.lineTo(ax + dx * len, ay + dy * len);
    ctx.stroke();
    ctx.fillStyle = css("--accent");
    ctx.beginPath();
    ctx.moveTo(ax + dx * (len + 8), ay + dy * (len + 8));
    ctx.lineTo(ax + dx * len - dy * 6, ay + dy * len + dx * 6);
    ctx.lineTo(ax + dx * len + dy * 6, ay + dy * len - dx * 6);
    ctx.closePath();
    ctx.fill();
    ctx.fillText(`wind ${fmt(wind.Wind.speed, 1)} m/s`, ax + 44, ay + 4);
  }
}

function showBoat(view) {
  const boat = view.entities.find((e) => e.Boat);
  const rows = [];
  if (boat) {
    const b = boat.Boat;
    rows.push(["Speed", `${fmt(b.speed)} m/s`], ["Heading", `${fmt(b.heading_deg, 1)}°`], ["Heel", `${fmt(b.heel_deg, 1)}°`],
      ["Apparent wind", `${fmt(b.aws, 1)} m/s at ${fmt(b.awa_deg, 0)}°`], ["Sail", `hoist ${fmt(b.hoist_now)}, sheet ${fmt(b.sheet_now)}`]);
    if (boat.Tally) rows.push(["Crates", `${boat.Tally.taken} of ${boat.Tally.total} (worth ${boat.Tally.worth})`]);
    if (boat.Log) rows.push(["Sailed", `${fmt(boat.Log.distance, 1)} m, best ${fmt(boat.Log.top_speed)} m/s`]);
  }
  $("boat").replaceChildren(...rows.flatMap(([k, v]) => {
    const dt = document.createElement("dt"); dt.textContent = k;
    const dd = document.createElement("dd"); dd.textContent = v;
    return [dt, dd];
  }));
}

/** The engine's renderer on the page, fed by the worker; `null` without WebGPU or with `?render=2d`. */
async function openViewport(pocket, params) {
  if (!navigator.gpu || params.get("render") === "2d") return null;
  try {
    const { createViewport } = await import("./viewport/viewport.js");
    const canvas = $("view");
    canvas.hidden = false;
    $("sea").hidden = true;
    const assetsUrl = new URL(".", new URL(await packageUrl(params), location.href)).href;
    // ?gpu_minimal=first-instance draws on WebGPU's baseline path (docs/spec/webgpu-baseline.md).
    const vp = await createViewport(canvas, { assetsUrl, gpuMinimal: params.get("gpu_minimal") || undefined });
    window.viewport = vp;
    window.pocketViewport = vp;
    pocket.onRender((bytes) => vp.pushFrame(bytes));
    // The recent frames' stats, for tools/web_bench.mjs.
    const samples = (window.pocketSamples = []);
    vp.onStats((s) => {
      samples.push(s);
      if (samples.length > 600) samples.shift();
      window.pocketStats = s;
      $("gpu").textContent = `${fmt(1000 / s.frame_ms, 0)} fps, GPU ${fmt(s.gpu_ms)} ms, ${s.instances} instances, ${s.draw_path} draws`;
    });
    return vp;
  } catch (e) {
    console.warn("no WebGPU viewport, drawing the map instead:", e);
    $("view").hidden = true;
    $("sea").hidden = false;
    return null;
  }
}

export async function runForm(params) {
  const speed = Number(params.get("speed") || 1);
  const { module } = await loadModule();
  const pkg = new Uint8Array(await (await fetch(await packageUrl(params), { cache: "no-store" })).arrayBuffer());
  const pocket = start(module, pkg, { seed: params.get("seed") ? Number(params.get("seed")) : undefined,
    pacing: { real_time: { speed } } });
  window.pocket = pocket;
  const viewport = await openViewport(pocket, params);
  await pocket.ready;
  $("speed").value = String(speed);
  let paused = false;
  let hoisted = true;
  const helm = (value) => pocket.command("world_edit", { edits: [{ op: "set", entity: "Sloop", component: "Boat", value }] },
    { source: "player" }).catch((e) => console.warn(e));
  $("play").onclick = async () => {
    paused = !paused;
    await pocket.command("time_control", { pause: paused });
    $("play").textContent = paused ? "Play" : "Pause";
  };
  $("speed").onchange = () => pocket.command("time_control", { pacing: { real_time: { speed: Number($("speed").value) } } });
  $("step1").onclick = () => pocket.command("step", { ticks: 1 });
  $("step60").onclick = () => pocket.command("step", { ticks: 60 });
  $("rudder").oninput = () => helm({ rudder: Number($("rudder").value) });
  $("sheet").oninput = () => helm({ sheet: Number($("sheet").value) });
  $("hoist").onclick = () => {
    hoisted = !hoisted;
    $("hoist").textContent = hoisted ? "Furl" : "Hoist";
    helm({ hoist: hoisted ? 1 : 0 });
  };
  // The keyboard moves the same controls (and so sends the same commands).
  const nudge = (id, d) => {
    const el = $(id);
    el.value = String(Math.min(Number(el.max), Math.max(Number(el.min), Number(el.value) + d)));
    el.oninput();
  };
  addEventListener("keydown", (e) => {
    if (e.target instanceof HTMLInputElement || e.target instanceof HTMLSelectElement) return;
    const k = e.key.toLowerCase();
    if (k === "a" || k === "arrowleft") nudge("rudder", -0.1);
    else if (k === "d" || k === "arrowright") nudge("rudder", 0.1);
    else if (k === "w" || k === "arrowup") nudge("sheet", 0.1);
    else if (k === "s" || k === "arrowdown") nudge("sheet", -0.1);
    else if (k === " ") $("hoist").onclick();
    else return;
    e.preventDefault();
  });
  let lastHash = "-";
  pocket.onTicks((hashes) => { lastHash = hashes[hashes.length - 1][1]; });
  pocket.onEvents((records) => {
    const list = $("events");
    for (const r of records) {
      const li = document.createElement("li");
      const kind = typeof r.event.kind === "string" ? r.event.kind : JSON.stringify(r.event.kind);
      li.textContent = `tick ${r.event.tick}: ${kind}`;
      list.prepend(li);
    }
    while (list.children.length > 30) list.lastChild.remove();
  });
  const canvas = $("sea");
  const fit = {};
  const frame = () => {
    const info = pocket.latest();
    if (info) {
      const view = pocket.view();
      if (!view.error) {
        if (!viewport) draw(canvas, view, fit);
        showBoat(view);
      }
      $("tick").textContent = info.tick;
      $("time").textContent = `${fmt(info.time.t_s, 1)} s${info.time.paused ? " (paused)" : ""}`;
      $("hash").textContent = lastHash;
      $("snaps").textContent = `${pocket.presenter.received()} rebuilt`;
    }
    if (!pocket.stopped) requestAnimationFrame(frame);
    else $("where").textContent = `The game stopped: ${pocket.stopped.message}`;
  };
  requestAnimationFrame(frame);
}
