# Pocket UI

The interface layer of the engine: what gameplay HUDs, menus and the editor are made of. It follows ADR 0004 (no Dear ImGui; a retained tree the agent can read) and is built so that an AI agent perceives and operates an interface with the same commands a script uses.

## Shape

```
TSX / TypeScript (sdk/runtime/ui.ts, jsx-runtime.ts)
   signals -> render() -> virtual tree -> diff -> ui.apply ops
                                                     |
engine/ui (C++)                                      v
   Document: elements (box, text, input) ---- Yoga flexbox layout ---- hit testing, focus, text input
   Painter: batched 2D quads (rounded rects, borders, glyphs) over the frame's color target
   Font: FreeType glyphs in an R8 atlas, Noto Sans CJK by default
```

- **Elements** are `box`, `text` and `input`. Every element has flexbox style (width/height/min/max in points, percent or auto; flex, direction, wrap, justify, align, margin, padding, gap, absolute position, overflow hidden/scroll, display none) and paint style (background, border, radius, opacity, color, fontSize, textAlign, textWrap). Colors are `#rgb`, `#rrggbb`, `#rrggbbaa` or `[r, g, b, a]` in 0..1. An element with `anchor` (an entity id) sits on that entity's projection at every layout: `anchorAlign` says which point of the element goes there (0..1 across and down; the default `[0.5, 1]` stands the element over the point), `anchorOffset` shifts it in points, and it is hidden while the entity is behind the camera or gone, so a name tag, a damage number or a speech bubble is a `Label` with `anchor` and no per-frame script (`ui.describe` reports the three; the element is placed in its parent's coordinates, so it goes under the root or a full-window layer).
- **Images** are a style on any box: `image` names a project-relative picture (PNG, JPG), drawn over the background and under the children, `fit` says how it meets the box (`contain`, the default: inside it with its proportions kept and centered; `cover`: filling it with the excess cropped; `fill`: stretched), `uv` picks the part shown (u0, v0, u1, v1 in 0..1, v down) for icons on a sheet, `radius` rounds its corners, and `slice` (left, top, right, bottom in picture pixels, or one number) draws it in nine slices for frames and panels: the corners keep their pixel size, the edges stretch along, the middle fills the box (`fit` is then ignored). `filter: "nearest"` samples it by the nearest texel, so pixel art scales into crisp blocks (linear, the default, scales soft). The renderer uploads and caches the texture the way it does a sprite's, the painter draws it as one more quad in the same pass, and a missing file draws nothing and is reported once. `ui.describe` reports `image`, `fit` and a `filter` of `nearest`; the editor's asset list shows images as thumbnails this way.
- **Layout defaults** are React Native's: column direction, no shrinking, stretch on the cross axis. Content that does not fit overflows or scrolls; it never collapses to zero height.
- **Text** wraps by words for Latin and per character for CJK. Measurement goes through Yoga's measure callback, so text sizes itself and containers size around it.
- **Input** elements own a caret, accept OS text input (composition works through SDL), Backspace/Delete/Left/Right/Home/End, Return (commits a `change`) and Escape (blurs). With `multiline` an input is a text area: Return puts in a new line (Cmd/Ctrl+Return commits); with `textWrap` (`wrap` on the SDK's `TextInput`) its lines wrap at the area's width, words kept whole (CJK by character) and a word wider than the area broken where it must be; Up and Down move to the nearest position on the row above or below, Home and End move along the row; the rows draw from the top, scroll with the wheel (a thumb on the right when they overflow) and a caret that moved comes into view; `ui.describe` reports `rows`, `wrap` and `scrollTop`; and the measured height follows the row count when no height is set; `ui.snapshot` shows its newlines as `\n`. Every input selects: Shift with the arrows, Home and End extends a selection from the caret, dragging and Shift+click select with the mouse, a double-click selects the word under the pointer (letters, digits and underscores together, a CJK character on its own) and a triple-click the line (`ui.click {clicks: 2}` and `3` do the same), Cmd/Ctrl+A selects all, Cmd/Ctrl+C, X and V copy, cut and paste (through the OS clipboard when a window exists, otherwise the document's own, so headless runs and agents cut and paste deterministically; a single line pastes newlines as spaces), typing, Backspace and Delete replace or remove the selection, and `ui.describe` reports `selection` as byte offsets. Focus follows clicks, and Tab walks it through the inputs and the elements that listen for clicks in tree order (Shift+Tab back, wrapping at the ends, disabled and hidden ones skipped), drawing a ring around a focused element that is not an input; Return or Space on a focused clickable element is a `click` at its center with `keyboard: true`, so a whole interface works from the keyboard. The runtime tells the platform when text input is wanted and where the caret is, so an IME's candidate window opens beside the text being written.
- **Events** exist only for elements that declared listeners (`on: ["click", ...]`, or JSX `onClick`). Click, mousedown/up, input, change, keydown, wheel, hover, focus/blur, drag/dragend. A click bubbles to the nearest listening ancestor; a press that moves becomes a drag, not a click.
- **Input events that landed on the interface** reach gameplay tagged with `ui: <node id>` so a game can ignore them.
- **Scrolling** is on any element with `overflow: "scroll"`; the wheel scrolls the nearest scrollable ancestor and a thumb is drawn.
- **Painting** happens after the scene in the same frame, in one extra render pass that loads the color target, so captures and the state hash contract are unchanged (the interface is presentation; it is not part of the state hash).

## Commands

Everything is a command, so scripts, the HTTP server, `pocket mcp` and tests share it:

| Command | Purpose |
|---|---|
| `ui.apply {ops}` | Mutate the tree: `["create", id, type]`, `["set", id, props]`, `["append", parent, id, index?]`, `["remove", id]`, `["text", id, str]`, `["clear", id]`, `["focus", id]`. Ids are chosen by the caller (the SDK allocates them). |
| `ui.snapshot {root?, depth?, max_nodes?, layout?, styles?}` | The interface as text, one line per element with type, id, name, rectangle, text or value, listeners, focus and scroll state. This is what an agent reads instead of pixels. |
| `ui.query {type?, text?, name?}` | Find elements by type, text substring or name. |
| `ui.describe {id}`, `ui.hit {x, y}` | One element in full; the element under a point. |
| `ui.click {id | x, y, clicks?}`, `ui.type {text}`, `ui.key {key}`, `ui.wheel {id | x, y, dy}` | Synthetic input through the same path real input takes; returns the UI events it produced. `clicks: 2` is a double-click (in an input it selects the word), `3` a triple (the line). |
| `ui.focus {id}`, `ui.stats` | Focus management; node/paint counters. |
| `render.viewport {x, y, w, h}` | Confine the scene to a rectangle in points (the editor's scene pane); `{reset: true}` restores the full frame. |

Elements carry an optional `name` that appears in snapshots and queries, so a script can say `<Button name="play" .../>` and an agent can `ui.query {name: "play"}` then `ui.click {id}`.

## TypeScript

`pocket ts` compiles JSX with the automatic runtime and `pocket/jsx-runtime` as the import source; no React. The SDK provides:

- `signal(value)`: a reactive value (`count()`, `count.set(1)`, `count.update(f)`). Setting one marks every mounted tree for re-render at the end of the frame (or immediately after a UI event).
- `mount(() => <App/>)`: renders now and after changes; the reconciler diffs the expanded tree (components are plain functions of props, fragments flatten) and emits only the operations that changed, keyed by `key` or index.
- Components: `Button`, `Label`, `Panel`, `TextInput`, `Row`, `Column`, a `theme`, and the intrinsic `box`, `text`, `input`.
- `ui.*`: the commands above as typed functions, including `ui.snapshot()` so a script can read its own interface.

Handlers registered by a bundle live in a per-context registry on `globalThis` (`__pocket_registry`), keyed by the `__pocket_bundle` name the runtime sets before evaluating a bundle. That is what lets the editor and a project share one JavaScriptCore context: `script.reload {name: "project"}` drops the project's handlers and interface, evaluates the bundle again and dispatches `start` to it alone.

## Verification

- `tests/ui_tests`: UTF-8 decoding, font metrics, painter pixels, flex layout numbers, wrapping (Latin and CJK), hit testing through clipping and hidden elements, click and drag semantics, focus and text editing with multi-byte characters, scroll clamping, snapshots, queries, removal, paint pixels.
- `tests/ts/ui.test.tsx`: JSX mounting, synthetic clicks reaching handlers, minimal diffs (node counts), typing into inputs, hit testing.
- `tests/evidence/ui/`: captures and a JSON-RPC session transcript from `samples/ui`.

## Not yet

Animations, syntax colouring in a text area, and golden-image comparison in CI (`render.compare` compares whole frames). Each of these slots into the element model without changing the commands.

## Text

Text is shaped by HarfBuzz with the font's own OpenType tables (contextual forms, ligatures, kerning, mark placement; right-to-left runs come out in visual order) and rasterized by FreeType into one glyph atlas keyed by glyph index. `Font::shape` returns positioned glyphs with their source byte offsets, which is what the painter draws and what text inputs use to place the caret; `Font::measure` is the run's advance, cached per string and size because layout asks for it often. One font per runtime for now: a project that needs Arabic, Devanagari or Thai sets `font` in `project.toml` to a face that has them (the Arabic test uses Noto Sans Arabic).
