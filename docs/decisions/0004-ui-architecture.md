# ADR 0004: UI architecture for the editor and the runtime

- Status: Proposed (2026-09-17)
- Deciders: repository owner (human), Claude (AI collaborator)
- Related: ADR 0001 (stack), ADR 0002 (graphics, pending), `docs/unity-lessons.md`

## Context

The editor needs a serious desktop UI: docking, menus, inspectors generated from metadata, text
editing with CJK input, theming. Games need a runtime UI. PocketEngine used Dear ImGui for the
editor and nothing for games. Unity's UI Toolkit showed the direction the industry converged on: one
retained-mode UI system for editor and runtime, shaped like the web (markup, stylesheets, flexbox,
event bubbling, data binding) and implemented natively. Unreal (Slate and UMG), Godot (Control
nodes) and Blender also build their editors on their own UI systems; none of them use Qt or a
browser.

Pocket3D adds two constraints: the scripting language is TypeScript, and AI agents must be able to
write, read and test UI.

## Options considered

1. **Dear ImGui.** Fastest for tools, but immediate-mode code is not declarative, there is no
   styling, layout or accessibility model, text and IME are rough, and it has no path to runtime UI.
   Unity's decade-long transition from IMGUI to UI Toolkit is the warning.
2. **Qt 6 (Widgets or QML).** Mature desktop polish, but LGPL and a hundred-megabyte dependency, its
   own code generator (moc) and its own JavaScript engine (QML), no runtime UI story for games, not
   TypeScript.
3. **Web UI in a webview or a bundled Chromium.** The strongest tooling and agent fluency, but a
   second render world with viewport-integration hacks, platform webview divergence or a 150 MB
   Chromium, a second JavaScript VM separated from the engine by IPC, and no runtime UI for games.
4. **Own retained-mode UI system with the web model, driven by TypeScript and rendered by the
   engine.** Unity UI Toolkit's shape, with TypeScript instead of C# and TSX instead of UXML.
   Chosen.

## Decision

Pocket3D has one UI system, "Pocket UI", used by the editor and by games.

- **Model.** A retained element tree with a DOM-like API: elements, attributes, classes, events with
  capture and bubbling, focus, hit testing. The tree is serializable to text (a snapshot that
  doubles as the accessibility tree) and queryable with CSS selectors.
- **Authoring.** TypeScript with TSX (oxc transforms it) and a fine-grained reactive core based on
  signals, in the style of Solid, rather than a virtual DOM: the engine owns the tree, so diffing is
  unnecessary. Components are plain TypeScript modules; no decorators, no markup files.
- **Layout.** Flexbox through Yoga (the layout engine Unity UI Toolkit and React Native use); grid
  later.
- **Styling.** A CSS subset in the spirit of Unity's USS: type, class, id and pseudo-class
  selectors, variables, transitions, themes as text files. Parsed and cascaded by the engine.
- **Rendering.** The engine's own 2D batch renderer on the RHI: rounded rectangles, borders and
  shadows as signed-distance fields, glyph atlases, images, clip rectangles. Vector paths and SVG
  icons through ThorVG (MIT, used by Godot) when needed.
- **Text.** FreeType and HarfBuzz for shaping (CJK, bidirectional text, emoji fallback); IME and
  clipboard through SDL3; accessibility through AccessKit (C bindings) at a later milestone.
- **Editor shell.** Docking, menus, dialogs, drag and drop are Pocket UI components written in
  TypeScript. The editor is a Pocket3D application; editor extensions are TypeScript packages using
  the same API as gameplay UI.
- **Testing.** UI tests query the tree, inject events and compare golden images headlessly; agents
  drive the editor through these APIs and through MCP.

## Consequences

- No Dear ImGui at any milestone. Before Pocket UI exists (M0 to M2) verification is headless:
  probes, captures and golden images.
- The UI system is a large milestone (M3) and the first big application built on the engine; it
  validates the script runtime, the RHI and the text stack.
- Web idioms carry over for humans and agents (TSX, CSS, DOM-like queries) without shipping a
  browser.
- Native OS integration is thin: SDL3 windows and events; the macOS menu bar needs a small native
  shim.

## Open points

The vector renderer when paths are needed (ThorVG or the Rive renderer), the accessibility
milestone, and the scope of rich text editing (a code editor panel may embed a dedicated component
later).
