# Game UI

- Status: built 2026-11-12, track `ui`. Charter 3 (principle 2, its 2026-11-12 note: the screen's
  UI is what every player sees), 4.4 (the game UI is drawn over the final image with unjittered
  matrices, at the output resolution).
- Sources: the renderer's text and bars (`crates/amoris-render/src/ui.rs`, DejaVu Sans Bold
  embedded) as they stood at `321bb008`; this file adds images, buttons, stacks, project fonts and
  pointer input to them.

The game's UI is world state. Its elements are components on entities, persisted, hashed, forked
and replayed like every other; the renderer lays them out and draws them each frame from the render
feed, and the hosts (the native window, the browser player) turn the pointer into presses that reach
the world through a recorded write, so a recording replays the same presses. Agents read the UI and
press its buttons through the same write.

## 1. What exists

| Piece | Where |
|---|---|
| The components `UiText`, `UiBar`, `UiImage`, `UiButton`, `UiStack` | `amoris-assets/src/visual.rs` |
| Their script fields and scene forms | `amoris-runtime/src/engine.rs` |
| Their extraction into the render feed (`UiView`) | `amoris-runtime/src/present.rs`, `amoris-assets/src/frame.rs` |
| `ui.press` (a write) and `ui.list` (a read) | `amoris-runtime/src/ui.rs`, `catalog.rs`, `game.rs` |
| Layout, drawing, fonts, images, hit testing | `amoris-render/src/ui.rs`, `shaders/ui.wgsl` |
| Project fonts and images through the asset source | `amoris-render/src/loader.rs`, `amoris-viewport/src/lib.rs` |
| The pointer in the native window | `amoris-render/src/app.rs`, `amoris-app/src/window.rs` |
| The pointer in the browser | `amoris-viewport/src/web.rs` (`ui_hit`, `ui_pointer`), `web/player.js` |
| The `player` tool's `ui` and `press`, `amoris player ui` and `press` | `amoris-mcp/src/tools.rs`, `amoris-app/src/client.rs` |
| A CJK font with its licence and source, and a scene that uses everything | `tests/fixtures/scenes/ui-hud/` |

## 2. Elements

Every element has an `anchor` (`top_left` ... `bottom_right`, or `entity`), an `offset` in logical
pixels from it, a `world_offset` in metres for `anchor: entity`, and `visible`. Colours are sRGB
with alpha. A logical pixel is the window's scale factor in device pixels.

| Component | Fields | Draws |
|---|---|---|
| `UiText` (version 2) | `text`, `size` (a line's height), `color`, `font` | one line of text with a one-pixel shadow |
| `UiBar` | `value`, `max`, `size`, `color`, `back` | a filled bar on its background |
| `UiImage` | `image`, `tint`, `size` | a project image (PNG or JPEG), multiplied by `tint` |
| `UiButton` | `size`, `color`, `hover_color`, `press_color`, `enabled`, `toggle`, `on`, `presses` | a box that the pointer and `ui.press` press |
| `UiStack` | `direction`, `spacing`, `padding`, `align`, `color`, `min_size`, `reference_height` | a box that lays out its children in a row or a column |

- `UiText.font` names a TrueType or OpenType file of the project (`fonts/DroidSansFallback.ttf`);
  empty is the engine's DejaVu Sans Bold. Version 2 of `UiText` added it, empty by default.
- `UiImage.size`: logical pixels; a side of 0 takes the image's own pixel count on that side, both 0
  the image's pixel size, one 0 keeps the image's aspect.
- `UiButton`: `color` is its face, `hover_color` while the pointer is over it, `press_color` while
  the pointer holds it and while a toggle is `on`; a disabled button draws its face at 45% of its
  alpha and is never hovered, held or pressed. `presses` counts its presses; `ui.press` writes it.
- `UiStack`: `direction` `column` (default) or `row`; `align` places children across it (`start`,
  `center`, `end`); `color` is its background (transparent by default); `min_size` its least size.

## 3. Layout

The renderer lays the UI out each frame in device pixels, from the feed and the window's size.

- **Nodes.** All the elements of one entity form its node. An entity with a `UiButton` or a
  `UiStack`, or whose `Parent` is an entity with a `UiStack`, is a laid-out node; the others are
  drawn element by element at their own anchors, as before this file (their frames are unchanged).
- **A node's box.** A stack's box holds its children (below); any other node's box is as wide and as
  tall as its widest and tallest element: the button's `size`, the image's, the bar's, the text's
  measured width and `size`. Every element of the node is centred in the box, so a button's
  `UiText` is its label and its `UiImage` its icon, whatever their own anchors and offsets say.
- **Stacks.** A stack's children are the laid-out nodes whose `Parent` is the stack, in `EntityId`
  order (the order a scene spawns them in). A column places them top to bottom, a row left to
  right, `spacing` apart, inside `padding` on every side; its box is at least `min_size`. Stacks
  nest: a child stack is laid out as a box of its own size. An element's anchor and offset are
  ignored inside a stack.
- **Roots.** A laid-out node that is not a stack's child is a root: its box is placed at its anchor
  and offset as a bar is (offsets point inward from the right and bottom edges), and
  `anchor: entity` centres it above the entity's projected `Transform` plus `world_offset`. The
  anchor, offset and `world_offset` a root uses are those of its `UiStack`, else its `UiButton`,
  else its first element's.
- **Scaling with the window.** Anchors keep a root at its corner, edge or centre at any window
  size. A root stack whose `reference_height` is above 0 is also scaled, with everything in it, by
  the window's height in device pixels over `reference_height`: a HUD laid out for 720 rows fills
  the same share of a 1440-row window. Otherwise the scale is the window's scale factor.
- **Visibility.** An invisible element is not drawn; an invisible stack hides everything under it
  (extraction leaves those elements out, so they are neither drawn nor pressed).

## 4. Drawing

The UI pass draws after the overlays, over the final image, at the output resolution, in one
render pass and one pipeline (`ui.wgsl`): textured, coloured quads whose vertices carry a mode,
glyph and solid quads sampling the R8 glyph atlas's coverage, image quads their own RGBA texture
(an `Rgba8UnormSrgb` texture per image, bound as group 1, drawn in runs of the same texture).
Elements outside laid-out nodes draw in the feed's order, as they did; a laid-out root draws its
own elements in the feed's order, which within one entity is stack (its background), button (its
face), image, text and bar, then its children. In the browser the canvas is not sRGB-encoded and
the pass blends sRGB values, as the text and bars always did there, so translucent faces and
backgrounds come out darker than natively, where blending is linear.

The fragment stage reads one uniform buffer and two textures, no storage buffers, inside the WebGPU
baseline (webgpu-baseline.md).

## 5. Fonts

- The engine's font is DejaVu Sans Bold, embedded (`crates/amoris-render/fonts/`). A text without
  `font` draws exactly as it did: the same glyphs, positions and baseline (`0.8` of the size below
  the top).
- A project font is loaded on demand through the renderer's asset source (`request_ui`, read and
  parsed on the loader's thread natively; fetched by the page and parsed in the viewport in the
  browser) and kept by path. Until it arrives its texts are not drawn and count as pending assets
  (`FrameStats.pending_assets`, which a capture waits for); a font that fails to load is reported
  once and its texts fall back to the engine's font. Its baseline is its ascent's share of the
  ascent and descent below the top.
- Glyphs of every font are rasterized when first drawn (fontdue) into one shared 1024 x 1024 atlas,
  keyed by font, character and pixel size; no font is baked ahead. When the atlas is full, the
  glyphs that did not fit are left out of that frame and the atlas is emptied for the next, which
  rasterizes what it draws again.
- A CJK font: `tests/fixtures/scenes/ui-hud/fonts/DroidSansFallback.ttf`, Droid Sans Fallback from
  the Android Open Source Project, Apache License 2.0, with `LICENSE.txt` and `SOURCE.md` beside it.
  It covers the CJK unified ideographs, kana and Hangul (51,522 glyphs; 3.9 MB; parsed in 78 ms
  natively on the M5). A game that writes Chinese, Japanese or Korean copies it, with its licence
  and source, into its own folder (games share no files) and names it in `UiText.font`.
- Not done: shaping (complex scripts, ligatures), kerning, right-to-left text, line wrapping.

## 6. Pointer and presses

Hit testing is the presenter's: only it knows the window's size and the measured text. The world
receives presses alone.

- **Hover and hold are presentation.** The renderer is told where the pointer is and whether its
  primary button is down (`Renderer::set_ui_pointer`); the topmost enabled button under it draws
  its `hover_color`, or `press_color` while held. Hover reaches neither the world nor the scripts.
- **A press is a write.** A primary-button release over the same enabled button it went down on is
  a press: the host sends `ui.press {entity}` (7). The window and the browser player send it as the
  local player (`Source::Player(0)`), the input source of their other player input; its recorded
  form is replayed like `player.act`, so a replay presses the same buttons at the same boundaries.
- **The native window.** While the mouse is not captured (before the first click, after Escape)
  the cursor is the UI's: hover shows, a press on a button presses it and does not capture, and a
  click elsewhere captures the mouse as before. A captured mouse never presses UI.
- **The browser player.** While the pointer is not locked, a click whose point is over a button
  (through the page's translucent veil too) presses it and does not ask for the lock; in the
  keyboard mode that embedded browsers fall back to (the pointer free, drag to look), a stationary
  click on a button presses it instead of being the primary action. The viewport answers
  `ui_hit(x, y)` (the entity under a canvas pixel, 0 for none) and takes `ui_pointer(x, y, over,
  down)`.
- **Agents.** A seat reads the screen's UI with `ui.list` and presses with `ui.press`; the `player`
  tool's actions `ui` and `press` and `amoris player ui` and `amoris player press <entity>` route to
  them. A game with players lets its seats send these two besides the `player.*` tools: what is on
  the screen every player sees (charter 3, principle 2, the 2026-11-12 note), and a press changes
  only what any player could press.
- Not done: keyboard or gamepad focus and navigation between buttons, per-seat UI, presses in the
  editor's viewport. The player page is the first-person seat's: for a game without a walker its
  veil stays up (clicks still reach the buttons under it).

## 7. Commands

`ui.press {entity}` (Write). `entity` is an id, a name or `Name#id`. Refused, changing nothing:
`ui.not_a_button` (no `UiButton`), `ui.disabled` (`enabled` false), `ui.hidden` (it or a stack
above it is invisible). Applied at the boundary: `presses` is incremented (saturating), a toggle's
`on` flips, and `ui.pressed {on, presses}` is emitted with the button as its subject, which the
scripts see in the next tick's `ctx.events` (`events: ["ui."]`). Answers `{entity, on, presses}`.
Recorded in its canonical form `{"entity": <id>}`.

`ui.list {}` (Read): the visible elements in `EntityId` order, each `{entity, kind, parent?, name?}`
with its kind's fields: a text's `text`, a bar's `value` and `max`, an image's `image`, a button's
`label` (its `UiText`, not listed again as a text), `enabled`, `toggle`, `on` and `presses`, a
stack's `direction`. At most 256 elements; `omitted` counts the rest.

`ui.` is an engine event prefix: scripts may not emit `ui.*` (`script.event_reserved`).

## 8. The render feed

`UiView` carries one element: `kind` (`Text`, `Bar`, `Image`, `Button`, `Stack`), `text`, `asset` (a
text's font, an image's file), `fill`, `size`, `color`, `back`, `hover`, `press`, `anchor`,
`offset`, `world`, `parent` (the entity's `Parent`, 0 for none), a button's `enabled` and `on`, a
stack's `column`, `align`, `spacing`, `padding` and `reference_height`. The frame's `ui` list is
sent whole when any UI component, a UI entity's `Transform` or its `Parent` changed. The change to
`UiView` changed the feed's `FORMAT`; the WebGPU viewport was rebuilt with it.

## 9. Tests and checks

- Renderer (`cargo test -p amoris-render --test ui`): an image at its anchor with its tint and at
  its own size; a column stack of buttons placed by its padding and spacing, with their hit
  rectangles, and a row aligned to its end; each button state's colour (face, hover, held, on,
  disabled); a CJK line in the fixture's font against the engine font's other glyphs, and a text
  whose font is on the way left out and counted pending; a `reference_height` stack at two window
  heights.
- The engine font's frames: the `ui_frames` example draws texts at every anchor, over entities and
  beside bars, at scales 1 and 2, from JSON that both forms of `UiView` read; built in this tree and
  in an export of `321bb008` (before this file), its PNGs are byte for byte the same (2026-11-12,
  Metal on the M5).
- Runtime (`cargo test -p amoris-runtime --features transpile --test ui`): the extraction of every
  element, a hidden stack's children left out; `ui.press` refusals changing nothing; presses, the
  toggle and the counter, the event in the rule's next tick; `ui.list` and a press as the seat of
  a game with players, which sends no developer command; the run's recording replays identical.
- `amoris check tests/fixtures/scenes/ui-hud`: its inputs press as the local player at ticks 10, 40
  and 60, through determinism, a fork, a replay and a reload.
- The captures (2026-11-12, M5, Metal and Chrome 156): `amoris serve tests/fixtures/scenes/ui-hud`
  with the Sound toggle pressed by `amoris player press Sound`, captured natively (`capture`) and
  in headless Chrome's WebGPU viewport (`/wasm/index.html?render=...`, `node tools/web_bench.mjs`)
  with the pointer over Settings: the same layout, `ui_hit` naming Start, Sound and Settings and
  none for the disabled Quit, a gap or the scene. The game exported with `cargo xtask export-web`
  in headless Chrome: synthesized clicks on Sound and then Start, through the veil, reached the
  worker as two `ui.press` and the rule's status line, and no pointer lock was asked for. Not run:
  Direct3D 12 and Vulkan, and a real mouse in a native window.
