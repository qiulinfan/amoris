// Monaco, bundled (no CDN): the editor core with its features, TypeScript's syntax and language
// service in a worker, the `pocket` module's declarations, and a theme matching the editor.
// Loaded lazily with the Scripts panel.

import * as monaco from "monaco-editor/editor";
import "monaco-editor/features/register.all";
import "monaco-editor/languages/definitions/typescript/register";
import * as ts from "monaco-editor/languages/features/typescript/register";
import EditorWorker from "monaco-editor/editor/editor.worker?worker";
import TsWorker from "monaco-editor/languages/features/typescript/ts.worker?worker";
// The SDK as the engine embeds it (generated from the prelude by crates/pocket-script/tests/prelude.rs),
// for when the host has not sent the project's declarations yet.
import bundledSdk from "../../../../crates/pocket-script/src/prelude/pocket.d.ts?raw";
import type { ScriptTypes } from "../../host/protocol";
import { useScripts } from "../../state/scripts";

self.MonacoEnvironment = {
  getWorker(_id: string, label: string) {
    return label === "typescript" || label === "javascript" ? new TsWorker() : new EditorWorker();
  },
};

// The options of the tsconfig.json the engine writes into a project (pocket-script's types::TSCONFIG),
// so the worker reports what `tsc` reports in scripts.check.
ts.typescriptDefaults.setCompilerOptions({
  target: ts.ScriptTarget.ESNext,
  module: ts.ModuleKind.ESNext,
  moduleResolution: ts.ModuleResolutionKind.NodeJs,
  lib: ["es2023"],
  strict: true,
  noEmit: true,
  isolatedModules: true,
  allowImportingTsExtensions: true,
  allowNonTsExtensions: true,
});
ts.typescriptDefaults.setDiagnosticsOptions({ noSemanticValidation: false, noSyntaxValidation: false });
ts.typescriptDefaults.setEagerModelSync(true);

/** Until the host's declarations arrive: any component name, loosely typed. */
const LOOSE_COMPONENTS = `export {};
declare module "pocket" {
  interface Components { [name: string]: any }
  interface ComponentColumns { [name: string]: any }
}
`;

let loaded: { dispose(): void }[] = [];

/** Loads `pocket`'s declarations into the TypeScript worker as `node_modules/pocket`. */
function loadSdk(pocket: string, components: string) {
  for (const lib of loaded) lib.dispose();
  loaded = [
    ts.typescriptDefaults.addExtraLib(pocket, "file:///node_modules/pocket/index.d.ts"),
    ts.typescriptDefaults.addExtraLib(components, "file:///node_modules/pocket/components.d.ts"),
  ];
}

/** Where the `pocket` module's types come from, for the status bar. */
export function sdkLabel(types: ScriptTypes | null): string {
  if (!types?.text) return "bundled (components untyped until the host answers scripts.types)";
  const c = types.components;
  return `.pocket/types (${c.engine.length} engine + ${c.project.length} game components)`;
}

function apply() {
  const text = useScripts.getState().types?.text;
  const pocket = text?.["pocket.d.ts"];
  const components = text?.["components.d.ts"];
  if (pocket && components) loadSdk(pocket, components);
  else loadSdk(bundledSdk, LOOSE_COMPONENTS);
}

apply();
useScripts.subscribe((s, prev) => {
  if (s.types !== prev.types) apply();
});

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
