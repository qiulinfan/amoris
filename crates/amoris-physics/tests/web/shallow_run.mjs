// The shallow-water spike's WebAssembly run (examples/shallow_web.rs; docs/spikes/shallow-water.md),
// shared by Node (shallow.mjs) and Chrome (shallow.html): for each scheme, the median substep at
// each size, then the hash chain `shallow_bench` prints natively. Returns the JSON lines.
export async function run(bytes, sizes, target) {
  const module = await WebAssembly.compile(bytes);
  if (WebAssembly.Module.imports(module).length > 0) {
    throw new Error("the module imports " + JSON.stringify(WebAssembly.Module.imports(module)));
  }
  const { exports: x } = await WebAssembly.instantiate(module, {});
  const hex = () =>
    (x.hash(1) >>> 0).toString(16).padStart(8, "0") + (x.hash(0) >>> 0).toString(16).padStart(8, "0");
  const lines = [];
  for (const [pipes, scheme] of [[0, "advect"], [1, "pipes"]]) {
    for (const n of sizes) {
      x.setup(n, pipes);
      x.run(10);
      const rounds = Math.min(200, Math.max(5, Math.floor(30_000_000 / (n * n))));
      const per = [];
      for (let r = 0; r < rounds; r++) {
        const t = performance.now();
        x.run(10);
        per.push((performance.now() - t) / 10);
      }
      per.sort((a, b) => a - b);
      const ms = per[Math.floor(per.length / 2)];
      lines.push({ target, scheme, n, columns: n * n, ms_per_substep: Number(ms.toFixed(4)),
        ns_per_column: Number(((ms * 1e6) / (n * n)).toFixed(3)) });
    }
    x.setup(128, pipes);
    const chain = [];
    for (let i = 0; i < 10; i++) {
      x.run(60);
      chain.push(hex());
    }
    lines.push({ scheme, chain });
  }
  return lines;
}
