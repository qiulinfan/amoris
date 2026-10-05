// The web check (docs/spec/checks.md 7.2), run when the page is loaded with `?check`: the shipped
// page's own module and worker, driven as a page drives them, with what `cargo xtask check`'s web
// step put beside it (expected.json: the native `pocket hashes` of every workload and seed, the
// packages and inputs, the replays the native replay check recorded). It sets document.title to
// `DONE <json>` or `FAIL <json>` (tools/webcheck.py's protocol) with `errors`, `warnings` and
// `measurements` in the shapes xtask reads.
//
// - determinism: every seed stepped through its inputs (sent as commands with `at`), the worker's
//   world hash after every tick against the native chain (web.cross_target_diverged names the
//   first differing tick); one seed again with the presenter acknowledging late, so the worker's
//   flow control skips publications, and once in real time at 8x, since pacing changes no result.
// - threads: each snapshot rebuilt on the page to the worker's hash, versions one apart
//   (`publication_order`), the hash stream complete and in order, never more than two snapshots
//   unacknowledged; and the ordering test through messages (threads.md 11): edits of the sailing
//   sample's Sloop.Boat from the editor, two developers and a player, all inputs of one tick, sent
//   in several orders, must give one world hash and the player's value (the last in the canonical
//   order), else `web.threads_failed {test: "ordering"}`.
// - replay: each native recording replays in the worker, which has no transpiler.
// - controls: a project with `expect` (checks.md 8.6) runs its seeds stepped like any workload, but
//   its problems are not the page's errors: they go to `controls` as `{project, expected, got,
//   caught, errors}`, which xtask judges (`check.control_passed`, `check.control_wrong_failure`);
//   a control that is not caught fails the page.
import { loadModule, start } from "./pocket.js";

const REAL_TIME_SPEED = 8;
const LATE_ACK_MS = 25;
const MAX_UNACKED = 2;
// The ordering test: its project, the tick its edits are inputs of, the sources (in the canonical
// order, threads.md 5.2) with the rudder each sets, who steps and reads, and the arrival orders.
const ORDERING_PROJECT = "samples/sailing";
const ORDERING_AT = 5;
const ORDERING_SOURCES = ["editor", { developer: 0 }, { developer: 1 }, { player: 0 }];
const ORDERING_RUDDERS = [0.11, -0.22, 0.33, -0.44];
const ORDERING_READER = { developer: 2 };
const ORDERINGS = [[0, 1, 2, 3], [3, 2, 1, 0], [1, 3, 0, 2], [2, 0, 3, 1], [3, 0, 2, 1], [0, 3, 1, 2]];
const r3 = (x) => Math.round(x * 1000) / 1000;

function stats(values) {
  const v = [...values].sort((a, b) => a - b);
  if (!v.length) return null;
  const at = (q) => v[Math.min(v.length - 1, Math.round((v.length - 1) * q))];
  return { n: v.length, median: r3(at(0.5)), p95: r3(at(0.95)), mean: r3(v.reduce((a, b) => a + b, 0) / v.length),
    min: r3(v[0]), max: r3(v[v.length - 1]) };
}

function problem(code, message, detail = {}) {
  return { code, message, detail };
}

function measure(list, name, value, unit, stat = "value", samples = 1) {
  if (value === null || value === undefined || Number.isNaN(value)) return;
  list.push({ name, value: r3(value), unit, stat, samples });
}

function waitFor(cond, ms, what) {
  return new Promise((resolve, reject) => {
    const t0 = performance.now();
    const look = () => {
      if (cond()) resolve();
      else if (performance.now() - t0 > ms) reject(new Error(`timed out after ${ms} ms waiting for ${what}`));
      else setTimeout(look, 2);
    };
    look();
  });
}

export function parseInputs(text) {
  const out = [];
  text.split("\n").forEach((raw, i) => {
    const t = raw.trim();
    if (!t || t.startsWith("#") || t.startsWith("//")) return;
    const v = JSON.parse(t);
    out.push({ line: i + 1, tick: v.tick, source: v.source, name: v.name, params: v.params || {} });
  });
  return out;
}

async function fetchBytes(url) {
  const r = await fetch(url, { cache: "no-store" });
  if (!r.ok) throw new Error(`${url}: HTTP ${r.status}`);
  return new Uint8Array(await r.arrayBuffer());
}

// One run of a seed: the inputs as commands held for their ticks, then `ticks` ticks.
async function runSeed(module, pkg, inputs, run, variant) {
  const p = start(module, pkg, { seed: run.seed, pacing: "stepped", ackDelayMs: variant.lateAckMs || 0 });
  const hashes = [];
  p.onTicks((list) => { for (const [t, h] of list) hashes[t] = h; });
  const out = { variant: variant.label, seed: run.seed, refused: [], step: null };
  try {
    await p.ready;
    const sent = inputs.map((i) => p.command(i.name, i.params, { source: i.source, at: i.tick })
      .catch((e) => out.refused.push({ line: i.line, tick: i.tick, error: e })));
    const t0 = performance.now();
    if (variant.realTime) {
      await p.command("time_control", { pacing: { real_time: { speed: variant.realTime } } });
      await waitFor(() => hashes[run.ticks] !== undefined, 120000, `tick ${run.ticks} in real time`);
      await p.command("time_control", { pause: true });
    } else {
      out.step = await p.command("step", { ticks: run.ticks });
    }
    out.run_ms = performance.now() - t0;
    await waitFor(() => hashes[run.ticks] !== undefined, 10000, `the hash of tick ${run.ticks}`);
    await Promise.all(sent);
    out.perf = await p.perf();
  } finally {
    out.hashes = hashes;
    out.problems = p.problems();
    out.maxUnacked = p.maxUnacked;
    out.rebuildMs = p.rebuildMs;
    out.transferMs = p.transferMs;
    out.timeline = p.timeline;
    out.stopped = p.stopped;
    p.close();
  }
  return out;
}

// The ordering test (threads.md 11) through messages: one fresh game per arrival order.
async function orderingRun(module, pkg, errors) {
  const results = [];
  for (const order of ORDERINGS) {
    const p = start(module, pkg, { seed: 1, pacing: "stepped" });
    try {
      await p.ready;
      const edits = order.map((i) => p.command("world_edit",
        { edits: [{ op: "set", entity: "Sloop", component: "Boat", value: { rudder: ORDERING_RUDDERS[i] } }] },
        { source: ORDERING_SOURCES[i], at: ORDERING_AT }));
      const step = await p.command("step", { ticks: ORDERING_AT }, { source: ORDERING_READER });
      await Promise.all(edits);
      const got = await p.command("world_get", { entity: "Sloop", components: ["Boat"] }, { source: ORDERING_READER });
      results.push({ order, world_hash: step.world_hash, rudder: got.components.Boat.rudder });
    } catch (e) {
      errors.push(problem("web.threads_failed", `the ordering run ${JSON.stringify(order)} failed: ${e.message || JSON.stringify(e)}`,
        { test: "ordering", order, error: e }));
      return { orders: results.length, identical: false };
    } finally {
      p.close();
    }
  }
  const hashes = new Set(results.map((r) => r.world_hash));
  const last = ORDERING_RUDDERS[ORDERING_RUDDERS.length - 1];
  const wrong = results.filter((r) => r.rudder !== last);
  if (hashes.size !== 1 || wrong.length) {
    errors.push(problem("web.threads_failed",
      `${ORDERINGS.length} arrival orders of one tick's edits gave ${hashes.size} world hashes; ${wrong.length} did not end with the player's rudder ${last}`,
      { test: "ordering", results }));
  }
  return { orders: results.length, identical: hashes.size === 1 && !wrong.length, world_hash: results[0] && results[0].world_hash };
}

function compare(project, run, got, errors) {
  for (let t = 0; t <= run.ticks; t++) {
    if (got.hashes[t] !== run.hashes[t]) {
      errors.push(problem("web.cross_target_diverged",
        `${project.name} seed ${run.seed} (${got.variant}): the web build's world hash after tick ${t} differs from the native build's`,
        { project: project.name, seed: run.seed, tick: t, native: run.hashes[t], web: got.hashes[t] ?? null, variant: got.variant }));
      return false;
    }
  }
  return true;
}

function judge(project, run, got, errors, warnings) {
  const ok = compare(project, run, got, errors);
  for (const p of got.problems) errors.push({ ...p, detail: { ...p.detail, project: project.name, seed: run.seed, variant: got.variant } });
  if (got.maxUnacked > MAX_UNACKED) {
    errors.push(problem("web.threads_failed", `${got.maxUnacked} snapshots were unacknowledged at once; at most ${MAX_UNACKED} may be`,
      { test: "flow_control", max_unacked: got.maxUnacked, variant: got.variant }));
  }
  for (const r of got.refused) {
    errors.push(problem("web.test_failed", `the input on line ${r.line} (tick ${r.tick}) was refused: ${r.error.message}`,
      { test: "inputs", project: project.name, seed: run.seed, ...r }));
  }
  if (got.step && got.step.errors && got.step.errors.length) {
    warnings.push(problem("web.step_errors", `${got.step.errors.length} failed invocations during the run`,
      { project: project.name, seed: run.seed, errors: got.step.errors.slice(0, 3) }));
  }
  const skipped = got.perf ? got.perf.posted.filter((x) => !x).length : 0;
  if (got.variant.includes("late") && skipped === 0) {
    warnings.push(problem("web.flow_control_idle", "the late presenter never made the worker skip a publication", { seed: run.seed }));
  }
  return { variant: got.variant, seed: run.seed, ticks: run.ticks, identical: ok, final_hash: got.hashes[run.ticks],
    run_ms: r3(got.run_ms || 0), skipped_publications: skipped, max_unacked: got.maxUnacked, snapshots: got.rebuildMs.length };
}

export async function runCheck() {
  const errors = [], warnings = [], measurements = [], runs = [], replays = [], controls = [];
  let ordering = null;
  const expected = await (await fetch("expected.json", { cache: "no-store" })).json();
  const loaded = await loadModule();
  measure(measurements, "web.start.compile", loaded.compile_ms, "ms");
  measure(measurements, "web.start.instantiate_page", loaded.instantiate_ms, "ms");
  const ticks = { step: [], publish: [], hash: [] };
  const rebuild = [], transfer = [];
  let firstRun = null;
  for (const project of expected.projects) {
    const pkg = await fetchBytes(project.package);
    const inputs = parseInputs(new TextDecoder().decode(await fetchBytes(project.inputs)));
    // A control's problems are its outcome, not the page's errors; it runs each seed stepped.
    const control = project.expect ? { errors: [], warnings: [] } : null;
    const variants = project.runs.map((run, i) => ({ run, label: i === 1 && !control ? "stepped, late acks" : "stepped",
      lateAckMs: i === 1 && !control ? LATE_ACK_MS : 0 }));
    if (!control) {
      if (project.runs.length === 1) variants.push({ run: project.runs[0], label: "stepped, late acks", lateAckMs: LATE_ACK_MS });
      variants.push({ run: project.runs[0], label: `real time x${REAL_TIME_SPEED}`, realTime: REAL_TIME_SPEED });
    }
    const errs = control ? control.errors : errors, warns = control ? control.warnings : warnings;
    for (const v of variants) {
      let got;
      try {
        got = await runSeed(loaded.module, pkg, inputs, v.run, v);
      } catch (e) {
        errs.push(problem("web.test_failed", `${project.name} seed ${v.run.seed} (${v.label}) did not finish: ${e.message || JSON.stringify(e)}`,
          { test: "determinism", project: project.name, seed: v.run.seed, variant: v.label }));
        continue;
      }
      const verdict = judge(project, v.run, got, errs, warns);
      if (control) continue;
      firstRun = firstRun || got;
      runs.push(verdict);
      if (got.perf && !v.realTime && !v.lateAckMs) {
        got.perf.step_ms.forEach((x) => ticks.step.push(x));
        got.perf.publish_ms.forEach((x, i) => (got.perf.posted[i] ? ticks.publish : ticks.hash).push(x));
        rebuild.push(...got.rebuildMs);
        transfer.push(...got.transferMs);
      }
    }
    if (control) {
      const got = [...new Set(control.errors.map((e) => e.code))];
      // The first problem of each code, so xtask's judgement sees every code the control failed with.
      controls.push({ project: project.name, expected: project.expect, got, caught: got.includes(project.expect),
        errors: got.map((code) => control.errors.find((e) => e.code === code)) });
      continue;
    }
    if (project.name === ORDERING_PROJECT && !ordering) ordering = await orderingRun(loaded.module, pkg, errors);
    for (const url of project.replays || []) {
      const p = start(loaded.module, pkg, {});
      try {
        await p.ready;
        const result = await p.verify(await fetchBytes(url));
        replays.push({ replay: url, ...result });
        if (result.error || !result.identical) {
          errors.push(problem("web.replay_diverged", `${url} did not replay identically in the worker`,
            { project: project.name, replay: url, tick: result.divergence ? result.divergence.tick : null, result }));
        }
      } finally {
        p.close();
      }
    }
  }
  if (firstRun) {
    const t = firstRun.timeline;
    measure(measurements, "web.start.module_ready", loaded.loaded_at, "ms", "since navigation");
    measure(measurements, "web.start.worker_created", t.created, "ms", "since navigation");
    measure(measurements, "web.start.worker_ready", t.ready, "ms", "since navigation");
    measure(measurements, "web.start.first_snapshot", t.first_snapshot, "ms", "since navigation");
    measure(measurements, "web.start.worker_instantiate", t.worker_instantiate_ms, "ms");
    measure(measurements, "web.start.game_build", t.game_build_ms, "ms");
  }
  for (const [name, values] of [["web.tick.sail", ticks.step], ["web.publish.sail", ticks.publish],
    ["web.hash.sail", ticks.hash], ["web.snapshot.rebuild", rebuild], ["web.snapshot.transfer", transfer]]) {
    const s = stats(values);
    if (!s) continue;
    measure(measurements, name, s.median, "ms", "median", s.n);
    measure(measurements, name, s.p95, "ms", "p95", s.n);
  }
  if (!ordering) {
    errors.push(problem("web.threads_failed", `the ordering test needs ${ORDERING_PROJECT} among the web workloads`,
      { test: "ordering", project: ORDERING_PROJECT }));
  }
  const summary = { ok: errors.length === 0 && controls.every((c) => c.caught),
    cross_origin_isolated: self.crossOriginIsolated, runs, ordering, controls, replays,
    errors, warnings, measurements, user_agent: navigator.userAgent };
  return summary;
}

export async function main(out) {
  let summary;
  try {
    summary = await runCheck();
  } catch (e) {
    summary = { ok: false, errors: [problem("web.test_failed", `the check page failed: ${e.stack || e.message || JSON.stringify(e)}`, { test: "page" })] };
  }
  if (out) out.textContent = JSON.stringify(summary, null, 2);
  document.title = (summary.ok ? "DONE " : "FAIL ") + JSON.stringify(summary);
}
