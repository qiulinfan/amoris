// How long the viewport's entity-id readback takes in a browser (docs/bench/polish.md 3): an
// expression for tools/web_bench.mjs's EVAL that, on a viewport page (`window.pocketViewport`),
// asks for the visible set and for a pixel pick in turn, polls each frame until the answer comes,
// and returns the delays in milliseconds and frames with the frame time meanwhile.
//
//   EVAL="$(node tools/web_readback.mjs [trials])" node tools/web_bench.mjs \
//     "http://127.0.0.1:8090/viewport/?demo=cubes&dense&count=100000" 6
//
// Printed by itself (no page), it is the expression.
const trials = Number(process.argv[2] || 12);
const expression = `(async () => {
  const vp = window.pocketViewport.raw;
  const frame = () => new Promise((r) => requestAnimationFrame(r));
  const out = { visible: [], pick: [] };
  for (let i = 0; i < ${trials}; i++) {
    const kind = i % 2 ? "pick" : "visible";
    // Let any earlier read finish first.
    for (let k = 0; k < 3; k++) await frame();
    const t0 = performance.now();
    if (kind === "pick") vp.request_pick(Math.floor(innerWidth / 2), Math.floor(innerHeight / 2));
    else vp.request_visible();
    let frames = 0;
    for (;;) {
      await frame();
      frames++;
      const got = kind === "pick" ? vp.take_pick() : vp.take_visible();
      if ((kind === "pick" && got !== -1) || (kind === "visible" && got)) break;
      if (performance.now() - t0 > 10000) { frames = -frames; break; }
    }
    out[kind].push({ ms: +(performance.now() - t0).toFixed(1), frames });
  }
  const stat = (a) => { const s = a.map((x) => x.ms).sort((x, y) => x - y);
    return { n: s.length, min: s[0], p50: s[Math.floor(s.length / 2)], max: s[s.length - 1],
             frames_p50: a.map((x) => x.frames).sort((x, y) => x - y)[Math.floor(a.length / 2)] }; };
  const st = (window.pocketSamples || []).slice(-120);
  const frameMs = st.reduce((a, s) => a + s.frame_ms, 0) / Math.max(st.length, 1);
  return { visible: stat(out.visible), pick: stat(out.pick), frame_ms: +frameMs.toFixed(2),
           gpu_ms: st.length ? +st[st.length - 1].gpu_ms.toFixed(2) : null, raw: out };
})()`;
console.log(expression);
