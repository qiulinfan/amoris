// Times the physics cache on the level-scale scene in WebAssembly (examples/level_web.rs;
// docs/bench/physics.md, "Static geometry and the cache"), the same steps and JSON fields as
// `physics_bench level` natively: the first tick (which builds every body), the warm ticks, the
// terrain's spec hash, the cache's encode and decode, the stand-in snapshot, restore and fork, a
// fork's continuation, and a cold restore after the scene is dropped. Each operation's result is
// freed (`release`) before the next timed call that would replace it, outside the timer.
//
//   cargo build -p amoris-physics --example level_web --target wasm32-unknown-unknown --release
//   node crates/amoris-physics/tests/web/level.mjs <target>/wasm32-unknown-unknown/release/examples/level_web.wasm CELLS [--side 10] [--warm 150] [--rounds 5] [--check 30]
import { readFile } from "node:fs/promises";

const [wasmPath, cellsArg, ...rest] = process.argv.slice(2);
const flag = (name, d) => {
  const i = rest.indexOf(name);
  return i >= 0 ? Number(rest[i + 1]) : d;
};
const cells = Number(cellsArg);
const side = flag("--side", 10);
const warm = flag("--warm", 150);
const rounds = flag("--rounds", 5);
const checkTicks = flag("--check", 30);

const module = await WebAssembly.compile(await readFile(wasmPath));
if (WebAssembly.Module.imports(module).length > 0) {
  console.error("the module imports", WebAssembly.Module.imports(module));
  process.exit(1);
}
const { exports: x } = await WebAssembly.instantiate(module, {});
const must = (ok, what) => {
  if (!ok) {
    console.error(`${what} failed`);
    process.exit(1);
  }
};
const time = (f) => {
  const t = performance.now();
  const r = f();
  return [performance.now() - t, r];
};
const median = (v) => [...v].sort((a, b) => a - b)[Math.floor(v.length / 2)];
const [BYTES, STAGED, SNAP, COPY, FORKED] = [1, 2, 4, 8, 16];
const text = () => {
  const len = x.describe();
  return new TextDecoder().decode(new Uint8Array(x.memory.buffer, x.text_ptr(), len)).split("\n");
};

const [setupMs] = time(() => x.setup(cells, side));
const [firstMs, first] = time(() => x.tick());
must(first, "the first tick");
const ticks = [];
for (let i = 1; i < warm; i++) {
  const [ms, ok] = time(() => x.tick());
  must(ok, `tick ${i + 1}`);
  ticks.push(ms);
}
const [spec, encode, decode, snapshot, restore, fork] = [[], [], [], [], [], []];
let cacheBytes = 0;
let snapshotBytes = 0;
for (let r = 0; r < rounds; r++) {
  spec.push(time(() => x.terrain_spec_hash())[0]);
  let ms;
  x.release(BYTES | STAGED);
  [ms, cacheBytes] = time(() => x.cache_encode());
  must(cacheBytes, "the cache's encode");
  encode.push(ms);
  const [d, ok] = time(() => x.cache_decode());
  must(ok, "the cache's decode");
  decode.push(d);
  x.release(STAGED | SNAP | COPY);
  [ms, snapshotBytes] = time(() => x.snapshot());
  must(snapshotBytes, "the snapshot");
  snapshot.push(ms);
  const [rs, ok2] = time(() => x.restore());
  must(ok2 && x.copy_same(), "a restore");
  restore.push(rs);
  x.release(COPY | FORKED);
  const [fs, forked] = time(() => x.fork_once());
  must(forked, "a fork");
  fork.push(fs);
}
x.release(BYTES | STAGED | SNAP | COPY | FORKED);
x.snapshot();
const [identity, hashAtWarm] = text();
const colliderBytes = x.section_len(1);
const continues = x.check(checkTicks) === 1;
x.release(SNAP);
x.snapshot();
x.drop_scene();
const [coldMs, cold] = time(() => x.restore());
must(cold, "the cold restore");
const coldSame = x.copy_same();
console.log(
  JSON.stringify({
    scene: "level",
    engine: `node ${process.version}`,
    cells,
    triangles: 2 * cells * cells,
    bodies: 4 * (2 * side + 1) ** 2,
    identity,
    hash_at_warm: hashAtWarm,
    setup_ms: setupMs,
    first_step_ms: firstMs,
    tick_ms: median(ticks),
    warm,
    cache_bytes: cacheBytes,
    collider_bytes: colliderBytes,
    snapshot_bytes: snapshotBytes,
    terrain_spec_hash_ms: median(spec),
    cache_encode_ms: median(encode),
    cache_decode_ms: median(decode),
    snapshot_ms: median(snapshot),
    restore_ms: median(restore),
    fork_ms: median(fork),
    rounds,
    fork_continues: continues,
    fork_checked_ticks: checkTicks,
    cold_restore_ms: coldMs,
    cold_restore_same: coldSame === 1,
  }),
);
