// Evidence that the script editor type-checks with the project's own declarations (docs/sdk.md):
// against a dev server on a real host, it waits for `scripts.types`, opens scripts/rules.ts, makes
// two mistakes (a misspelt column, a column the query's `fields` leaves out), prints Monaco's
// TypeScript markers and the completions of a query's columns, saves two screenshots, and prints
// the worker's findings on the mistakes of sdk_mistakes.ts.txt (docs/sdk.md's list).
//
//   pocket serve samples/sailing --port 7911
//   EDITOR_PORT=5191 POCKET_HOST=http://127.0.0.1:7911 bun run dev
//   bun tools/sdk_check.ts ../docs/evidence/sdk http://127.0.0.1:5191/
import { Cdp, launchChrome } from "./cdp";
const OUT = process.argv[2]!;
const URL_ = process.argv[3] ?? "http://127.0.0.1:5191/";
const { proc, pageWs } = await launchChrome(9417);
// Chrome is stopped however this ends: a failed step, an exception, Ctrl-C.
const stop = () => {
  try {
    proc.kill();
  } catch {
    // already gone
  }
};
process.on("exit", stop);
process.on("SIGINT", () => process.exit(130));
process.on("SIGTERM", () => process.exit(143));
const cdp = await Cdp.connect(pageWs);
async function js<T = unknown>(expression: string): Promise<T> {
  const r = await cdp.send<{ result: { value?: T }; exceptionDetails?: { text: string; exception?: { description?: string } } }>("Runtime.evaluate", {
    expression: `(async () => { ${expression} })()`, awaitPromise: true, returnByValue: true,
  });
  if (r.exceptionDetails) throw new Error(`${r.exceptionDetails.text}: ${r.exceptionDetails.exception?.description ?? ""}`);
  return r.result.value as T;
}
try {
  await cdp.send("Page.enable");
  await cdp.send("Emulation.setDeviceMetricsOverride", { width: 1600, height: 1000, deviceScaleFactor: 1, mobile: false });
  await cdp.send("Page.navigate", { url: URL_ });
  for (let i = 0; i < 100; i++) {
    await Bun.sleep(200);
    const ok = await js<boolean>(`return !!window.pocket && !!window.pocket.scripts.getState().types?.text`).catch(() => false);
    if (ok) break;
  }
  const info = await js(`const t = window.pocket.scripts.getState().types; return { components: t.components, from: t.project_from, files: Object.keys(t.text) };`);
  console.log("types:", JSON.stringify(info));
  await js(`window.pocket.openPanel("scripts"); window.pocket.scripts.getState().openFile("scripts/rules.ts", 27); return 1;`);
  // Wait for the file's model (Monaco loads lazily with the panel; a loaded machine takes longer).
  const opened = await js<boolean>(`
    const m = await import("/src/panels/scripts/monaco.ts");
    for (let i = 0; i < 300; i++) {
      if (m.monaco.editor.getModels().some((x) => x.uri.path.endsWith("scripts/rules.ts"))) return true;
      await new Promise((r) => setTimeout(r, 100));
    }
    return false;`);
  if (!opened) throw new Error("scripts/rules.ts did not open in the editor within 30 s");
  // Edit the model as a person would: a wrong field name and a column the query leaves out.
  const markers = await js(`
    const m = await import("/src/panels/scripts/monaco.ts");
    const model = m.monaco.editor.getModels().find((x) => x.uri.path.endsWith("scripts/rules.ts"));
    const text = model.getValue().replace("Math.abs(b.speed[r])", "Math.abs(b.sped[r])").replace("const set = b.hoist_now[r] >= 0.5;", "const set = b.heel_deg[r] >= 0.5;");
    model.setValue(text);
    for (let i = 0; i < 60; i++) {
      await new Promise((r) => setTimeout(r, 200));
      const ms = m.monaco.editor.getModelMarkers({ resource: model.uri }).filter((x) => x.owner === "typescript");
      if (ms.length >= 2) return ms.map((x) => ({ line: x.startLineNumber, col: x.startColumn, message: x.message }));
    }
    return m.monaco.editor.getModelMarkers({ resource: model.uri }).map((x) => ({ owner: x.owner, line: x.startLineNumber, message: x.message }));
  `);
  console.log("markers:", JSON.stringify(markers, null, 1));
  await Bun.sleep(300);
  console.log("status bar:", await js(`return document.querySelector(".code-status")?.textContent ?? "";`));
  await js(`
    const m = await import("/src/panels/scripts/monaco.ts");
    const ed = m.monaco.editor.getEditors()[0];
    ed.setPosition({ lineNumber: 27, column: 40 });
    ed.revealLineInCenter(27);
    ed.focus();
    ed.trigger("sdk-check", "editor.action.showHover", {});
    return 1;`);
  await Bun.sleep(1200);
  const shot = await cdp.send<{ data: string }>("Page.captureScreenshot", { format: "png" });
  await Bun.write(`${OUT}/editor-sdk-error.png`, Buffer.from(shot.data, "base64"));
  // Completions on a query's columns.
  await js(`
    const m = await import("/src/panels/scripts/monaco.ts");
    const ed = m.monaco.editor.getEditors()[0];
    const model = ed.getModel();
    model.setValue(model.getValue().replace("Math.abs(b.sped[r])", "Math.abs(b.speed[r])").replace("b.heel_deg[r]", "b.hoist_now[r]").replace("const l = boats.cols.Log;", "const l = boats.cols.Log;\\n        boats.cols.Boat."));
    const line = model.getValue().split("\\n").findIndex((l) => l.trim() === "boats.cols.Boat.") + 1;
    ed.setPosition({ lineNumber: line, column: model.getLineMaxColumn(line) });
    ed.revealLineInCenter(line);
    ed.focus();
    ed.trigger("sdk-check", "editor.action.triggerSuggest", {});
    return line;`);
  await Bun.sleep(1500);
  const shot2 = await cdp.send<{ data: string }>("Page.captureScreenshot", { format: "png" });
  await Bun.write(`${OUT}/editor-sdk-completions.png`, Buffer.from(shot2.data, "base64"));
  const items = await js(`return [...document.querySelectorAll(".suggest-widget .monaco-list-row")].map((r) => r.getAttribute("aria-label")).slice(0, 12);`);
  console.log("completions:", JSON.stringify(items));
  // The mistakes docs/sdk.md lists, in a model of their own: the worker's code and line for each, to
  // compare with `tsc` 7 on the same file (`tsc -p <project>` with it under scripts/).
  const MISTAKES = await Bun.file(new URL("./sdk_mistakes.ts.txt", import.meta.url)).text();
  const found = await js(`
    const m = await import("/src/panels/scripts/monaco.ts");
    const model = m.monaco.editor.createModel(${JSON.stringify(MISTAKES)}, "typescript", m.uriOf("scripts/mistakes.ts"));
    let ms = [];
    for (let i = 0; i < 60; i++) {
      await new Promise((r) => setTimeout(r, 200));
      ms = m.monaco.editor.getModelMarkers({ resource: model.uri }).filter((x) => x.owner === "typescript");
      if (ms.length >= 15) break;
    }
    model.dispose();
    return ms.map((x) => \`\${x.startLineNumber}:\${x.startColumn} TS\${typeof x.code === "object" ? x.code.value : x.code}\`).sort();`);
  console.log("mistakes (worker):", JSON.stringify(found));
} finally {
  cdp.close();
  stop();
}
