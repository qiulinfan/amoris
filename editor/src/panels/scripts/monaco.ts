// Monaco, bundled (no CDN): the editor core with its features, TypeScript's syntax and language
// service in a worker, the `pocket` module's declarations, and a theme matching the editor.
// Loaded lazily with the Scripts panel.

import * as monaco from "monaco-editor/editor";
import "monaco-editor/features/register.all";
import "monaco-editor/languages/definitions/typescript/register";
import * as ts from "monaco-editor/languages/features/typescript/register";
import EditorWorker from "monaco-editor/editor/editor.worker?worker";
import TsWorker from "monaco-editor/languages/features/typescript/ts.worker?worker";
import stub from "../../../sdk-stub/pocket.d.ts?raw";

self.MonacoEnvironment = {
  getWorker(_id: string, label: string) {
    return label === "typescript" || label === "javascript" ? new TsWorker() : new EditorWorker();
  },
};

// The SDK's generated declarations when sdk/ has them (bundled at build time), else the stub.
const sdk = import.meta.glob("../../../../sdk/**/*.d.ts", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

ts.typescriptDefaults.setCompilerOptions({
  target: ts.ScriptTarget.ESNext,
  module: ts.ModuleKind.ESNext,
  moduleResolution: ts.ModuleResolutionKind.NodeJs,
  lib: ["es2022"],
  strict: true,
  noEmit: true,
  allowNonTsExtensions: true,
  noUnusedLocals: true,
});
ts.typescriptDefaults.setDiagnosticsOptions({ noSemanticValidation: false, noSyntaxValidation: false });
ts.typescriptDefaults.setEagerModelSync(true);

export const sdkSource: string = (() => {
  const entries = Object.entries(sdk);
  if (entries.length === 0) {
    ts.typescriptDefaults.addExtraLib(stub, "file:///node_modules/@types/pocket/index.d.ts");
    return "stub (editor/sdk-stub/pocket.d.ts)";
  }
  for (const [path, text] of entries) {
    const rel = path.replace(/^.*\/sdk\//, "");
    ts.typescriptDefaults.addExtraLib(text, `file:///node_modules/pocket/${rel}`);
  }
  if (!entries.some(([p]) => /\/index\.d\.ts$/.test(p))) {
    ts.typescriptDefaults.addExtraLib(stub, "file:///node_modules/@types/pocket/index.d.ts");
  }
  return `sdk/ (${entries.length} declaration files)`;
})();

monaco.editor.defineTheme("pocket-dark", {
  base: "vs-dark",
  inherit: true,
  rules: [
    { token: "comment", foreground: "6b7385", fontStyle: "italic" },
    { token: "keyword", foreground: "c792ea" },
    { token: "string", foreground: "a5d6a7" },
    { token: "number", foreground: "f78c6c" },
    { token: "type", foreground: "82aaff" },
    { token: "identifier", foreground: "d6dbe4" },
    { token: "delimiter", foreground: "8f98a8" },
  ],
  colors: {
    "editor.background": "#14171c",
    "editor.foreground": "#d6dbe4",
    "editorLineNumber.foreground": "#4a5262",
    "editorLineNumber.activeForeground": "#aab2c0",
    "editor.lineHighlightBackground": "#1b1f26",
    "editor.lineHighlightBorder": "#00000000",
    "editor.selectionBackground": "#2c4a7a",
    "editor.inactiveSelectionBackground": "#26344d",
    "editorGutter.background": "#14171c",
    "editorIndentGuide.background1": "#232832",
    "editorIndentGuide.activeBackground1": "#3a4252",
    "editorWidget.background": "#1a1e25",
    "editorWidget.border": "#2b313c",
    "editorSuggestWidget.background": "#1a1e25",
    "editorSuggestWidget.border": "#2b313c",
    "editorSuggestWidget.selectedBackground": "#26344d",
    "editorHoverWidget.background": "#1a1e25",
    "editorHoverWidget.border": "#2b313c",
    "scrollbarSlider.background": "#ffffff14",
    "scrollbarSlider.hoverBackground": "#ffffff22",
    "editorError.foreground": "#f0605d",
    "editorWarning.foreground": "#e5a53a",
    "editorInfo.foreground": "#59a6ff",
    "editorOverviewRuler.border": "#00000000",
    focusBorder: "#4f8cff66",
  },
});

export { monaco };

/** The model URI of a script path (`scripts/main.ts` → file:///project/scripts/main.ts). */
export function uriOf(path: string) {
  return monaco.Uri.parse(`file:///project/${path}`);
}

export function pathOf(uri: { path: string }): string {
  return uri.path.replace(/^\/project\//, "");
}
