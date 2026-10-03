// The Pocket editor: a TSX program on Pocket UI that runs beside a project in the same runtime.
// It sees the world through the same commands scripts and agents use, so everything shown here
// (hierarchy, inspector, console, transcript) is also reachable by `ui_snapshot`, and every
// button is reachable by `ui_click`. Every edit is undoable (editor/history.ts) and the scene
// pane has a translate gizmo (editor/gizmo.ts) that agents drag with `ui.drag`.
import { Checkbox, Choice, Label, Row, Slider, command, h, mount, onFrame, onInput, physics, render, setProjectRoot, signal, terrain, theme, tilemap, ui, world } from "pocket";
import type { Bus, ComponentName, Described, Dim, Edge, Scene, Transform, UiEvent, VNode, WorldEvent } from "pocket";
import { applyOrbit, orbitFromCamera } from "./orbit";
import type { Orbit } from "./orbit";
import * as history from "./history";
import { axisDelta, axisQuat, intoParent, layoutFor, multiplyQuat, planeDelta, turnIntoParent } from "./gizmo";
import type { ParentFrame } from "./gizmo";
import type { Axis, GizmoLayout } from "./gizmo";
import type { Vec3 } from "pocket";

// ------------------------------------------------------------------------------------ state
interface TreeRow { id: number; name: string; path: string; depth: number }
interface LogRow { seq: number; tick?: number; level: string; cat: string; msg: string }
interface SchemaField { name: string; type: string; doc: string; names?: string[] }
interface SchemaComponent { name: string; doc: string; serialized: boolean; fields: SchemaField[]; default?: Record<string, unknown> }
// The panes live in three docks (left, right, bottom), each showing one of its panes at a time
// behind tabs; a tab dragged onto another dock moves its pane there. The widths keep their first
// names: `hierarchy` is the left dock's, `inspector` the right's.
interface Layout { hierarchy: number; inspector: number; bottom: number; docks: Record<Dock, Pane[]>; active: Record<Dock, Pane | "">; }
interface AssetRow { path: string; kind: "mesh" | "image" | "tilemap" | "audio" | "script" | "material" | "other"; bytes: number; loaded: boolean; importer?: "gltf" | "obj" | "stl" | "ply" | "vox" | "voxels" | "blender"; builtin?: boolean }
// Meshes the engine makes, listed under the project's files so they are placed the same way
// (docs/design/assets.md, Props; docs/design/animation.md, A character without a file).
const BUILTIN_MESHES = ["humanoid", "tree", "pine", "rock", "bush", "barrel", "lamp", "fence", "house", "crate", "chest", "torch", "bench", "table", "chair", "well", "sign", "tower", "crop"];
const builtinRows: AssetRow[] = BUILTIN_MESHES.map((path) => ({ path, kind: "mesh", bytes: 0, loaded: false, builtin: true }));
type Tab = "console" | "events" | "transcript" | "assets" | "input" | "audio" | "script" | "timeline";
const TABS: Tab[] = ["console", "events", "transcript", "assets", "input", "audio", "script", "timeline"];
type Pane = "hierarchy" | "inspector" | Tab;
type Dock = "left" | "right" | "bottom";
const PANES: Pane[] = ["hierarchy", "inspector", ...TABS];
const DOCKS: Dock[] = ["left", "right", "bottom"];
const PANE_LABELS: Record<Pane, string> = { hierarchy: "Hierarchy", inspector: "Inspector", console: "Console", events: "Events", transcript: "Transcript", assets: "Assets", input: "Input", audio: "Audio", script: "Script", timeline: "Timeline" };
interface ActionBindings { positive?: string[]; negative?: string[]; axis?: string[]; deadzone?: number }
interface GizmoView { center: { x: number; y: number }; x: { x: number; y: number }; y: { x: number; y: number }; z: { x: number; y: number } }

const LAYOUT_PATH = ".pocket/editor.json";
const DEFAULT_LAYOUT: Layout = { hierarchy: 240, inspector: 340, bottom: 200, docks: { left: ["hierarchy"], right: ["inspector"], bottom: [...TABS] }, active: { left: "hierarchy", right: "inspector", bottom: "console" } };

const info = command<{ name: string; scene: string | null; contexts: string[]; window: { width: number; height: number } }>("project.info");
const schema = command<{ components: SchemaComponent[] }>("world.schema").components;

const selection = signal<number[]>([]);   // ordered; the last one is the primary selection
const playing = signal(false);
const paused = signal(true);
const overlays = signal(false);   // colliders, joints and lights drawn as lines in the scene pane
const snap = signal(false);       // gizmo drags land on the grid: snapStep units, 15 degrees, quarter scales
const localAxes = signal(false);  // gizmo handles on the entity's own axes instead of the world's
const snapStep = signal(0.5);     // the grid a snapped move lands on, cycled by the toolbar
const SNAP_STEPS = [0.1, 0.25, 0.5, 1, 2];
const SNAP_ANGLE = Math.PI / 12, SNAP_SCALE = 0.25;
const status = signal({ tick: 0, hash: "", entities: 0, frames: 0 });
const rows = signal<TreeRow[]>([]);
const described = signal<Described | null>(null);
const logs = signal<LogRow[]>([]);
const recentEvents = signal<WorldEvent[]>([]);
const transcriptText = signal("");
const notice = signal(`Editing ${info.name}${info.scene ? ` (${info.scene})` : ""}. Press Play to start the project's scripts.`);
const addingComponent = signal(false);
const layout = signal<Layout>({ ...DEFAULT_LAYOUT });
const gizmo = signal<GizmoView | null>(null);
const historyVersion = signal(0);
const brush = signal<{ layer: string; gid: number } | null>(null);   // tile painting in the scene pane while a TileMap is selected
type SculptMode = "raise" | "lower" | "flatten" | "smooth" | "paint" | "erase";
type Rgb = { r: number; g: number; b: number };
// A paint brush lays a colour, or with `layer` one of the terrain's textured layers (docs/design/terrain.md, Layers).
type SculptSettings = { mode: SculptMode; radius: number; strength: number; color: Rgb; layer: number | null };
const sculpt = signal<SculptSettings | null>(null);   // terrain sculpting (or painting) in the scene pane while a Terrain is selected
const sculptSettings = signal<SculptSettings>({ mode: "raise", radius: 4, strength: 0.5, color: { r: 0.45, g: 0.35, b: 0.24 }, layer: null });
// Colours a terrain brush offers (sRGB): a dirt path, sand, dark grass, stone, a burnt patch.
const PAINTS: Array<[string, Rgb]> = [["dirt", { r: 0.45, g: 0.35, b: 0.24 }], ["sand", { r: 0.82, g: 0.74, b: 0.55 }], ["moss", { r: 0.2, g: 0.33, b: 0.15 }], ["stone", { r: 0.55, g: 0.55, b: 0.55 }], ["ash", { r: 0.12, g: 0.11, b: 0.1 }]];
const actions = signal<Record<string, ActionBindings>>({});   // the input map, shown and edited by the Input tab
const buses = signal<Bus[]>([]);   // the Audio tab's buses, as audio.buses answers
interface TypeError { file?: string; line?: number; column?: number; severity: string; message: string }
const typeErrors = signal<TypeError[]>([]);   // the project's type errors, from `pocket editor --watch` (script.diagnostics)
const master = signal<{ master_volume: number; muted: boolean }>({ master_volume: 1, muted: false });
const newAction = signal("");    // the Input tab's new-action name and keys, until Add
const newKeys = signal("");
const capture = signal<{ name: string; part: "positive" | "negative" | "axis" } | null>(null);   // the Input tab's Press: the next key or pad button rebinds this part
const assetRows = signal<AssetRow[]>([]);      // the project's assets/ folder, shown by the Assets tab
const assetPick = signal("");                  // the asset whose description is shown
const assetInfo = signal("");
const scriptPath = signal("");                 // the script open in the Script tab's text area
const scriptText = signal("");
const scriptDirty = signal(false);

let editScene: unknown = null;
let orbit: Orbit | undefined;
let lastViewport = "";
let viewportRect = { x: 0, y: 0, w: 0, h: 0 };
let mainRect = { x: 0, y: 0, w: 0, h: 0 };
let frame = 0;
let dragState: { ids: number[]; before: Map<number, Transform>; parents: Map<number, ParentFrame | undefined>; layout: GizmoLayout; axis: Axis; turned: number; scaled: number; moved: Vec3 } | null = null;
let stroke: { entity: number; layer: string; pos: { x: number; y: number }; cells: Map<string, { tile_x: number; tile_y: number; was: number; gid: number }> } | null = null;
let sculptStroke: { entity: number; paint: boolean; layers: boolean; before: number[]; pos: { x: number; y: number }; target?: number; touches: number } | null = null;

// ------------------------------------------------------------------------------------ look
// One palette, one spacing scale and the few pieces every pane is built from (docs/editor.md,
// Look). Surfaces get lighter from the window behind the panes to a pane, a section header, a
// button and a hovered one; inputs sit darker than the pane they are in; one accent marks the
// selection, the focus and the primary action.
const C = {
    window: "#121317",       // behind the panes: the toolbar, the status bar, the splitters
    strip: "#17181d",        // tab strips
    pane: "#1e2026",         // a pane's body (an active tab takes it, so it joins its pane)
    section: "#272a31",      // a section's header in the inspector
    raised: "#2d3039",       // a button
    hover: "#393d48",        // a button under the pointer
    rowHover: "#ffffff0d",   // a row or tab under the pointer
    field: "#141519",        // inputs, sunken into the pane
    fieldLine: "#30333c",
    line: "#2b2d35",         // separators, tree guides
    text: "#e3e5ea",
    dim: "#a1a7b3",
    faint: "#6c727e",
    accent: "#4f8cff",
    accentHover: "#6a9eff",
    accentSoft: "#4f8cff33", // a toggle that is on
    accentText: "#a9c7ff",
    selected: "#294370",     // the primary selection's row
    selectedSoft: "#22324f", // the rest of the selection
    danger: "#e5484d",
    ok: "#3dbf6d",
    warn: "#e8b04a",
};
// Axis letters and colour channels in the axis colours.
const AXIS_COLORS: Record<string, string> = { x: "#e5534b", y: "#5cbf5c", z: "#4f8cff", r: "#e5534b", g: "#5cbf5c", b: "#4f8cff" };
const S = { xs: 2, sm: 4, md: 6, lg: 8, xl: 12 };   // the spacing scale, in points
const SIZE = { small: 11, body: 12, title: 13 };    // font sizes
const ROW = 22;          // a hierarchy, list or field row
const STRIP = 24;        // a tab strip (editor tests click the 15th hierarchy row at 1024x640: keep the strip and rows this low)
const GAP = 4;           // between panes: the splitters
const LABEL_W = 88;      // the inspector's label column

// The SDK's own widgets (Choice, Slider, Checkbox) take the palette too; the project's interface,
// a bundle of its own, keeps the SDK's theme.
Object.assign(theme, { panel: C.pane, panelAlt: C.raised, border: C.fieldLine, text: C.text, muted: C.dim, accent: C.accent, danger: C.danger, ok: C.ok, fontSize: SIZE.body });

const hovered = signal("");        // the element under the pointer that shows it (a button, a row, a tab, a splitter)
const focusedInput = signal("");   // the input with the keyboard's focus, outlined in the accent

/** Hover handler: the element shows it while the pointer is over it. */
function hoverOn(key: string): (e: UiEvent) => void {
    return (e) => {
        if (e.entered) hovered.set(key);
        else if (hovered() === key) hovered.set("");
    };
}

/** The editor's button: raised by default, `primary` in the accent, `ghost` flat until hovered
 * (`quiet` dims its text until then), `on` for a toggle that is on; its name is its label unless
 * given, as the SDK's is. */
function Button(props: { label: string; onClick?: (e: UiEvent) => void; primary?: boolean; ghost?: boolean; quiet?: boolean; on?: boolean; disabled?: boolean; name?: string; small?: boolean; icon?: VNode | null }): VNode {
    const name = props.name ?? props.label;
    const hot = !props.disabled && hovered() === name;
    const bg = props.disabled ? (props.ghost ? null : "#ffffff08")
        : props.primary ? (hot ? C.accentHover : C.accent)
        : props.on ? (hot ? "#4f8cff4d" : C.accentSoft)
        : props.ghost ? (hot ? C.raised : null)
        : hot ? C.hover : C.raised;
    const fg = props.disabled ? C.faint : props.primary ? "#ffffff" : props.on ? C.accentText : props.quiet && !hot ? C.dim : C.text;
    return (
        <box name={name} direction="row" align="center" justify="center" gap={S.sm + 1} height={props.small ? 20 : 26} padding={[0, props.small ? S.md + 1 : S.lg + 2]} radius={4} background={bg}
            disabled={props.disabled} onClick={props.disabled ? undefined : props.onClick} onHover={hoverOn(name)}>
            {props.icon ?? null}
            <Label text={props.label} size={props.small ? SIZE.small : SIZE.body} color={fg} />
        </box>
    );
}

/** The editor's text input: sunken, outlined in the accent while it has the focus. `grow` shares a
 * row's room equally with its siblings (vector fields). */
function TextInput(props: { value: string; onChange?: (value: string) => void; onInput?: (value: string) => void; placeholder?: string; width?: Dim; height?: Dim; flex?: number; grow?: boolean; name?: string; disabled?: boolean; multiline?: boolean; wrap?: boolean; syntax?: string; size?: number; padding?: Edge }): VNode {
    const focused = props.name !== undefined && focusedInput() === props.name;
    return h("input", {
        name: props.name,
        value: props.value,
        placeholder: props.placeholder,
        width: props.width,
        height: props.height,
        flex: props.flex,
        ...(props.grow ? { flexGrow: 1, flexShrink: 1, flexBasis: 0, minWidth: 0 } : {}),
        multiline: props.multiline,
        textWrap: props.wrap,
        syntax: props.syntax,
        disabled: props.disabled,
        color: props.disabled ? C.faint : C.text,
        fontSize: props.size ?? SIZE.body,
        background: C.field,
        borderColor: focused ? C.accent : C.fieldLine,
        border: 1,
        radius: 4,
        padding: props.padding ?? [S.xs, S.md],
        onChange: props.onChange ? (e: UiEvent) => props.onChange?.(e.value ?? "") : undefined,
        onInput: props.onInput ? (e: UiEvent) => props.onInput?.(e.value ?? "") : undefined,
        onFocus: props.name !== undefined ? () => focusedInput.set(props.name!) : undefined,
        onBlur: props.name !== undefined ? () => { if (focusedInput() === props.name) focusedInput.set(""); } : undefined,
    });
}

/** A thin vertical rule between groups of a bar. */
function Sep(props: { height?: number }): VNode {
    return <box width={1} height={props.height ?? 18} background={C.line} margin={[0, S.sm]} />;
}

/** A section's header bar: its title, then whatever goes at its right (a button, a value). */
function SectionHeader(props: { title: string; children?: unknown }): VNode {
    return (
        <box direction="row" align="center" gap={S.md} height={24} padding={{ left: S.lg, right: S.xs }} radius={4} background={C.section}>
            <Label text={props.title} size={SIZE.body} weight="bold" />
            <box flex={1} />
            {props.children}
        </box>
    );
}

/** An inspector section: a header and its rows indented under it. */
function Section(props: { title: string; name?: string; right?: unknown; children?: unknown }): VNode {
    return (
        <box gap={S.sm} name={props.name}>
            <SectionHeader title={props.title}>{props.right}</SectionHeader>
            <box gap={3} padding={{ left: S.sm, right: S.xs }}>{props.children}</box>
        </box>
    );
}

/** A labelled row: the label in a column of its own, the fields filling the rest. */
function Prop(props: { label: string; children?: unknown }): VNode {
    return (
        <box direction="row" align="center" gap={S.md} minHeight={ROW}>
            <box width={LABEL_W} overflow="hidden"><Label text={props.label} muted /></box>
            <box direction="row" align="center" gap={S.sm} flexGrow={1} flexShrink={1} flexBasis={0}>{props.children}</box>
        </box>
    );
}

/** What an empty pane says: what is missing and how to get it. */
function Empty(props: { title: string; hint?: string }): VNode {
    return (
        <box flexGrow={1} align="center" justify="center" gap={S.sm} padding={[S.xl * 2, S.xl]}>
            <Label text={props.title} muted size={SIZE.title} />
            {props.hint ? <Label text={props.hint} size={SIZE.body} color={C.faint} wrap align="center" /> : null}
        </box>
    );
}

/** JSON copy (structuredClone is not in the script host). */
function clone<T>(v: T): T {
    return JSON.parse(JSON.stringify(v)) as T;
}

function selected(): number {
    const s = selection();
    return s.length > 0 ? s[s.length - 1] : 0;
}

function alive(id: number): boolean {
    try {
        world.describe(id);
        return true;
    } catch {
        return false;
    }
}

// ------------------------------------------------------------------------------------ data
function refreshHierarchy(): void {
    const out: TreeRow[] = [];
    for (const r of world.query({ fields: [] })) {
        const parts = r.path.split("/").filter((p) => p.length > 0);
        out.push({ id: r.id, name: parts[parts.length - 1] ?? String(r.id), path: r.path, depth: parts.length - 1 });
    }
    out.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
    rows.set(out);
    const live = selection().filter((id) => out.some((r) => r.id === id));
    if (live.length !== selection().length) {
        selection.set(live);
        refreshSelected();
    }
}

function refreshSelected(): void {
    const id = selected();
    if (id === 0) {
        described.set(null);
        gizmo.set(null);
        return;
    }
    try {
        described.set(world.describe(id));
    } catch {
        select(0);
    }
}

function select(id: number, mode: "replace" | "toggle" = "replace"): void {
    let s = selection().slice();
    if (mode === "replace") s = id === 0 ? [] : [id];
    else if (id !== 0) s = s.includes(id) ? s.filter((x) => x !== id) : [...s, id];
    selection.set(s);
    addingComponent.set(false);
    if (brush() !== null && (s.length === 0 || !world.has(s[s.length - 1], "TileMap"))) brush.set(null);
    if (sculpt() !== null && (s.length === 0 || !world.has(s[s.length - 1], "Terrain"))) sculpt.set(null);
    refreshSelected();
}

function selectAll(): void {
    selection.set(rows().map((r) => r.id));
    refreshSelected();
}

/** Selected entities whose ancestors are not selected (destroying a parent takes its children). */
function topLevelSelection(): Array<{ id: number; path: string }> {
    const withPaths = selection().map((id) => ({ id, path: rows().find((r) => r.id === id)?.path ?? "" })).filter((s) => s.path.length > 0);
    return withPaths.filter((s) => !withPaths.some((o) => o.id !== s.id && s.path.startsWith(o.path + "/")));
}

// Models' thumbnails: drawn once a session by assets.preview into the project's .pocket/thumbs,
// shown by the Assets tab like an image's.
const thumbs = new Map<string, string>();
function thumbFor(path: string): string {
    let thumb = thumbs.get(path);
    if (thumb === undefined) {
        thumb = `.pocket/thumbs/${path.replace(/[\\/]/g, "_")}.png`;
        try {
            command("assets.preview", { path, size: 64, out: thumb });
        } catch {
            thumb = "";   // unreadable: no thumbnail
        }
        thumbs.set(path, thumb);
    }
    return thumb;
}

/** Refresh what every dock shows now (the tabs' data; the hierarchy and inspector refresh themselves). */
function refreshBottom(): void {
    const l = layout();
    for (const d of DOCKS) {
        const p = l.active[d];
        if (p !== "" && p !== "hierarchy" && p !== "inspector") refreshTab(p);
    }
}

function refreshTab(t: Tab): void {
    if (t === "console") logs.set(command<LogRow[]>("log.tail", { n: 40 }));
    else if (t === "events") recentEvents.set(command<WorldEvent[]>("events.recent", { n: 40 }));
    else if (t === "assets" || t === "script") {
        assetRows.set(command<AssetRow[]>("assets.list"));
        const found = command<{ diagnostics: TypeError[] }>("script.diagnostics").diagnostics;
        if (JSON.stringify(found) !== JSON.stringify(typeErrors())) typeErrors.set(found);
    }
    else if (t === "input") actions.set(command<Record<string, ActionBindings>>("input.describe"));
    else if (t === "audio") refreshMixer();
    else if (t === "timeline") timelineVersion.update((v) => v + 1);
    else transcriptText.set(command<{ text: string }>("transcript", { max_lines: 30 }).text);
}

// ------------------------------------------------------------------------------------ layout persistence
function loadLayout(): void {
    try {
        const r = command<{ text: string }>("project.read", { path: LAYOUT_PATH });
        const j = JSON.parse(r.text) as { layout?: Partial<Layout>; tab?: Tab; snap?: boolean; snap_step?: number; local?: boolean };
        if (j.layout) layout.set(normalizeLayout({ ...DEFAULT_LAYOUT, ...j.layout }, j.tab));
        if (j.snap === true) snap.set(true);
        if (j.local === true) localAxes.set(true);
        if (typeof j.snap_step === "number" && SNAP_STEPS.includes(j.snap_step)) snapStep.set(j.snap_step);
    } catch {
        // No saved layout yet.
    }
}

// A saved layout made whole: every pane in exactly one dock (one the file lacks goes to the bottom),
// each dock showing one of its own; a layout from before the docks gets them as they were.
function normalizeLayout(l: Layout, legacyTab?: Tab): Layout {
    const seen = new Set<Pane>();
    const docks = { left: [] as Pane[], right: [] as Pane[], bottom: [] as Pane[] };
    for (const d of DOCKS) for (const p of (l.docks?.[d] ?? DEFAULT_LAYOUT.docks[d])) if (PANES.includes(p) && !seen.has(p)) { docks[d].push(p); seen.add(p); }
    for (const p of PANES) if (!seen.has(p)) docks.bottom.push(p);
    const active = { ...DEFAULT_LAYOUT.active, ...(l.active ?? {}) };
    if (legacyTab !== undefined && docks.bottom.includes(legacyTab)) active.bottom = legacyTab;
    for (const d of DOCKS) if (!docks[d].includes(active[d] as Pane)) active[d] = docks[d][0] ?? "";
    return { ...l, docks, active };
}

/** Show a pane: the dock it is in brings it to the front. */
function showPane(p: Pane): void {
    const l = layout();
    const d = DOCKS.find((k) => l.docks[k].includes(p));
    if (d === undefined) return;
    layout.set({ ...l, active: { ...l.active, [d]: p } });
    refreshBottom();
    saveLayout();
}

/** Move a pane to a dock (the end of its tabs, in front); the dock it left shows its first pane. */
function movePane(p: Pane, to: Dock): void {
    const l = layout();
    const from = DOCKS.find((k) => l.docks[k].includes(p));
    if (from === undefined || from === to) { showPane(p); return; }
    const docks = { ...l.docks, [from]: l.docks[from].filter((x) => x !== p), [to]: [...l.docks[to], p] };
    const active = { ...l.active, [to]: p, [from]: l.active[from] === p ? (docks[from][0] ?? "") : l.active[from] };
    layout.set({ ...l, docks, active });
    notice.set(`${PANE_LABELS[p]} moved to the ${to} dock`);
    refreshBottom();
    saveLayout();
}

/** Where a tab dropped at (x, y) goes: the dock under it, or the edge of the scene pane it is near. */
function dockAt(x: number, y: number): Dock | null {
    const vp = viewportRect, main = mainRect;
    if (main.w === 0) return null;
    if (y >= main.y + main.h) return "bottom";
    if (x < vp.x) return "left";
    if (x >= vp.x + vp.w) return "right";
    if (x < vp.x + vp.w * 0.15) return "left";
    if (x >= vp.x + vp.w * 0.85) return "right";
    if (y >= vp.y + vp.h * 0.85) return "bottom";
    return null;
}

function saveLayout(): void {
    try {
        command("project.write", { path: LAYOUT_PATH, json: { layout: layout(), snap: snap(), snap_step: snapStep(), local: localAxes() } });
    } catch (e) {
        notice.set(`Layout not saved: ${String(e)}`);
    }
}

function clamp(v: number, lo: number, hi: number): number {
    return Math.max(lo, Math.min(hi, v));
}

// ------------------------------------------------------------------------------------ edits (all undoable)
function edit(label: string, redo: () => void, undo: () => void): void {
    history.perform(label, redo, undo);
    historyVersion.update((v) => v + 1);
}

function doUndo(): void {
    const label = history.undo();
    historyVersion.update((v) => v + 1);
    notice.set(label ? `Undid: ${label}` : "Nothing to undo");
    refreshHierarchy();
    refreshSelected();
}

function doRedo(): void {
    const label = history.redo();
    historyVersion.update((v) => v + 1);
    notice.set(label ? `Redid: ${label}` : "Nothing to redo");
    refreshHierarchy();
    refreshSelected();
}

function play(): void {
    if (!playing()) {
        editScene = world.save();
        history.clear();
        historyVersion.update((v) => v + 1);
        command("script.start", { name: "project" });
        playing.set(true);
        notice.set("Playing. Stop restores the scene as it was when you pressed Play.");
    }
    command("resume");
    paused.set(false);
}

function pause(): void {
    command("pause");
    paused.set(true);
}

function step(): void {
    command("step", { ticks: 1 });
    paused.set(true);
}

function stop(): void {
    command("pause");
    paused.set(true);
    if (playing()) {
        command("script.reload", { name: "project", start: false });
        if (editScene !== null) world.load(editScene as never);
        playing.set(false);
        history.clear();
        historyVersion.update((v) => v + 1);
        notice.set("Stopped; the scene was restored.");
    }
    refreshHierarchy();
    refreshSelected();
}

function saveScene(): void {
    try {
        const r = command<{ path: string; entities: number }>("project.save_scene");
        notice.set(`Saved ${r.entities} entities to ${r.path}`);
    } catch (e) {
        notice.set(`Save failed: ${String(e)}`);
    }
}

function spawnEntity(): void {
    const parent = selected() || undefined;
    const ref = { id: 0, fragment: undefined as Scene | undefined };
    edit(
        "Spawn entity",
        () => {
            ref.id = ref.fragment ? world.instantiate(ref.fragment, { parent }) : world.spawn("Entity", { parent, components: { Transform: {} } });
            refreshHierarchy();
            select(ref.id);
        },
        () => {
            ref.fragment = world.save(ref.id);
            world.destroy(ref.id);
            select(0);
            refreshHierarchy();
        },
    );
}

function deleteSelected(): void {
    const targets = topLevelSelection();
    if (targets.length === 0) return;
    const saved = targets.map((t) => ({ id: t.id, fragment: world.save(t.id), parent: world.describe(t.id).parent }));
    const label = saved.length === 1 ? `Delete ${described()?.name ?? "entity"}` : `Delete ${saved.length} entities`;
    edit(
        label,
        () => {
            for (const s of saved) if (alive(s.id)) world.destroy(s.id);
            select(0);
            refreshHierarchy();
        },
        () => {
            for (const s of saved) s.id = world.instantiate(s.fragment, { parent: s.parent });
            refreshHierarchy();
            selection.set(saved.map((s) => s.id));
            refreshSelected();
        },
    );
}

function siblingNames(parent: number | undefined): string[] {
    if (parent === undefined) return world.roots().map((id) => world.describe(id).name);
    return world.describe(parent).children.map((c) => c.name);
}

function uniqueName(base: string, taken: string[]): string {
    const stem = base.replace(/ \d+$/, "");
    if (!taken.includes(base)) return base;
    for (let i = 2; ; i++) {
        const candidate = `${stem} ${i}`;
        if (!taken.includes(candidate)) return candidate;
    }
}

function duplicateSelected(): void {
    const targets = topLevelSelection();
    if (targets.length === 0) return;
    const copies = targets.map((t) => {
        const d = world.describe(t.id);
        return { fragment: world.save(t.id), parent: d.parent, name: uniqueName(d.name, siblingNames(d.parent)), id: 0 };
    });
    edit(
        copies.length === 1 ? `Duplicate ${copies[0].name}` : `Duplicate ${copies.length} entities`,
        () => {
            for (const c of copies) c.id = world.instantiate(c.fragment, { parent: c.parent, name: c.name });
            refreshHierarchy();
            selection.set(copies.map((c) => c.id));
            refreshSelected();
        },
        () => {
            for (const c of copies) if (alive(c.id)) world.destroy(c.id);
            select(0);
            refreshHierarchy();
        },
    );
}

function renameEntity(id: number, name: string): void {
    const before = world.describe(id).name;
    if (name === before) return;
    edit(`Rename ${before}`, () => { world.rename(id, name); refreshHierarchy(); refreshSelected(); }, () => { world.rename(id, before); refreshHierarchy(); refreshSelected(); });
}

function addComponent(id: number, comp: ComponentName): void {
    edit(`Add ${comp}`, () => { world.set(id, comp, {} as never); refreshSelected(); }, () => { world.remove(id, comp); refreshSelected(); });
}

function removeComponent(id: number, comp: ComponentName): void {
    const before = world.get(id, comp);
    edit(`Remove ${comp}`, () => { world.remove(id, comp); refreshSelected(); }, () => { if (before) world.set(id, comp, before as never); refreshSelected(); });
}

function setField(entity: number, comp: ComponentName, field: string, sub: string | null, raw: string, type: string): void {
    let value: unknown = raw;
    if (type === "bool") value = raw === "true";
    else if (type === "json") {
        try {
            value = JSON.parse(raw);
        } catch {
            notice.set(`${comp}.${field}: not valid JSON`);
            return;
        }
        if (!Array.isArray(value)) {
            notice.set(`${comp}.${field}: expected a JSON array`);
            return;
        }
    } else if (type !== "string") {
        const n = Number(raw);
        if (!Number.isFinite(n)) return;
        value = n;
    }
    const patch: Record<string, unknown> = sub === null ? { [field]: value } : { [field]: { [sub]: value } };
    const before = world.get(entity, comp) as Record<string, unknown> | undefined;
    // Blur re-reports the input's value; an unchanged value is not an edit.
    const current = before === undefined ? undefined : sub === null ? before[field] : (before[field] as Record<string, unknown> | undefined)?.[sub];
    if (type === "json" ? JSON.stringify(current) === JSON.stringify(value) : current === value) return;
    try {
        edit(`Set ${comp}.${field}${sub ? "." + sub : ""}`, () => { world.set(entity, comp, patch as never); refreshSelected(); }, () => { if (before) world.set(entity, comp, before as never); refreshSelected(); });
    } catch (e) {
        notice.set(`Set failed: ${String(e)}`);
    }
}

// ------------------------------------------------------------------------------------ scene pane
function pixelScale(): number {
    const root = ui.describe(1) as { rect: { w: number } };
    const win = command<{ window: { width: number } }>("project.info").window;
    return root.rect.w > 0 ? win.width / root.rect.w : 1;
}

// ------------------------------------------------------------------------------------ tile brush
function tileMapSelected(): number {
    const id = selected();
    return id !== 0 && world.has(id, "TileMap") ? id : 0;
}

/** Paint the brush tile under a scene-pane point (in points), once per cell per stroke. */
function paintAt(x: number, y: number): void {
    const b = brush();
    if (!b || !stroke) return;
    const s = pixelScale();
    const z = world.get(stroke.entity, "Transform")?.position.z ?? 0;
    const ray = render.unproject(x * s, y * s, "xy", z);
    if (!ray.hit || !ray.point) return;
    const cell = tilemap.cell(stroke.entity, ray.point.x, ray.point.y);
    if (!cell.inside) return;
    const key = `${cell.tile_x},${cell.tile_y}`;
    if (stroke.cells.has(key)) return;
    try {
        const r = tilemap.set(stroke.entity, { tile_x: cell.tile_x, tile_y: cell.tile_y }, { gid: b.gid }, b.layer);
        stroke.cells.set(key, { tile_x: cell.tile_x, tile_y: cell.tile_y, was: r.was, gid: b.gid });
    } catch (err) {
        notice.set(`Paint failed: ${String(err)}`);
    }
}

/** One stroke is one undo step: every cell goes back to what it held before. */
function endStroke(): void {
    const st = stroke;
    stroke = null;
    if (!st) return;
    const cells = [...st.cells.values()].filter((c) => c.was !== c.gid);
    if (cells.length === 0) return;
    const apply = (use: "gid" | "was") => {
        for (const c of cells) tilemap.set(st.entity, { tile_x: c.tile_x, tile_y: c.tile_y }, { gid: c[use] }, st.layer);
    };
    history.record({ label: `paint ${cells.length} tile${cells.length === 1 ? "" : "s"}`, redo: () => apply("gid"), undo: () => apply("was") });
    historyVersion.update((v) => v + 1);
}

function saveMap(id: number): void {
    try {
        const r = tilemap.save(id);
        notice.set(`Saved ${r.path} (${r.bytes} bytes).`);
    } catch (err) {
        notice.set(`Save failed: ${String(err)}`);
    }
}

function TileBrush(props: { id: number }) {
    let mapInfo: { layers: Array<{ name: string; tiles: number }>; tilesets: Array<{ name: string; first_gid: number; tile_count: number }>; revision: number };
    try {
        mapInfo = tilemap.info(props.id) as typeof mapInfo;
    } catch (err) {
        return <Label text={`Map: ${String(err)}`} muted wrap />;
    }
    const b = brush();
    const layer = b?.layer ?? mapInfo.layers[0]?.name ?? "";
    const gid = b?.gid ?? mapInfo.tilesets[0]?.first_gid ?? 1;
    const tiles: number[] = [];
    for (const t of mapInfo.tilesets) for (let i = 0; i < Math.min(t.tile_count, 32); ++i) tiles.push(t.first_gid + i);
    return (
        <Section title="Tiles" name="tiles" right={<Button label={b ? "Stop painting" : "Paint"} small primary={b !== null} name="paint" onClick={() => brush.set(b ? null : { layer, gid })} />}>
            <Row wrap gap={S.sm}>
                {mapInfo.layers.map((l) => <Button key={l.name} label={`${l.name} (${l.tiles})`} small on={l.name === layer} name={`layer:${l.name}`} onClick={() => brush.set({ layer: l.name, gid })} />)}
            </Row>
            <Row wrap gap={S.sm}>
                <Button label="erase" small on={gid === 0} name="tile:0" onClick={() => brush.set({ layer, gid: 0 })} />
                {tiles.map((g) => <Button key={g} label={String(g)} small on={g === gid} name={`tile:${g}`} onClick={() => brush.set({ layer, gid: g })} />)}
            </Row>
            <Row gap={S.sm}>
                <Label text={b ? `Click or drag in the scene: gid ${gid} on ${layer}. Revision ${mapInfo.revision}.` : `Revision ${mapInfo.revision}. Pick a layer and a tile to paint under the mouse.`} color={C.faint} size={SIZE.small} wrap flex={1} />
                <Button label="Save map" small name="save-map" onClick={() => saveMap(props.id)} />
            </Row>
        </Section>
    );
}

// ------------------------------------------------------------------------------------ terrain brush
function terrainSelected(): number {
    const id = selected();
    return id !== 0 && world.has(id, "Terrain") ? id : 0;
}

/** Sculpt the selected terrain under a scene-pane point (in points): the ray from the camera through
 * it finds the ground, and the brush works there. */
function sculptAt(x: number, y: number): void {
    const b = sculpt();
    const st = sculptStroke;
    if (!b || !st) return;
    const s = pixelScale();
    const ray = render.unproject(x * s, y * s, "xz", 0);
    const hit = physics.raycast(ray.origin, ray.direction, { max_distance: 5000 });
    if (!hit || hit.entity !== st.entity) return;
    if (b.mode === "flatten" && st.target === undefined) st.target = hit.point.y;
    try {
        if ((b.mode === "paint" || b.mode === "erase") && st.layers && b.layer !== null) terrain.paintLayer(hit.point.x, hit.point.z, b.layer, { entity: st.entity, mode: b.mode, radius: b.radius, amount: Math.min(b.strength * 0.5, 1) });
        else if (b.mode === "paint" || b.mode === "erase") terrain.paint(hit.point.x, hit.point.z, b.mode === "paint" ? b.color : null, { entity: st.entity, radius: b.radius, amount: Math.min(b.strength * 0.5, 1) });
        else terrain.sculpt(hit.point.x, hit.point.z, { entity: st.entity, mode: b.mode, radius: b.radius, amount: b.mode === "raise" || b.mode === "lower" ? b.strength * 0.25 : b.strength * 0.5, target: st.target });
        st.touches++;
    } catch (err) {
        notice.set(`Sculpt failed: ${String(err)}`);
    }
}

/** One stroke is one undo step: the whole grid before and after. */
function endSculpt(): void {
    const st = sculptStroke;
    sculptStroke = null;
    if (!st || st.touches === 0) return;
    const before = st.before;
    const touches = `${st.touches} touch${st.touches === 1 ? "" : "es"}`;
    if (st.paint && st.layers) {
        const after = terrain.layerPaints(st.entity).paint;
        history.record({ label: `paint the terrain's layers (${touches})`, redo: () => { terrain.setLayerPaints(after, st.entity); }, undo: () => { terrain.setLayerPaints(before, st.entity); } });
    } else if (st.paint) {
        const after = terrain.paints(st.entity).paint;
        history.record({ label: `paint the terrain (${touches})`, redo: () => { terrain.setPaints(after, st.entity); }, undo: () => { terrain.setPaints(before, st.entity); } });
    } else {
        const after = terrain.heights(st.entity).heights;
        history.record({ label: `sculpt the terrain (${touches})`, redo: () => { terrain.setHeights(after, st.entity); }, undo: () => { terrain.setHeights(before, st.entity); } });
    }
    historyVersion.update((v) => v + 1);
}

// The hour of a Sky's day (docs/design/rendering.md, A day): a slider and the four times people
// look at most, each an undoable edit; the sun stands where the hour puts it as soon as it is set.
function SkyClock(props: { id: number }) {
    const sky = world.get(props.id, "Sky");
    if (!sky) return null;
    const t = sky.time_of_day;
    const setHour = (h: number) => {
        const before = world.get(props.id, "Sky")!.time_of_day;
        edit("time of day", () => world.set(props.id, "Sky", { time_of_day: h }), () => world.set(props.id, "Sky", { time_of_day: before }));
        refreshSelected();
    };
    const minutes = Math.round(t * 60);   // to the nearest minute (a stored 18.4 is 18.39999...)
    const clock = t < 0 ? "not set" : `${String(Math.floor(minutes / 60) % 24).padStart(2, "0")}:${String(minutes % 60).padStart(2, "0")}`;
    const times: Array<[string, number]> = [["dawn", 6.2], ["noon", 12], ["dusk", 18.4], ["night", 23]];
    return (
        <Section title="Time of day" name="sky-clock" right={<box padding={{ right: S.md }}><Label text={clock} color={C.accentText} name="sky-clock:time" /></box>}>
            <Prop label="hour">
                <Slider value={t < 0 ? 12 : t} min={0} max={24} step={0.25} width="100%" name="sky-clock:hour" onInput={(v) => setHour(v)} />
            </Prop>
            <Prop label="">
                {times.map(([label, h]) => <Button key={label} label={label} small name={`sky-clock:${label}`} onClick={() => setHour(h)} />)}
            </Prop>
        </Section>
    );
}

// A Weather's four amounts as sliders (docs/design/rendering.md, Weather): how hard it rains and
// snows, how wet things are and how much snow lies, each change an undoable edit.
function WeatherSliders(props: { id: number }) {
    const wx = world.get(props.id, "Weather");
    if (!wx) return null;
    const fields: Array<["rain" | "snow" | "wet" | "cover", string]> = [["rain", "rain"], ["snow", "snow"], ["wet", "wet"], ["cover", "snow lying"]];
    const setField = (f: "rain" | "snow" | "wet" | "cover", v: number) => {
        const before = world.get(props.id, "Weather")![f];
        edit(`weather ${f}`, () => world.set(props.id, "Weather", { [f]: v }), () => world.set(props.id, "Weather", { [f]: before }));
        refreshSelected();
    };
    return (
        <Section title="Weather" name="weather-sliders">
            {fields.map(([f, label]) => (
                <Prop key={f} label={label}>
                    <box flexGrow={1} flexShrink={1} flexBasis={0}><Slider value={wx[f]} min={0} max={1} step={0.05} width="100%" name={`weather:${f}`} onInput={(v) => setField(f, v)} /></box>
                    <box width={30}><Label text={wx[f].toFixed(2)} muted size={SIZE.small} align="right" name={`weather:${f}:value`} /></box>
                </Prop>
            ))}
        </Section>
    );
}

function TerrainBrush(props: { id: number }) {
    let info;
    try {
        info = terrain.info(props.id);
    } catch (err) {
        return <Label text={`Terrain: ${String(err)}`} muted wrap />;
    }
    const b = sculpt();
    const set = sculptSettings();
    const pick = (next: Partial<typeof set>) => {
        const merged = { ...set, ...next };
        sculptSettings.set(merged);
        if (b) sculpt.set(merged);
    };
    const name = world.describe(props.id).name.toLowerCase().replace(/[^a-z0-9]+/g, "-") || "terrain";
    return (
        <Section title="Sculpt" name="terrain-brush" right={<Button label={b ? "Stop sculpting" : "Sculpt"} small primary={b !== null} name="sculpt" onClick={() => sculpt.set(b ? null : set)} />}>
            <Row wrap gap={S.sm}>
                {(["raise", "lower", "flatten", "smooth", "paint", "erase"] as const).map((m) => <Button key={m} label={m} small on={set.mode === m} name={`sculpt:${m}`} onClick={() => pick({ mode: m })} />)}
            </Row>
            {(set.mode === "paint" || set.mode === "erase") && (info.layers ?? []).length > 0 ? (
                <Row wrap gap={S.sm} align="center">
                    <Label text="layer" muted />
                    <Button label="colour" small on={set.layer === null} name="paint-layer:colour" onClick={() => pick({ layer: null })} />
                    {(info.layers ?? []).map((l, i) => <Button key={`${i}`} label={l || `layer ${i}`} small on={set.layer === i} name={`paint-layer:${l || i}`} onClick={() => pick({ layer: i })} />)}
                </Row>
            ) : null}
            {set.mode === "paint" && (set.layer === null || (info.layers ?? []).length === 0) ? (
                <Row wrap gap={S.sm} align="center">
                    <box width={18} height={18} radius={4} border={1} borderColor={C.fieldLine} background={[set.color.r, set.color.g, set.color.b]} name="paint:swatch" />
                    {PAINTS.map(([label, c]) => <Button key={label} label={label} small on={set.color.r === c.r && set.color.g === c.g && set.color.b === c.b} name={`paint:${label}`} onClick={() => pick({ color: c })} />)}
                </Row>
            ) : null}
            <Prop label="radius">
                <box flexGrow={1} flexShrink={1} flexBasis={0}><Slider value={set.radius} min={0.5} max={16} step={0.5} width="100%" name="sculpt:radius" onInput={(v) => pick({ radius: v })} /></box>
                <box width={30}><Label text={`${set.radius}`} muted size={SIZE.small} align="right" /></box>
            </Prop>
            <Prop label="strength">
                <box flexGrow={1} flexShrink={1} flexBasis={0}><Slider value={set.strength} min={0.05} max={2} step={0.05} width="100%" name="sculpt:strength" onInput={(v) => pick({ strength: v })} /></box>
                <box width={30}><Label text={`${set.strength}`} muted size={SIZE.small} align="right" /></box>
            </Prop>
            <Row gap={S.sm}>
                <Label text={`${info.source === "noise" ? "Noise" : info.source}${info.edited ? ", sculpted" : ""}; ${info.resolution} by ${info.resolution}, ${info.lowest.toFixed(1)} to ${info.highest.toFixed(1)} high${info.painted > 0 ? `, ${Math.round(info.painted * 100)}% painted` : ""}. ${b ? "Click or drag on the ground." : ""}`} color={C.faint} size={SIZE.small} wrap flex={1} />
            </Row>
            <Row gap={S.sm}>
                <Button label="Save heightmap" small name="terrain:save" onClick={() => {
                    try {
                        const r = terrain.save(`assets/${name}-heights.png`, { entity: props.id });
                        notice.set(`Heights saved to ${r.path}; the terrain reads its heightmap from it now (Save the scene to keep that).`);
                    } catch (err) {
                        notice.set(`Save failed: ${String(err)}`);
                    }
                }} />
                <Button label="Reset" small name="terrain:reset" onClick={() => {
                    const before = terrain.heights(props.id).heights;
                    terrain.reset({ entity: props.id });
                    const after = terrain.heights(props.id).heights;
                    history.record({ label: "reset the terrain", redo: () => { terrain.setHeights(after, props.id); }, undo: () => { terrain.setHeights(before, props.id); } });
                    historyVersion.update((v) => v + 1);
                }} />
            </Row>
            {info.painted > 0 || info.paintmap ? (
                <Row gap={4}>
                    <Button label="Save paint" small name="terrain:save-paint" onClick={() => {
                        try {
                            const r = terrain.save(`assets/${name}-paint.png`, { paint: true, entity: props.id });
                            notice.set(`Paint saved to ${r.path}; the terrain reads its paintmap from it now (Save the scene to keep that).`);
                        } catch (err) {
                            notice.set(`Save failed: ${String(err)}`);
                        }
                    }} />
                    <Button label="Clear paint" small name="terrain:clear-paint" onClick={() => {
                        const before = terrain.paints(props.id).paint;
                        terrain.setPaints([], props.id);
                        history.record({ label: "clear the terrain's paint", redo: () => { terrain.setPaints([], props.id); }, undo: () => { terrain.setPaints(before, props.id); } });
                        historyVersion.update((v) => v + 1);
                    }} />
                </Row>
            ) : null}
            {(info.layers ?? []).length > 0 ? (
                <Row gap={4}>
                    <Button label="Save layers" small name="terrain:save-layers" onClick={() => {
                        try {
                            const r = terrain.save(`assets/${name}-layers.png`, { layers: true, entity: props.id });
                            notice.set(`Layer paint saved to ${r.path}; the terrain reads its layermap from it now (Save the scene to keep that).`);
                        } catch (err) {
                            notice.set(`Save failed: ${String(err)}`);
                        }
                    }} />
                    <Button label="Clear layer paint" small name="terrain:clear-layers" onClick={() => {
                        const before = terrain.layerPaints(props.id).paint;
                        terrain.setLayerPaints([], props.id);
                        history.record({ label: "clear the terrain's layer paint", redo: () => { terrain.setLayerPaints([], props.id); }, undo: () => { terrain.setLayerPaints(before, props.id); } });
                        historyVersion.update((v) => v + 1);
                    }} />
                </Row>
            ) : null}
        </Section>
    );
}

/** A hierarchy row dragged onto another becomes its child; dropped on the panel's own background it
 * becomes a root. One undoable edit; a row dropped on itself or its own descendant is left alone. */
function dropRow(id: number, e: UiEvent): void {
    if (e.x === undefined || e.y === undefined || !alive(id)) return;
    // The named box under the pointer, or the nearest named one above it (a row's label is not the row).
    let under = "";
    for (let node = ui.hit(e.x, e.y), hops = 0; node && hops < 16; ++hops) {
        const d = ui.describe(node) as { name?: string; parent?: number };
        const name = d.name ?? "";
        if (name.startsWith("entity:") || name === "hierarchy") { under = name; break; }
        node = d.parent ?? 0;
    }
    if (!under) return;
    let parent = 0;
    let where = "the root";
    if (under.startsWith("entity:")) {
        const row = rows().find((r) => `entity:${r.name}` === under);
        if (!row || row.id === id) return;
        parent = row.id;
        where = row.name;
    } else if (under !== "hierarchy") {
        return;   // dropped somewhere else: not a move
    }
    // Its own descendants are not a home for it.
    for (let p = parent; p !== 0;) {
        if (p === id) { notice.set("An entity cannot go under its own child"); return; }
        p = world.describe(p).parent ?? 0;
    }
    const before = world.describe(id).parent ?? 0;
    if (before === parent) return;
    const name = world.describe(id).name;
    const move = (to: number) => {
        command("world.reparent", { entity: id, parent: to === 0 ? null : to, keep_world: true });   // it stays where it stands
        command("world.update_transforms");
        refreshHierarchy();
        refreshSelected();
    };
    history.perform(`move ${name} under ${where}`, () => move(parent), () => move(before));
    historyVersion.update((v) => v + 1);
    notice.set(`${name} moved under ${where}`);
}

// ------------------------------------------------------------------------------------ input map
/** Apply a whole map through input.map (live state of actions that stay is kept) and show it. */
function applyActions(map: Record<string, ActionBindings>): void {
    command("input.map", { actions: map });
    actions.set(command<Record<string, ActionBindings>>("input.describe"));
}

function splitBindings(text: string): string[] {
    return text.split(/[,\s]+/).map((s) => s.trim()).filter((s) => s.length > 0);
}

/** One direction of one action rebound from its text field: an undoable edit. */
function rebind(name: string, part: "positive" | "negative" | "axis", text: string): void {
    const before = clone(actions());
    const after = clone(actions());
    const entry = after[name] ?? {};
    const list = splitBindings(text);
    if (list.length > 0) entry[part] = list;
    else delete entry[part];
    after[name] = entry;
    if (JSON.stringify(before[name]) === JSON.stringify(after[name])) return;
    edit(`Rebind ${name}`, () => applyActions(after), () => applyActions(before));
    notice.set(`${name} ${part}: ${list.length > 0 ? list.join(", ") : "nothing"}`);
}

/** The Input tab's Press: the next key (or pad button) replaces the part's keyboard keys; pad and mouse bindings stay. */
function pressFor(name: string, part: "positive" | "negative" | "axis"): void {
    capture.set({ name, part });
    notice.set(`Press a key or pad button for ${name} ${part} (Escape cancels).`);
}

function captured(binding: string | null): void {
    const cap = capture();
    if (!cap) return;
    capture.set(null);
    if (binding === null) { notice.set("Rebind cancelled."); return; }
    const kept = (actions()[cap.name]?.[cap.part] ?? []).filter((k) => k.startsWith("pad:") || k.startsWith("mouse:"));
    rebind(cap.name, cap.part, [binding, ...kept.filter((k) => k !== binding)].join(", "));
}

function addAction(name: string, keys: string): void {
    const clean = name.trim();
    const list = splitBindings(keys);
    if (!clean) { notice.set("A new action needs a name"); return; }
    if (list.length === 0) { notice.set(`${clean} needs at least one key (an action without bindings is refused)`); return; }
    if (actions()[clean] !== undefined) { notice.set(`There is already an action named ${clean}`); return; }
    const before = clone(actions());
    const after = { ...clone(actions()), [clean]: { positive: list } };
    edit(`Add action ${clean}`, () => applyActions(after), () => applyActions(before));
    newAction.set("");
    newKeys.set("");
    notice.set(`${clean} added: ${list.join(", ")}`);
}

function removeAction(name: string): void {
    const before = clone(actions());
    const after = clone(actions());
    delete after[name];
    edit(`Remove action ${name}`, () => applyActions(after), () => applyActions(before));
}

/** The map as input.json beside project.toml, which the runtime takes over project.toml's. */
function saveBindings(): void {
    try {
        command("project.write", { path: "input.json", json: { actions: actions() } });
        notice.set(`Bindings saved to input.json (${Object.keys(actions()).length} actions); it replaces the project.toml map`);
    } catch (e) {
        notice.set(`Bindings not saved: ${String(e)}`);
    }
}

// ------------------------------------------------------------------------------------ audio
function refreshMixer(): void {
    buses.set(command<Bus[]>("audio.buses"));
    master.set(command<{ master_volume: number; muted: boolean }>("audio.master"));
}

/** A bus changed live: the running game hears it at once; Save keeps it. */
function setBus(name: string, settings: Partial<Bus>): void {
    command("audio.bus", { name, ...settings });
    refreshMixer();
}

function saveMixer(): void {
    const out: Record<string, Partial<Bus>> = {};
    for (const b of buses()) out[b.name] = { volume: b.volume, muted: b.muted, lowpass: b.lowpass, highpass: b.highpass, echo: b.echo, echo_feedback: b.echo_feedback, echo_mix: b.echo_mix, reverb: b.reverb, duck_by: b.duck_by, duck_amount: b.duck_amount, duck_seconds: b.duck_seconds };
    try {
        command("project.write", { path: "audio.json", json: { buses: out } });
        notice.set(`Mixer saved to audio.json (${buses().length} buses); it applies over the project.toml buses`);
    } catch (e) {
        notice.set(`Mixer not saved: ${String(e)}`);
    }
}

// ------------------------------------------------------------------------------------ assets
/** One line about an asset, from assets.describe. */
function openScript(path: string): void {
    const r = command<{ text: string }>("project.read", { path });
    scriptPath.set(path);
    scriptText.set(r.text);
    scriptDirty.set(false);
    notice.set(`Opened ${path}.`);
}

function saveScript(): void {
    const path = scriptPath();
    if (!path) return;
    command("project.write", { path, text: scriptText() });
    scriptDirty.set(false);
    notice.set(`Saved ${path}; pocket --watch rebuilds and reloads it.`);
}

function describeAsset(path: string): string {
    try {
        const d = command<Record<string, unknown>>("assets.describe", { path });
        if (typeof d.error === "string") return d.error;
        if (d.kind === "mesh") {
            const count = (v: unknown, one: string) => { const n = Array.isArray(v) ? v.length : 0; return n > 0 ? `, ${n} ${one}${n > 1 ? "s" : ""}` : ""; };
            const via = d.importer === "blender" ? `; read through Blender, kept as ${String(d.converted)}` : d.importer === "obj" ? "; an OBJ" : d.importer === "stl" ? "; an STL" : d.importer === "ply" ? "; a PLY" : d.importer === "vox" ? "; a MagicaVoxel model" : d.importer === "voxels" ? "; voxels written as text" : "";
            return `${String(d.vertices)} vertices, ${String(d.triangles)} triangles, ${String(d.submeshes)} submeshes, ${String(d.nodes)} nodes${count(d.clips, "clip")}${count(d.lights, "light")}${count(d.cameras, "camera")}${via}`;
        }
        if (d.kind === "tilemap") {
            const layers = Array.isArray(d.layers) ? d.layers.length : 0;
            return `${String(d.orientation)} ${String(d.width)}x${String(d.height)} tiles of ${String(d.tile_width)}x${String(d.tile_height)} px, ${layers} layers`;
        }
        if (d.kind === "image") return `${String(d.width)}x${String(d.height)} px`;
        return "";
    } catch (e) {
        return String(e);
    }
}

function pickAsset(row: AssetRow): void {
    assetPick.set(row.path);
    assetInfo.set(row.kind === "mesh" || row.kind === "image" || row.kind === "tilemap" ? describeAsset(row.path) : `${row.kind}, ${row.bytes} bytes`);
}

/** The components an asset becomes when placed: a mesh on the ground plane, a sprite or a map in
 * the XY plane, a sound where it was dropped. Nothing for a file the runtime does not read. */
function assetComponents(row: AssetRow): { components: Record<string, unknown>; plane: "xz" | "xy" } | null {
    if (row.kind === "mesh") return { components: { MeshRenderer: { mesh: row.path } }, plane: "xz" };
    if (row.kind === "tilemap") return { components: { TileMap: { map: row.path } }, plane: "xy" };
    if (row.kind === "audio") return { components: { AudioSource: { clip: row.path } }, plane: "xz" };
    if (row.kind === "image") {
        // One unit on its longer side, the aspect kept.
        let size = { x: 1, y: 1 };
        try {
            const d = command<{ width?: number; height?: number }>("assets.describe", { path: row.path });
            if (d.width && d.height) size = { x: d.width / Math.max(d.width, d.height), y: d.height / Math.max(d.width, d.height) };
        } catch {
            // Undecodable: a unit square still shows the missing image.
        }
        return { components: { Sprite: { texture: row.path, size } }, plane: "xy" };
    }
    return null;
}

/** What a model brings besides its geometry, which only its node tree keeps: its lights and
 * cameras ("1 light and 1 camera"), or "" for a model placed as one drawable (none of either, or
 * one whose skin or clips move it as a whole). */
function modelExtras(path: string): string {
    try {
        const d = command<{ lights?: unknown[]; cameras?: unknown[]; clips?: unknown[]; skinned?: boolean }>("assets.describe", { path });
        if (d.skinned || (d.clips?.length ?? 0) > 0) return "";
        const parts: string[] = [];
        const lights = d.lights?.length ?? 0, cameras = d.cameras?.length ?? 0;
        if (lights > 0) parts.push(`${lights} light${lights > 1 ? "s" : ""}`);
        if (cameras > 0) parts.push(`${cameras} camera${cameras > 1 ? "s" : ""}`);
        return parts.join(" and ");
    } catch {
        return "";
    }
}

/** An asset row dropped on the scene pane: an entity under the pointer, one undoable edit. */
function placeAsset(row: AssetRow, e: UiEvent): void {
    if (e.x === undefined || e.y === undefined) return;
    // Inside the scene pane (by its rectangle: the gizmo's handles float over it and must not count as elsewhere).
    if (e.x < viewportRect.x || e.x >= viewportRect.x + viewportRect.w || e.y < viewportRect.y || e.y >= viewportRect.y + viewportRect.h) return;
    placeAssetAt(row, e.x, e.y);
}

/** An asset placed where a point of the window (in points) looks at its plane: a mesh on the
 * ground, a sprite or a map in the XY plane, a sound where it lands. A model with lights or cameras
 * (a Blender scene, say) comes in as its node tree, so they come along; any other as one entity.
 * One undoable edit. */
function placeAssetAt(row: AssetRow, x: number, y: number): void {
    const made = assetComponents(row);
    if (!made) { notice.set(`${row.path}: nothing in the runtime reads this kind of file`); return; }
    const s = pixelScale();
    const ray = render.unproject(x * s, y * s, made.plane, 0);
    const position = ray.hit && ray.point ? ray.point : { x: 0, y: 0, z: 0 };
    const stem = (row.path.split("/").pop() ?? row.path).replace(/\.[^.]+$/, "");
    const name = uniqueName(stem, siblingNames(undefined));
    const extras = row.kind === "mesh" ? modelExtras(row.path) : "";
    const ref = { id: 0, fragment: undefined as Scene | undefined };
    edit(
        `Place ${name}`,
        () => {
            ref.id = ref.fragment ? world.instantiate(ref.fragment)
                : extras ? world.instantiateMesh(row.path, { name, position })
                : world.spawn(name, { components: { Transform: { position }, ...made.components } as never });
            refreshHierarchy();
            select(ref.id);
        },
        () => {
            ref.fragment = world.save(ref.id);
            world.destroy(ref.id);
            select(0);
            refreshHierarchy();
        },
    );
    notice.set(`${name} placed at ${position.x.toFixed(2)}, ${position.y.toFixed(2)}, ${position.z.toFixed(2)}${extras ? `, as its node tree with ${extras}` : ""}${ray.hit ? "" : " (the pointer's ray misses the plane)"}`);
}

/** The picked asset placed in the middle of the scene pane (the Place button). */
function placePicked(): void {
    const row = [...assetRows(), ...builtinRows].find((r) => r.path === assetPick());
    if (row) placeAssetAt(row, viewportRect.x + viewportRect.w / 2, viewportRect.y + viewportRect.h / 2);
}

/** The picked model read again from its file: a Blender-read one converted anew even when its
 * content has not changed (a new Blender, say); the scene draws what came out. */
function reimportPicked(): void {
    const path = assetPick();
    try {
        const r = command<{ importer: string; seconds?: number; cached?: boolean }>("assets.import", { path, force: true });
        assetInfo.set(describeAsset(path));
        // Its thumbnail again, and the picture the interface holds of it forgotten.
        const old = thumbs.get(path);
        thumbs.delete(path);
        const thumb = thumbFor(path);
        if (old && thumb) command("assets.reload", { path: thumb });
        notice.set(`${path} imported again${r.importer === "blender" && r.seconds !== undefined ? ` through Blender in ${r.seconds.toFixed(1)} s` : ""}`);
    } catch (e) {
        notice.set(`${path} not imported: ${String(e)}`);
    }
}

function onViewportDown(e: UiEvent): void {
    endStroke();
    endSculpt();
    const shaping = terrainSelected();
    if (sculpt() !== null && shaping !== 0) {
        const paint = sculpt()!.mode === "paint" || sculpt()!.mode === "erase";
        const layers = paint && sculpt()!.layer !== null && (terrain.info(shaping).layers ?? []).length > 0;
        const before = layers ? terrain.layerPaints(shaping).paint : paint ? terrain.paints(shaping).paint : terrain.heights(shaping).heights;
        sculptStroke = { entity: shaping, paint, layers, before, pos: { x: e.x ?? 0, y: e.y ?? 0 }, touches: 0 };
        sculptAt(sculptStroke.pos.x, sculptStroke.pos.y);
        return;
    }
    const painting = tileMapSelected();
    if (brush() !== null && painting !== 0) {
        stroke = { entity: painting, layer: brush()!.layer, pos: { x: e.x ?? 0, y: e.y ?? 0 }, cells: new Map() };
        paintAt(stroke.pos.x, stroke.pos.y);
        return;
    }
    const s = pixelScale();
    const hit = render.pick(Math.floor((e.x ?? 0) * s), Math.floor((e.y ?? 0) * s));
    const toggle = e.mods?.includes("shift") || e.mods?.includes("meta") || e.mods?.includes("ctrl");
    if (hit) select(hit.id, toggle ? "toggle" : "replace");
    else if (!toggle) select(0);
    orbit = orbitFromCamera();
}

function onViewportUp(): void {
    endStroke();
    endSculpt();
}

function onViewportDrag(e: UiEvent): void {
    if (sculptStroke) {
        sculptStroke.pos.x += e.dx ?? 0;
        sculptStroke.pos.y += e.dy ?? 0;
        sculptAt(sculptStroke.pos.x, sculptStroke.pos.y);
        return;
    }
    if (stroke) {
        stroke.pos.x += e.dx ?? 0;
        stroke.pos.y += e.dy ?? 0;
        paintAt(stroke.pos.x, stroke.pos.y);
        return;
    }
    if (!orbit) orbit = orbitFromCamera();
    if (!orbit) return;
    orbit.yaw -= (e.dx ?? 0) * 0.01;
    orbit.pitch = Math.max(-1.4, Math.min(1.4, orbit.pitch + (e.dy ?? 0) * 0.01));
    applyOrbit(orbit);
}

function onViewportWheel(e: UiEvent): void {
    if (!orbit) orbit = orbitFromCamera();
    if (!orbit) return;
    orbit.distance = Math.max(0.5, orbit.distance * (1 - (e.dy ?? 0) * 0.08));
    applyOrbit(orbit);
}

/** Handle positions in points relative to the main row (the handles are its absolute children). */
function updateGizmo(): void {
    const id = selected();
    // While a brush works the scene pane (tiles, sculpting) the handles are out of its way.
    if (id === 0 || mainRect.w === 0 || brush() !== null || sculpt() !== null) {
        if (gizmo() !== null) gizmo.set(null);
        return;
    }
    const lay = layoutFor(id, localAxes());
    if (!lay) {
        if (gizmo() !== null) gizmo.set(null);
        return;
    }
    const s = pixelScale();
    const pt = (p: { x: number; y: number }) => ({ x: Math.round(p.x / s - mainRect.x), y: Math.round(p.y / s - mainRect.y) });
    const next: GizmoView = { center: pt(lay.center), x: pt(lay.tips.x), y: pt(lay.tips.y), z: pt(lay.tips.z) };
    // Only handles inside the scene pane are shown; the rest would sit over other panes.
    const inside = (p: { x: number; y: number }) => p.x + mainRect.x >= viewportRect.x && p.x + mainRect.x <= viewportRect.x + viewportRect.w && p.y + mainRect.y >= viewportRect.y && p.y + mainRect.y <= viewportRect.y + viewportRect.h;
    if (!inside(next.center)) {
        if (gizmo() !== null) gizmo.set(null);
        return;
    }
    const cur = gizmo();
    if (cur === null || JSON.stringify(cur) !== JSON.stringify(next)) gizmo.set(next);
}

function gizmoDown(axis: Axis): void {
    const ids = selection().filter((id) => world.has(id, "Transform"));
    const primary = selected();
    if (ids.length === 0 || primary === 0) return;
    const lay = layoutFor(primary, localAxes());
    if (!lay) return;
    const before = new Map<number, Transform>();
    const parents = new Map<number, ParentFrame | undefined>();
    for (const id of ids) {
        before.set(id, clone(world.get(id, "Transform")!));
        // World moves and turns are expressed in each parent's frame, so a child under a turned or scaled parent follows the handle exactly.
        const parent = world.describe(id).parent ?? 0;
        const pw = parent > 0 ? world.get(parent, "WorldTransform") : undefined;
        parents.set(id, pw ? { rotation: pw.rotation, scale: pw.scale } : undefined);
    }
    dragState = { ids, before, parents, layout: lay, axis, turned: 0, scaled: 1, moved: { x: 0, y: 0, z: 0 } };
}

/** Round to the nearest multiple of `step`. */
function snapTo(v: number, step: number): number {
    return Math.round(v / step) * step;
}

function gizmoDrag(e: UiEvent): void {
    if (!dragState) return;
    const s = pixelScale();
    const dx = (e.dx ?? 0) * s, dy = (e.dy ?? 0) * s;
    // Every drag is applied from the transforms at its start, so snapping rounds the whole move
    // (with Snap on: positions to half units on the dragged axes, turns to 15 degrees, scales to quarters).
    const snapping = snap();
    const axis = dragState.axis;
    if (axis === "rotate" || axis === "rotate_x" || axis === "rotate_z") {
        // Horizontal drag turns the selection around a world axis (R: Y, RX: X, RZ: Z), or the entity's own with Local on, 100 px per radian.
        dragState.turned += dx * 0.01;
        const about = axis === "rotate" ? "y" : axis === "rotate_x" ? "x" : "z";
        const q = axisQuat(about, snapping ? snapTo(dragState.turned, SNAP_ANGLE) : dragState.turned);
        for (const [id, before] of dragState.before) if (alive(id)) world.set(id, "Transform", { rotation: localAxes() ? multiplyQuat(before.rotation, q) : multiplyQuat(turnIntoParent(q, dragState.parents.get(id)), before.rotation) });
    } else if (axis === "scale" || axis === "scale_x" || axis === "scale_y" || axis === "scale_z") {
        // Drag right to grow, left to shrink, relative to the size at the start of the drag: S on
        // every axis, SX, SY, SZ on one.
        dragState.scaled = Math.max(0.01, dragState.scaled * Math.exp(dx * 0.005));
        const k = dragState.scaled;
        const scaled = (v: number, a: "x" | "y" | "z") => {
            if (axis !== "scale" && axis !== `scale_${a}`) return v;
            return snapping ? Math.max(SNAP_SCALE, snapTo(v * k, SNAP_SCALE)) : v * k;
        };
        for (const [id, before] of dragState.before) if (alive(id)) world.set(id, "Transform", { scale: { x: scaled(before.scale.x, "x"), y: scaled(before.scale.y, "y"), z: scaled(before.scale.z, "z") } });
    } else {
        const delta = axis === "plane" ? planeDelta(selected(), dx, dy) : axisDelta(dragState.layout, axis, dx, dy);
        const m = dragState.moved;
        m.x += delta.x; m.y += delta.y; m.z += delta.z;
        const step = snapStep();
        for (const [id, before] of dragState.before) {
            if (!alive(id)) continue;
            const local = intoParent(m, dragState.parents.get(id));
            // Snapped on the axes the drag touches (in the parent's frame, a turned parent can spread one world axis over several).
            const at = (v: number, a: "x" | "y" | "z") => (snapping && (axis === "plane" || Math.abs(local[a]) > 1e-9) ? snapTo(v + local[a], step) : v + local[a]);
            world.set(id, "Transform", { position: { x: at(before.position.x, "x"), y: at(before.position.y, "y"), z: at(before.position.z, "z") } });
        }
    }
    command("world.update_transforms");   // paused: no tick will do it before the handles are placed
    refreshSelected();
    updateGizmo();
}

function gizmoEnd(): void {
    if (!dragState) return;
    const st = dragState;
    dragState = null;
    const after = new Map<number, Transform>();
    for (const id of st.ids) {
        const t = world.get(id, "Transform");
        if (t) after.set(id, clone(t));
    }
    const moved = [...after].some(([id, t]) => JSON.stringify(st.before.get(id)) !== JSON.stringify(t));
    if (!moved) return;
    const verb = st.axis === "rotate" ? "Rotate" : st.axis === "scale" ? "Scale" : "Move";
    history.record({
        label: st.ids.length === 1 ? `${verb} entity` : `${verb} ${st.ids.length} entities`,
        undo: () => { for (const [id, t] of st.before) if (alive(id)) world.set(id, "Transform", t); refreshSelected(); },
        redo: () => { for (const [id, t] of after) if (alive(id)) world.set(id, "Transform", t); refreshSelected(); },
    });
    historyVersion.update((v) => v + 1);
    notice.set(`${verb}d ${st.ids.length === 1 ? described()?.name ?? "entity" : `${st.ids.length} entities`}`);
}

// ------------------------------------------------------------------------------------ frame
onFrame(() => {
    frame++;
    const st = command<{ tick: number; state_hash: string; frames: number }>("state");
    const sum = world.summary();
    const next = { tick: st.tick, hash: st.state_hash, entities: sum.entities, frames: st.frames };
    const cur = status();
    if (cur.tick !== next.tick || cur.hash !== next.hash || cur.entities !== next.entities) status.set(next);
    // The hierarchy follows the world every 15 frames, and at once when an entity came or went (a command from outside spawned one).
    if (frame % 15 === 1 || cur.entities !== next.entities) {
        refreshHierarchy();
        refreshSelected();
    }
    if (frame % 20 === 2) refreshBottom();
    // Keep the scene inside the pane.
    const vp = ui.query({ name: "viewport" })[0];
    const main = ui.query({ name: "main" })[0];
    if (main) mainRect = main.rect;
    if (vp) {
        viewportRect = vp.rect;
        const key = `${Math.round(vp.rect.x)},${Math.round(vp.rect.y)},${Math.round(vp.rect.w)},${Math.round(vp.rect.h)}`;
        if (key !== lastViewport) {
            lastViewport = key;
            command("render.viewport", { x: vp.rect.x, y: vp.rect.y, w: vp.rect.w, h: vp.rect.h });
            setProjectRoot(vp.id);
        }
    }
    updateGizmo();
});

onInput((events) => {
    for (const e of events) {
        if (capture() !== null) {
            // The Input tab's Press waits for a key or pad button.
            if (e.type === "key_down" && e.ui === undefined && e.key !== undefined) { captured(e.key === "Escape" ? null : e.key); continue; }
            if (e.type === "pad_button" && e.pressed && e.button !== undefined) { captured(`pad:${e.button}`); continue; }
        }
        if (e.ui !== undefined && e.type !== "key_down") continue;
        if (e.type === "key_down" && e.ui === undefined) {
            const cmd = e.mods?.includes("meta") || e.mods?.includes("ctrl");
            const shift = e.mods?.includes("shift") ?? false;
            if (cmd && e.key === "Z") { if (shift) doRedo(); else doUndo(); }
            else if (cmd && e.key === "Y") doRedo();
            else if (cmd && e.key === "S") saveScene();
            else if (cmd && e.key === "D") duplicateSelected();
            else if (cmd && e.key === "A") selectAll();
            else if (e.key === "Space") { if (paused()) play(); else pause(); }
            else if (e.key === "Delete" || e.key === "Backspace") deleteSelected();
            else if (e.key === "Escape") select(0);
        }
    }
});

// ------------------------------------------------------------------------------------ views
function toggleOverlays(): void {
    const on = !overlays();
    render.debug({ colliders: on, joints: on, lights: on });
    overlays.set(on);
}

function toggleSnap(): void {
    snap.set(!snap());
    saveLayout();
    notice.set(snap() ? `Snap on: moves to ${snapStep()} units, turns to 15 degrees, scales to quarters` : "Snap off");
}

function toggleLocal(): void {
    localAxes.set(!localAxes());
    saveLayout();
    updateGizmo();
    notice.set(localAxes() ? "Local axes: the handles move and turn along the entity's own axes" : "World axes");
}

/** The next grid step for snapped moves: 0.1, 0.25, 0.5, 1, 2 units, round and round. */
function cycleSnapStep(): void {
    const i = SNAP_STEPS.indexOf(snapStep());
    snapStep.set(SNAP_STEPS[(i + 1) % SNAP_STEPS.length]);
    saveLayout();
    notice.set(`Snap step ${snapStep()} units`);
}

// The transport's icons, drawn from boxes where the UI font has no glyph (pause, stop, step's bar).
function PlayIcon(props: { color: string }): VNode {
    return <Label text="▶" size={9} color={props.color} />;
}
function PauseIcon(props: { color: string }): VNode {
    return (
        <box direction="row" gap={3}>
            <box width={3} height={10} radius={1} background={props.color} />
            <box width={3} height={10} radius={1} background={props.color} />
        </box>
    );
}
function StepIcon(props: { color: string }): VNode {
    return (
        <box direction="row" align="center" gap={1}>
            <Label text="▶" size={8} color={props.color} />
            <box width={2} height={9} radius={1} background={props.color} />
        </box>
    );
}
function StopIcon(props: { color: string }): VNode {
    return <box width={9} height={9} radius={2} background={props.color} />;
}

function Toolbar() {
    historyVersion();
    const none = selection().length === 0;
    return (
        <Row padding={[0, S.lg]} gap={S.sm} name="toolbar" height={36} background={C.window}>
            <box width={14} height={14} radius={4} background={C.accent} margin={{ right: 2 }} />
            <Label text="Pocket" size={SIZE.title} weight="bold" />
            <Label text={info.name} color={C.faint} size={SIZE.body} />
            <Sep />
            <box direction="row" align="center" gap={S.xs} padding={S.xs} radius={6} background={C.pane} name="transport">
                {paused()
                    ? <Button label="Play" primary name="play" onClick={play} icon={<PlayIcon color="#ffffff" />} />
                    : <Button label="Pause" on name="pause" onClick={pause} icon={<PauseIcon color={C.accentText} />} />}
                <Button label="Step" ghost name="step" onClick={step} disabled={!paused()} icon={<StepIcon color={paused() ? C.text : C.faint} />} />
                <Button label="Stop" ghost name="stop" onClick={stop} disabled={!playing()} icon={<StopIcon color={playing() ? C.danger : C.faint} />} />
            </box>
            <Sep />
            <Button label="Undo" ghost name="undo" onClick={doUndo} disabled={!history.canUndo()} />
            <Button label="Redo" ghost name="redo" onClick={doRedo} disabled={!history.canRedo()} />
            <Sep />
            <Button label="Spawn" ghost name="spawn" onClick={spawnEntity} />
            <Button label="Clone" ghost name="duplicate" onClick={duplicateSelected} disabled={none} />
            <Button label="Delete" ghost name="delete" onClick={deleteSelected} disabled={none} />
            <box flex={1} />
            {info.scene ? <Label text={info.scene} color={C.faint} size={SIZE.small} /> : null}
            <Button label="Save" name="save" onClick={saveScene} disabled={!info.scene} />
        </Row>
    );
}

/** The hierarchy: a row per entity by path, children indented under their parent with a guide per
 * level; the primary selection stronger than the rest of it. */
function Hierarchy(props: { width: Dim; grow?: boolean }) {
    const list = rows();
    const sel = selection();
    const primary = selected();
    const hov = hovered();
    return (
        <box width={props.width} flex={props.grow ? 1 : undefined} direction="column" background={C.pane} overflow="hidden" name="hierarchy">
            <box flexGrow={1} flexShrink={1} padding={S.sm} overflow="scroll">
                {list.length === 0 ? <Empty title="No entities" hint="Press Play to run the project's scripts, or Spawn one." /> : null}
                {list.map((r, i) => {
                    const on = sel.includes(r.id);
                    const parent = list[i + 1]?.path.startsWith(`${r.path}/`) ?? false;
                    const guides: VNode[] = [];
                    for (let d = 0; d < r.depth; d++) guides.push(<box key={`g${d}`} position="absolute" left={S.lg + 3 + d * 14} top={0} width={1} height={ROW} background={C.line} />);
                    return (
                        <box key={r.id} name={`entity:${r.name}`} direction="row" align="center" gap={S.md} height={ROW} padding={{ left: S.lg + r.depth * 14, right: S.md }} radius={4}
                            background={on ? (r.id === primary ? C.selected : C.selectedSoft) : hov === `row:${r.id}` ? C.rowHover : null}
                            onClick={(e) => select(r.id, e.mods?.includes("shift") || e.mods?.includes("meta") || e.mods?.includes("ctrl") ? "toggle" : "replace")} onDrag={() => undefined} onDragEnd={(e) => dropRow(r.id, e)} onHover={hoverOn(`row:${r.id}`)}>
                            {guides}
                            <box width={7} height={7} radius={parent ? 2 : 4} background={on ? C.accentText : parent ? C.dim : null} borderColor={parent || on ? null : C.faint} border={parent || on ? 0 : 1} />
                            <Label text={r.name} color={on ? "#ffffff" : C.text} />
                        </box>
                    );
                })}
            </box>
        </box>
    );
}

/** A field's row in the inspector: its name in the label column, its input (or inputs) filling the rest. */
function fieldInputs(entity: number, comp: ComponentName, field: SchemaField, value: unknown) {
    if (Array.isArray(value)) {
        // A list field (records): edited as JSON, the whole array at once.
        return (
            <Prop key={field.name} label={`${field.name} (${value.length})`}>
                <TextInput value={JSON.stringify(value)} grow name={`${comp}.${field.name}`} onChange={(v) => setField(entity, comp, field.name, null, v, "json")} />
            </Prop>
        );
    }
    if (value !== null && typeof value === "object") {
        // A vector or a colour: one input a part, sharing the row, each led by its letter in the axis colour.
        const obj = value as Record<string, unknown>;
        return (
            <Prop key={field.name} label={field.name}>
                {Object.keys(obj).map((k) => (
                    <box key={k} direction="row" align="center" gap={S.xs} flexGrow={1} flexShrink={1} flexBasis={0}>
                        <Label text={k.toUpperCase()} size={10} weight="bold" color={AXIS_COLORS[k] ?? C.faint} />
                        <TextInput value={formatNumber(obj[k])} grow padding={[S.xs, S.sm]} name={`${comp}.${field.name}.${k}`} onChange={(v) => setField(entity, comp, field.name, k, v, "float")} />
                    </box>
                ))}
            </Prop>
        );
    }
    if (typeof value === "boolean") {
        const name = `${comp}.${field.name}`;
        const hot = hovered() === name;
        return (
            <Prop key={field.name} label={field.name}>
                <box name={name} direction="row" align="center" gap={S.md} height={ROW} onClick={() => setField(entity, comp, field.name, null, value ? "false" : "true", "bool")} onHover={hoverOn(name)}>
                    <box width={14} height={14} radius={3} justify="center" align="center" background={value ? C.accent : C.field} borderColor={value ? C.accent : hot ? C.dim : C.fieldLine} border={1}>
                        {value ? <Label text="✓" size={10} color="#ffffff" /> : null}
                    </box>
                    <Label text={value ? "true" : "false"} size={SIZE.small} color={hot ? C.text : C.dim} />
                </box>
            </Prop>
        );
    }
    const names = field.names;
    if (names && names.length > 0 && typeof value === "number") {
        // A code with value names (Light.kind, RigidBody.kind, a project's own): stepped through by name.
        return (
            <Prop key={field.name} label={field.name}>
                <Choice value={names[value] ?? String(value)} options={names} width="100%" name={`${comp}.${field.name}`} onChange={(v) => setField(entity, comp, field.name, null, String(names.indexOf(v)), "float")} />
            </Prop>
        );
    }
    if (comp === "MeshRenderer" && field.name === "texture") {
        // A pattern the engine draws (docs/design/assets.md, Patterns), stepped through by name: the
        // texture, its normal map and a world-laid tile in one edit.
        const now = String(value ?? "");
        const shown = now.startsWith("pattern:") ? now.slice(8).split("?")[0] : now === "" ? "none" : "file";
        return (
            <box key={field.name} direction="column" gap={3}>
                <Prop label={field.name}>
                    <TextInput value={now} grow name={`${comp}.${field.name}`} onChange={(v) => setField(entity, comp, field.name, null, v, "string")} />
                </Prop>
                <Prop label="pattern">
                    <Choice value={shown} options={["none", ...PATTERNS, ...(shown === "file" ? ["file"] : [])]} width="100%" name={`${comp}.pattern`} onChange={(v) => setPattern(entity, v)} />
                </Prop>
            </box>
        );
    }
    return (
        <Prop key={field.name} label={field.name}>
            <TextInput value={typeof value === "number" ? formatNumber(value) : String(value ?? "")} grow name={`${comp}.${field.name}`} onChange={(v) => setField(entity, comp, field.name, null, v, typeof value === "number" ? "float" : "string")} />
        </Prop>
    );
}

const PATTERNS = ["bricks", "tiles", "planks", "cobble", "shingles", "grid", "checker", "stripes", "concrete", "rock", "sand", "dirt", "grass", "metal", "noise"];

/** A pattern on a MeshRenderer: its texture, the matching normal map, laid on from the world's axes
 * (two units a repeat unless it had a tile), its colour white so the pattern shows; "none" clears them. */
function setPattern(entity: number, name: string): void {
    if (name === "file") return;
    const before = world.get(entity, "MeshRenderer") as Record<string, unknown> | undefined;
    if (!before) return;
    const patch = name === "none"
        ? { texture: "", normal_map: "", texture_tile: 0 }
        : { texture: `pattern:${name}`, normal_map: `pattern:${name}?map=normal`, texture_tile: Number(before.texture_tile) > 0 ? before.texture_tile : 2, color: { r: 1, g: 1, b: 1, a: 1 } };
    edit(`Pattern ${name}`, () => { world.set(entity, "MeshRenderer", patch as never); refreshSelected(); }, () => { world.set(entity, "MeshRenderer", before as never); refreshSelected(); });
}

function formatNumber(v: unknown): string {
    if (typeof v !== "number") return String(v ?? "");
    return Number.isInteger(v) ? String(v) : v.toFixed(3).replace(/\.?0+$/, "");
}

/** The inspector: the entity's name and path, the brushes its components bring, then a section per
 * component with a row per field, and Add component at the foot. */
function Inspector(props: { width: Dim; grow?: boolean }) {
    const d = described();
    if (!d) {
        return (
            <box width={props.width} flex={props.grow ? 1 : undefined} direction="column" background={C.pane} name="inspector">
                <Empty title="Nothing selected" hint="Select an entity in the hierarchy or click it in the scene. Shift-click adds to the selection." />
            </box>
        );
    }
    const present = Object.keys(d.components);
    const missing = schema.filter((c) => c.serialized && !present.includes(c.name));
    return (
        <box width={props.width} flex={props.grow ? 1 : undefined} direction="column" background={C.pane} overflow="hidden" name="inspector">
            <box flexGrow={1} flexShrink={1} padding={[S.lg, S.lg]} gap={S.lg + 2} overflow="scroll">
                <box gap={S.sm}>
                    <TextInput value={d.name} name="entity-name" size={SIZE.title} padding={[3, S.lg]} onChange={(v) => { if (v.length > 0) renameEntity(d.id, v); }} />
                    <Row gap={S.md} padding={[0, S.xs]}>
                        <Label text={d.path} size={SIZE.small} color={C.faint} flex={1} />
                        <Label text={`#${d.id}`} size={SIZE.small} color={C.faint} />
                    </Row>
                </box>
                {present.includes("TileMap") ? <TileBrush id={d.id} /> : null}
                {present.includes("Terrain") ? <TerrainBrush id={d.id} /> : null}
                {present.includes("Sky") ? <SkyClock id={d.id} /> : null}
                {present.includes("Weather") ? <WeatherSliders id={d.id} /> : null}
                {schema.filter((c) => present.includes(c.name)).map((c) => (
                    <Section key={c.name} title={c.name}
                        right={c.serialized ? <Button label="Remove" ghost quiet small name={`remove:${c.name}`} onClick={() => removeComponent(d.id, c.name as ComponentName)} /> : <box padding={{ right: S.md }}><Label text="derived" size={SIZE.small} color={C.faint} /></box>}>
                        {c.fields.map((f) => fieldInputs(d.id, c.name as ComponentName, f, (d.components as Record<string, Record<string, unknown>>)[c.name]?.[f.name]))}
                    </Section>
                ))}
                {addingComponent() ? (
                    <Section title="Add component" right={<Button label="cancel" ghost quiet small onClick={() => addingComponent.set(false)} />}>
                        <Row wrap gap={S.sm}>
                            {missing.map((c) => <Button key={c.name} label={c.name} small name={`add:${c.name}`} onClick={() => { addComponent(d.id, c.name as ComponentName); addingComponent.set(false); }} />)}
                        </Row>
                    </Section>
                ) : (
                    <Button label="+ Add component" name="add-component" onClick={() => addingComponent.set(true)} disabled={missing.length === 0} />
                )}
            </box>
        </box>
    );
}

/** What a tab pane shows (its body), in whichever dock it is. */
// ------------------------------------------------------------------------------------ timeline pane
// The selected entity's Timeline (docs/design/timelines.md): its file's tracks as rows of keys on a
// ruler with the playhead, which scrubs (timeline.seek applies the tracks there without a tick).
// A key is picked to delete it; a track keys the field's value now at the playhead; a new track
// starts from the value now. Every change writes the file, which the Timeline reads again, and is
// one undo step.
interface TimelineTrackDoc { entity?: string; component: string; field: string; keys: unknown[] }
interface TimelineDoc { duration?: number; tracks?: TimelineTrackDoc[]; events?: unknown[] }
const timelineVersion = signal(0);
const timelineKey = signal<{ path: string; track: number; key: number } | null>(null);
const timelineNew = signal<{ entity: string; component: string; field: string }>({ entity: "", component: "Transform", field: "position.y" });
const TIMELINE_WIDTH = 420;

function readTimeline(path: string): TimelineDoc | null {
    try {
        return JSON.parse(command<{ text: string }>("project.read", { path }).text) as TimelineDoc;
    } catch {
        return null;
    }
}
function writeTimeline(path: string, before: TimelineDoc | null, after: TimelineDoc, label: string): void {
    const put = (doc: TimelineDoc | null) => { if (doc) command("project.write", { path, json: doc }); timelineVersion.update((v) => v + 1); };
    put(after);
    history.record({ label, undo: () => put(before), redo: () => put(after) });
    historyVersion.update((v) => v + 1);
}
function keyTime(k: unknown): number {
    return Array.isArray(k) ? Number(k[0]) : Number((k as { time?: number }).time ?? 0);
}
function timelineDuration(d: TimelineDoc): number {
    if (typeof d.duration === "number") return d.duration;
    let end = 0;
    for (const tr of d.tracks ?? []) for (const k of tr.keys) end = Math.max(end, keyTime(k));
    for (const e of d.events ?? []) end = Math.max(end, keyTime(e));
    return end;
}
/** A field's value now as a key holds it: a number as it is, a vector or a colour as a list. */
function fieldNow(entity: number, component: string, field: string): unknown {
    let v: unknown = world.get(entity, component as ComponentName);
    for (const part of field.split(".")) {
        if (v === null || typeof v !== "object") return undefined;
        v = (v as Record<string, unknown>)[part];
    }
    if (v !== null && typeof v === "object") {
        const o = v as Record<string, number>;
        for (const order of [["x", "y", "z", "w"], ["r", "g", "b", "a"]]) if (order[0] in o) return order.filter((k) => k in o).map((k) => o[k]);
    }
    return v;
}
function trackTarget(owner: number, name: string | undefined): number {
    return !name || name === "." ? owner : world.find(name) ?? 0;
}
/** Key a track at the playhead with its field's value now (replacing a key already at that time). */
function keyAtPlayhead(owner: number, path: string, doc: TimelineDoc, track: number, time: number): void {
    const tr = doc.tracks?.[track];
    if (!tr) return;
    const target = trackTarget(owner, tr.entity);
    const value = target ? fieldNow(target, tr.component, tr.field) : undefined;
    if (value === undefined) { notice.set(`${tr.entity ?? "this entity"} has no ${tr.component}.${tr.field} to key`); return; }
    const after = clone(doc);
    const t = Math.round(time * 1000) / 1000;
    const keys = after.tracks![track].keys.filter((k) => Math.abs(keyTime(k) - t) > 1e-4);
    keys.push([t, value]);
    keys.sort((a, b) => keyTime(a) - keyTime(b));
    after.tracks![track].keys = keys;
    writeTimeline(path, doc, after, `key ${tr.component}.${tr.field} at ${t}s`);
}

function TimelineBody(): VNode[] {
    timelineVersion();
    historyVersion();
    const id = selected();
    const tl = id !== 0 && world.has(id, "Timeline") ? world.get(id, "Timeline") : undefined;
    if (!tl) {
        const owners = world.query({ with: ["Timeline"] });
        return [
            <Label key="hint" text={owners.length > 0 ? "Select an entity with a Timeline:" : "No entity has a Timeline yet: add one in the inspector and name its file."} muted size={SIZE.body} />,
            <Row key="owners" gap={4} wrap>
                {owners.map((r) => <Button key={r.id} label={r.path} small name={`tl:owner:${r.path}`} onClick={() => select(r.id)} />)}
            </Row>,
        ];
    }
    const path = tl.path;
    const doc = path ? readTimeline(path) : null;
    if (!doc) {
        return [
            <Row key="none" gap={6}>
                <Label text={path ? `${path} is not in the project yet.` : "This Timeline names no file."} muted size={SIZE.body} />
                {path ? <Button label="Create it" small name="tl:create" onClick={() => writeTimeline(path, null, { duration: 4, tracks: [], events: [] }, `create ${path}`)} /> : null}
            </Row>,
        ];
    }
    const duration = Math.max(timelineDuration(doc), 0.001);
    const x = (time: number) => Math.round(Math.min(Math.max(time / duration, 0), 1) * TIMELINE_WIDTH);
    const pick = timelineKey();
    const picked = pick && pick.path === path ? pick : null;
    const seek = (v: number) => { command("timeline.seek", { entity: id, time: v }); timelineVersion.update((n) => n + 1); };
    const tracks = doc.tracks ?? [];
    const draft = timelineNew();
    const out: Array<VNode | null> = [
        <Row key="head" gap={8} name="tl:head">
            <Label text={`${path}, ${duration.toFixed(2)} s`} size={SIZE.body} />
            <Button label={tl.playing ? "Stop" : "Play"} small primary={tl.playing} name="tl:play" onClick={() => { command(tl.playing ? "timeline.stop" : "timeline.play", tl.playing ? { entity: id } : { entity: id, path, time: tl.time }); timelineVersion.update((n) => n + 1); }} />
            <Slider value={tl.time} min={0} max={duration} step={duration / 400} width={TIMELINE_WIDTH - 60} name="tl:time" onInput={seek} />
            <Label text={`${tl.time.toFixed(2)} s`} muted size={SIZE.body} name="tl:now" />
        </Row>,
        ...tracks.map((tr, i) => (
            <Row key={`track${i}`} gap={8} name={`tl:track:${i}`}>
                <box width={190}><Label text={`${tr.entity && tr.entity !== "." ? tr.entity : "(this)"} ${tr.component}.${tr.field}`} size={SIZE.body} /></box>
                <box width={TIMELINE_WIDTH + 8} height={16} background={C.field} borderColor={C.fieldLine} border={1} radius={4}>
                    {tr.keys.map((k, n) => (
                        <box key={n} name={`tl:key:${i}:${n}`} position="absolute" left={x(keyTime(k))} top={1} width={8} height={12} radius={2}
                            background={picked && picked.track === i && picked.key === n ? C.warn : C.accent} onClick={() => timelineKey.set({ path, track: i, key: n })} />
                    ))}
                    <box position="absolute" left={x(tl.time) + 3} top={-1} width={2} height={16} background={C.danger} />
                </box>
                <Button label="Key" small name={`tl:track:${i}:key`} onClick={() => keyAtPlayhead(id, path, doc, i, tl.time)} />
                <Button label="Remove" small name={`tl:track:${i}:remove`} onClick={() => {
                    const after = clone(doc);
                    after.tracks!.splice(i, 1);
                    timelineKey.set(null);
                    writeTimeline(path, doc, after, `remove the ${tr.component}.${tr.field} track`);
                }} />
            </Row>
        )),
        picked && tracks[picked.track]?.keys[picked.key] !== undefined ? (
            <Row key="picked" gap={8} name="tl:picked">
                <Label text={`key at ${keyTime(tracks[picked.track].keys[picked.key]).toFixed(2)} s: ${JSON.stringify(Array.isArray(tracks[picked.track].keys[picked.key]) ? (tracks[picked.track].keys[picked.key] as unknown[]).slice(1) : tracks[picked.track].keys[picked.key])}`} size={SIZE.body} />
                <Button label="Go to it" small name="tl:key:goto" onClick={() => seek(keyTime(tracks[picked.track].keys[picked.key]))} />
                <Button label="Delete key" small name="tl:key:delete" onClick={() => {
                    const after = clone(doc);
                    after.tracks![picked.track].keys.splice(picked.key, 1);
                    if (after.tracks![picked.track].keys.length === 0) after.tracks!.splice(picked.track, 1);
                    timelineKey.set(null);
                    writeTimeline(path, doc, after, "delete a key");
                }} />
            </Row>
        ) : null,
        <Row key="new" gap={6} name="tl:new">
            <TextInput value={draft.entity} placeholder="entity (this)" width={110} name="tl:new:entity" onChange={(v) => timelineNew.set({ ...timelineNew(), entity: v })} />
            <TextInput value={draft.component} width={110} name="tl:new:component" onChange={(v) => timelineNew.set({ ...timelineNew(), component: v })} />
            <TextInput value={draft.field} width={110} name="tl:new:field" onChange={(v) => timelineNew.set({ ...timelineNew(), field: v })} />
            <Button label="Add track" small name="tl:new:add" onClick={() => {
                const d = timelineNew();
                const target = trackTarget(id, d.entity);
                const value = target ? fieldNow(target, d.component, d.field) : undefined;
                if (value === undefined) { notice.set(`${d.entity || "this entity"} has no ${d.component}.${d.field}`); return; }
                const after = clone(doc);
                after.tracks = [...(after.tracks ?? []), { ...(d.entity ? { entity: d.entity } : {}), component: d.component, field: d.field, keys: [[Math.round(tl.time * 1000) / 1000, value]] }];
                writeTimeline(path, doc, after, `add a ${d.component}.${d.field} track`);
            }} />
            <Label text="A new track starts with the field's value now at the playhead; Key adds the value now to a track." muted size={SIZE.small} wrap flex={1} />
        </Row>,
    ];
    return out.filter((n): n is VNode => n !== null);
}

function TabBody(t: Tab): VNode[] {
    let body;
    if (t === "timeline") {
        body = TimelineBody();
    } else if (t === "console") {
        // A line a row: the tick, the level in its colour, the category, the message.
        const ticked = logs().some((l) => l.tick !== undefined);   // a tick column once a line has one
        body = logs().length === 0 ? [<Empty key="empty" title="No log lines yet" />] : logs().map((l) => {
            const tone = l.level === "error" ? C.danger : l.level === "warn" ? C.warn : null;
            return (
                <Row key={l.seq} gap={S.lg} height={18}>
                    {ticked ? <box width={40}><Label text={l.tick !== undefined ? String(l.tick) : ""} size={SIZE.body} color={C.faint} align="right" /></box> : null}
                    <box width={34}><Label text={l.level} size={SIZE.body} color={tone ?? C.faint} /></box>
                    <Label text={`${l.cat}:`} size={SIZE.body} color={C.dim} />
                    <Label text={l.msg} size={SIZE.body} color={tone ?? C.text} />
                </Row>
            );
        });
    } else if (t === "events") {
        body = recentEvents().length === 0 ? [<Empty key="empty" title="No events yet" hint="Events appear as the project raises them while it plays." />] : recentEvents().map((e) => (
            <Row key={e.seq} gap={S.lg} height={18}>
                <box width={48}><Label text={`#${e.seq}`} size={SIZE.body} color={C.faint} align="right" /></box>
                <box width={48}><Label text={`t${e.tick}`} size={SIZE.body} color={C.faint} /></box>
                <Label text={e.type} size={SIZE.body} color={C.accentText} />
                {e.subject ? <Label text={`@${e.subject}`} size={SIZE.body} color={C.text} /> : null}
                {e.cause ? <Label text={`<- #${e.cause}`} size={SIZE.body} color={C.faint} /> : null}
                {e.data ? <Label text={JSON.stringify(e.data)} size={SIZE.body} color={C.dim} /> : null}
            </Row>
        ));
    } else if (t === "input") {
        const map = actions();
        const names = Object.keys(map).sort();
        const cap = capture();
        const field = (name: string, part: "positive" | "negative" | "axis") => (
            <Row gap={2}>
                <TextInput value={(map[name]?.[part] ?? []).join(", ")} width={128} name={`action:${name}:${part}`} onChange={(v) => rebind(name, part, v)} />
                <Button label={cap && cap.name === name && cap.part === part ? "..." : "Press"} small primary={cap !== null && cap.name === name && cap.part === part} name={`action:${name}:${part}:press`} onClick={() => pressFor(name, part)} />
            </Row>
        );
        body = [
            <Row key="head" gap={8}>
                <box width={110}><Label text="action" size={SIZE.small} color={C.faint} /></box>
                <box width={150}><Label text="positive" size={SIZE.small} color={C.faint} /></box>
                <box width={150}><Label text="negative" size={SIZE.small} color={C.faint} /></box>
                <box width={150}><Label text="axis" size={SIZE.small} color={C.faint} /></box>
            </Row>,
            ...names.map((name) => (
                <Row key={name} gap={8} name={`action:${name}`}>
                    <box width={110}><Label text={name} size={SIZE.body} /></box>
                    {field(name, "positive")}
                    {field(name, "negative")}
                    {field(name, "axis")}
                    <Button label="Remove" small name={`action:${name}:remove`} onClick={() => removeAction(name)} />
                </Row>
            )),
            <Row key="new" gap={8}>
                <TextInput value={newAction()} placeholder="new action" width={110} name="action:new" onInput={(v) => newAction.set(v)} onChange={(v) => newAction.set(v)} />
                <TextInput value={newKeys()} placeholder="its keys" width={150} name="action:new:keys" onInput={(v) => newKeys.set(v)} onChange={(v) => { newKeys.set(v); addAction(newAction(), v); }} />
                <Button label="Add" small name="action:add" onClick={() => addAction(newAction(), newKeys())} />
                <Button label="Save bindings" small name="save_bindings" onClick={saveBindings} />
                <Label text="Keys are SDL names (Space, Left, A), pad:a, pad:leftx, mouse:x; commas between them. Saved to input.json in the project." muted size={SIZE.body} wrap flex={1} />
            </Row>,
        ];
    } else if (t === "audio") {
        const pct = (v: number) => `${Math.round(v * 100)}%`;
        const m = master();
        body = [
            <Row key="master" gap={8} name="bus:master">
                <box width={90}><Label text="master" size={SIZE.body} /></box>
                <Slider value={m.master_volume} max={2} step={0.05} width={150} name="master:volume" onInput={(v) => { command("audio.master", { volume: v }); refreshMixer(); }} />
                <box width={40}><Label text={pct(m.master_volume)} muted size={SIZE.body} /></box>
                <Checkbox checked={m.muted} label="mute" name="master:muted" onChange={(c) => { command("audio.master", { muted: c }); refreshMixer(); }} />
                <Label text="The master is the player's (not saved); buses are the project's." muted size={SIZE.body} />
            </Row>,
            ...buses().map((b) => (
                <Row key={b.name} gap={8} name={`bus:${b.name}`}>
                    <box width={90}><Label text={b.name} size={SIZE.body} /></box>
                    <Slider value={b.volume} max={2} step={0.05} width={150} name={`bus:${b.name}:volume`} onInput={(v) => setBus(b.name, { volume: v })} />
                    <box width={40}><Label text={pct(b.volume)} muted size={SIZE.body} /></box>
                    <Checkbox checked={b.muted} label="mute" name={`bus:${b.name}:muted`} onChange={(c) => setBus(b.name, { muted: c })} />
                    <Label text="low-pass" muted size={SIZE.body} />
                    <Slider value={b.lowpass} step={0.05} width={80} name={`bus:${b.name}:lowpass`} onInput={(v) => setBus(b.name, { lowpass: v })} />
                    <Label text="high-pass" muted size={SIZE.body} />
                    <Slider value={b.highpass} step={0.05} width={80} name={`bus:${b.name}:highpass`} onInput={(v) => setBus(b.name, { highpass: v })} />
                    <Label text={b.echo > 0 ? `echo ${b.echo.toFixed(2)} s` : "echo"} muted size={SIZE.body} />
                    <Slider value={b.echo} max={1} step={0.05} width={80} name={`bus:${b.name}:echo`} onInput={(v) => setBus(b.name, { echo: v })} />
                    <Label text="room" muted size={SIZE.body} />
                    <Slider value={b.reverb} max={2} step={0.1} width={60} name={`bus:${b.name}:reverb`} onInput={(v) => setBus(b.name, { reverb: v })} />
                    <Label text={`${b.voices} voice${b.voices === 1 ? "" : "s"}${b.duck_by ? ` · ducks under ${b.duck_by} to ${pct(b.duck_amount)} in ${b.duck_seconds} s${b.ducked ? `, now ${pct(b.duck)}` : ""}` : ""}`} muted size={SIZE.body} />
                </Row>
            )),
            <Row key="save" gap={8}>
                <Button label="Refresh" small name="mixer:refresh" onClick={refreshMixer} />
                <Button label="Save mixer" small name="mixer:save" onClick={saveMixer} />
                <Label text="Changes are heard at once. A bus appears when a voice plays on it or project.toml [audio.buses] names it; Save writes audio.json beside project.toml." muted size={SIZE.body} wrap flex={1} />
            </Row>,
        ];
    } else if (t === "assets") {
        const list = assetRows();
        const picked = assetPick();
        body = [
            <Row key="hint" gap={8} align="center">
                <Label text={picked ? `${picked}: ${assetInfo()}` : `${list.length === 0 ? "No files under assets/; the built-in meshes are below. " : ""}Click one to describe it; drag it onto the scene, or pick it and Place, to put it there.`} muted size={SIZE.body} name="asset-info" wrap flex={1} />
                {picked ? <Button label="Place" small name="asset:place" onClick={placePicked} /> : null}
                {picked && list.find((r) => r.path === picked)?.importer ? <Button label="Reimport" small name="asset:reimport" onClick={reimportPicked} /> : null}
            </Row>,
            ...[...list, ...builtinRows].map((r) => {
                const on = picked === r.path;
                return (
                    <box key={r.path} name={`asset:${r.path}`} direction="row" align="center" height={ROW} padding={[0, S.md]} gap={S.lg} radius={4} background={on ? C.selected : hovered() === `asset:${r.path}` ? C.rowHover : null}
                        onClick={() => pickAsset(r)} onDrag={() => undefined} onDragEnd={(e) => placeAsset(r, e)} onHover={hoverOn(`asset:${r.path}`)}>
                        {r.kind === "image" ? <box width={16} height={16} image={r.path} name={`thumb:${r.path}`} /> : null}
                        {r.kind === "mesh" && thumbFor(r.path) ? <box width={16} height={16} image={thumbFor(r.path)} name={`thumb:${r.path}`} /> : null}
                        <Label text={r.path} size={SIZE.body} color={on ? "#ffffff" : C.text} />
                        <Label text={r.kind} size={SIZE.body} color={on ? C.accentText : C.dim} />
                        {r.importer && r.importer !== "gltf" ? <Label text={r.importer === "blender" ? "via Blender" : r.importer.toUpperCase()} size={SIZE.body} color={on ? C.accentText : C.dim} name={`importer:${r.path}`} /> : null}
                        <Label text={r.builtin ? "built in" : r.bytes >= 1048576 ? `${(r.bytes / 1048576).toFixed(1)} MB` : r.bytes >= 1024 ? `${(r.bytes / 1024).toFixed(1)} KB` : `${r.bytes} B`} size={SIZE.body} color={on ? C.accentText : C.faint} />
                        {r.loaded ? <Label text="loaded" size={SIZE.body} color={C.ok} /> : null}
                    </box>
                );
            }),
        ];
    } else if (t === "script") {
        const list = assetRows().filter((r) => r.kind === "script");
        const path = scriptPath();
        body = [
            <box key="script" direction="row" gap={8} flex={1}>
                <box direction="column" width={220} gap={1} overflow="scroll">
                    {list.length === 0 ? <Label text="No scripts under scripts/ or scenarios/." muted size={SIZE.body} wrap /> : list.map((r) => (
                        <box key={r.path} name={`script:${r.path}`} direction="row" align="center" height={ROW} padding={[0, S.md]} radius={4} background={path === r.path ? C.selected : hovered() === `script:${r.path}` ? C.rowHover : null}
                            onClick={() => openScript(r.path)} onHover={hoverOn(`script:${r.path}`)}>
                            <Label text={r.path} size={SIZE.body} color={path === r.path ? "#ffffff" : C.text} />
                        </box>
                    ))}
                </box>
                <box direction="column" flex={1} gap={4}>
                    <Row gap={8} align="center">
                        <Label text={path ? (scriptDirty() ? `${path} (modified)` : path) : "Open a script from the list."} size={SIZE.body} name="script-path" />
                        <Button label="Save" small primary={scriptDirty()} name="script:save" onClick={saveScript} />
                        <Label text="Cmd/Ctrl+Return saves. pocket editor --watch rebuilds and reloads the project after a save." muted size={SIZE.body} wrap flex={1} />
                    </Row>
                    <TextInput multiline flex={1} name="script:text" value={scriptText()} syntax={path || undefined} disabled={!path} padding={[S.sm, S.lg]} onInput={(v) => { scriptText.set(v); scriptDirty.set(true); }} onChange={(v) => { scriptText.set(v); saveScript(); }} />
                    {typeErrors().length > 0 ? (
                        <box direction="column" gap={1} name="script:errors">
                            {typeErrors().slice(0, 6).map((d, i) => (
                                <Label key={i} text={`${d.file ?? ""}${d.line !== undefined ? `:${d.line}:${d.column ?? 0}` : ""}  ${d.message.split("\n")[0]}`} size={SIZE.body} color={d.file === path ? C.danger : C.dim} />
                            ))}
                            {typeErrors().length > 6 ? <Label text={`and ${typeErrors().length - 6} more (the Console lists them all)`} muted size={SIZE.body} /> : null}
                        </box>
                    ) : null}
                </box>
            </box>,
        ];
    } else {
        const text = transcriptText();
        body = text.trim() === "" ? [<Empty key="empty" title="Nothing happened yet" hint="The transcript tells what the game did, tick by tick, once it plays." />] : text.split("\n").map((line, i) => <Label key={i} text={line} size={SIZE.body} />);
    }
    return body as VNode[];
}

/** A pane's tab label and what it adds in fainter type: the hierarchy counts its entities, the
 * inspector names what it shows. */
function paneLabel(p: Pane): [string, string] {
    if (p === "hierarchy") return ["Hierarchy", String(rows().length)];
    if (p === "inspector") {
        const d = described();
        const extra = selection().length - 1;
        return ["Inspector", d ? `${d.name}${extra > 0 ? ` (+${extra} more)` : ""}` : ""];
    }
    return [PANE_LABELS[p], ""];
}

/** A dock's tab: click shows its pane, a drag onto another dock (or near an edge of the scene) moves
 * it there. The tab in front takes its pane's colour under an accent line, so it joins the pane. */
function DockTab(props: { pane: Pane; active: boolean }) {
    const p = props.pane;
    const [title, detail] = paneLabel(p);
    const hot = hovered() === `tab:${p}`;
    return (
        <box name={`tab:${p}`} direction="row" align="center" gap={S.md} height={STRIP} padding={[0, S.lg + 2]} background={props.active ? C.pane : hot ? C.rowHover : null}
            onClick={() => showPane(p)} onDrag={() => undefined} onHover={hoverOn(`tab:${p}`)}
            onDragEnd={(e) => { const to = e.x !== undefined && e.y !== undefined ? dockAt(e.x, e.y) : null; if (to) movePane(p, to); }}>
            {props.active ? <box position="absolute" left={0} top={0} right={0} height={2} background={C.accent} /> : null}
            <Label text={title} size={SIZE.body} color={props.active || hot ? C.text : C.dim} />
            {detail ? <Label text={detail} size={SIZE.small} color={props.active ? C.dim : C.faint} /> : null}
        </box>
    );
}

/** A pane's own content at a dock's width; `grow` fills the height of a dock with tabs above it. */
function PaneView(p: Pane, width: Dim, grow = false): VNode {
    if (p === "hierarchy") return <Hierarchy width={width} grow={grow} />;
    if (p === "inspector") return <Inspector width={width} grow={grow} />;
    return (
        <box width={width} flex={grow ? 1 : undefined} direction="column" background={C.pane} name={`pane:${p}`}>
            <box flex={1} overflow="scroll" padding={[S.md, S.lg + 2]} gap={2} name={`${p}-body`}>
                {TabBody(p)}
            </box>
        </box>
    );
}

/** The left or right dock: its tabs above the pane in front (none when it holds no pane). */
function SideDock(props: { dock: "left" | "right" }) {
    const l = layout();
    const panes = l.docks[props.dock];
    const active = l.active[props.dock];
    if (panes.length === 0 || active === "") return null;
    const width = props.dock === "left" ? l.hierarchy : l.inspector;
    return (
        <box width={width} direction="column" name={`dock:${props.dock}`}>
            <Row gap={0} wrap background={C.strip}>
                {panes.map((p) => <DockTab key={p} pane={p} active={p === active} />)}
            </Row>
            {PaneView(active, "100%", true)}
        </box>
    );
}

/** The bottom dock: its tabs on a strip, the pane in front below (just the strip when it holds none). */
function Bottom() {
    const l = layout();
    const panes = l.docks.bottom;
    const active = l.active.bottom;
    return (
        <box height={panes.length > 0 ? l.bottom : undefined} direction="column" background={C.pane} name="bottom">
            <Row gap={0} wrap background={C.strip}>
                {panes.map((p) => <DockTab key={p} pane={p} active={p === active} />)}
            </Row>
            {active === "" ? null : active === "hierarchy" || active === "inspector" ? PaneView(active, "100%", true) : (
                <box flex={1} overflow="scroll" padding={[S.md, S.lg + 2]} gap={2} name="bottom-body">
                    {TabBody(active)}
                </box>
            )}
        </box>
    );
}

/** The status bar under the docks: whether the project is being edited, plays or is paused (the bar
 * takes a tint while it plays), the last notice, then the tick, the entity count and the state hash. */
function StatusBar() {
    const st = status();
    const mode = !playing() ? "Edit mode" : paused() ? "Paused" : "Playing";
    const tint = !playing() ? C.window : paused() ? "#2b2414" : "#132a1c";
    const dot = !playing() ? C.faint : paused() ? C.warn : C.ok;
    return (
        <Row height={22} gap={S.lg} padding={[0, S.lg + 2]} background={tint} name="statusbar">
            <box width={7} height={7} radius={4} background={dot} />
            <Label text={mode} size={SIZE.small} color={C.text} name="mode" />
            <box flexGrow={1} flexShrink={1} flexBasis={0} overflow="hidden">
                <Label text={notice()} size={SIZE.small} color={C.dim} name="notice" />
            </box>
            <Label text={`tick ${st.tick}`} size={SIZE.small} color={C.dim} name="tick" />
            <Label text={`${st.entities} entities`} size={SIZE.small} color={C.dim} name="entities" />
            <Label text={st.hash} size={SIZE.small} color={C.faint} name="hash" />
        </Row>
    );
}

/** The gizmo: absolute children of the main row, painted over the scene pane. Move handles at
 * the axis tips and the center; turns about Y, X and Z below to the left (R, RX, RZ); scales,
 * uniform and per axis, below to the right (S, SX, SY, SZ). */
/** The view bar over the scene pane's top-right corner (a project's HUD tends to take the top
 * left): what the pane shows and how drags land. */
function ViewBar() {
    status();   // placed from the pane's rectangle, which settles over the first frames
    if (viewportRect.w === 0 || mainRect.w === 0) return null;
    return (
        <box position="absolute" right={mainRect.x + mainRect.w - (viewportRect.x + viewportRect.w) + S.lg} top={S.lg} direction="row" align="center" gap={S.xs} padding={3} radius={6} background="#121317e0" borderColor={C.line} border={1} name="viewbar">
            <Button label={overlays() ? "✓ Overlays" : "Overlays"} small ghost on={overlays()} name="overlays" onClick={toggleOverlays} />
            <Sep height={14} />
            <Button label={snap() ? "✓ Snap" : "Snap"} small ghost on={snap()} name="snap" onClick={toggleSnap} />
            <Button label={`${snapStep()}`} small ghost name="snap_step" onClick={cycleSnapStep} />
            <Sep height={14} />
            <Button label={localAxes() ? "Local" : "World"} small ghost on={localAxes()} name="axes" onClick={toggleLocal} />
        </box>
    );
}

function GizmoHandles() {
    const g = gizmo();
    if (!g) return null;
    const handle = (axis: Axis, p: { x: number; y: number }, color: string, label: string) => (
        <box key={axis} name={`gizmo:${axis}`} position="absolute" left={p.x - 9} top={p.y - 9} width={18} height={18} radius={axis === "plane" ? 4 : 9} background={color} borderColor={hovered() === `gizmo:${axis}` ? "#ffffff" : "#0b0c0f"} border={hovered() === `gizmo:${axis}` ? 2 : 1} justify="center" align="center"
            onMouseDown={() => gizmoDown(axis)} onDrag={gizmoDrag} onDragEnd={gizmoEnd} onHover={hoverOn(`gizmo:${axis}`)}>
            <Label text={label} size={label.length > 1 ? 8 : 10} weight="bold" color="#101114" />
        </box>
    );
    // The turn and scale handles sit at fixed offsets from the center; one is pushed further out
    // along its own offset when an axis tip would land on it (the tips move with the camera, and
    // with Local on they follow the entity's turn).
    const clear = (ox: number, oy: number) => {
        let p = { x: g.center.x + ox, y: g.center.y + oy };
        for (let i = 0; i < 4; i++) {
            const near = [g.x, g.y, g.z].some((t) => Math.hypot(t.x - p.x, t.y - p.y) < 20);
            if (!near) break;
            p = { x: p.x + Math.sign(ox) * 24, y: p.y + Math.sign(oy) * 24 };
        }
        return p;
    };
    return [
        handle("plane", g.center, "#eef0f4", "+"),
        handle("x", g.x, AXIS_COLORS.x, "X"),
        handle("y", g.y, AXIS_COLORS.y, "Y"),
        handle("z", g.z, AXIS_COLORS.z, "Z"),
        handle("rotate", clear(-30, 30), "#f0a030", "R"),
        handle("rotate_x", clear(-54, 30), "#f0a030", "RX"),
        handle("rotate_z", clear(-30, 54), "#f0a030", "RZ"),
        handle("scale", clear(30, 30), "#c080f0", "S"),
        handle("scale_x", clear(54, 30), "#c080f0", "SX"),
        handle("scale_y", clear(30, 54), "#c080f0", "SY"),
        handle("scale_z", clear(54, 54), "#c080f0", "SZ"),
    ];
}

/** A gap between docks in the window's colour that takes the accent under the pointer; dragged, it resizes them. */
function Splitter(props: { name: string; vertical?: boolean; onDrag: (e: UiEvent) => void }) {
    const hot = hovered() === props.name;
    return <box name={props.name} width={props.vertical ? undefined : GAP} height={props.vertical ? GAP : undefined} background={hot ? C.accent : C.window} onDrag={props.onDrag} onDragEnd={saveLayout} onHover={hoverOn(props.name)} />;
}

function Editor() {
    return (
        <box width="100%" height="100%" direction="column" name="editor">
            <Toolbar />
            <Row flex={1} gap={0} align="stretch" name="main">
                <SideDock dock="left" />
                {layout().docks.left.length > 0 ? <Splitter name="split:hierarchy" onDrag={(e) => layout.update((l) => ({ ...l, hierarchy: clamp(l.hierarchy + (e.dx ?? 0), 120, 600) }))} /> : null}
                <box flex={1} name="viewport" onMouseDown={onViewportDown} onMouseUp={onViewportUp} onDrag={onViewportDrag} onDragEnd={onViewportUp} onWheel={onViewportWheel} overflow="hidden" />
                {layout().docks.right.length > 0 ? <Splitter name="split:inspector" onDrag={(e) => layout.update((l) => ({ ...l, inspector: clamp(l.inspector - (e.dx ?? 0), 160, 700) }))} /> : null}
                <SideDock dock="right" />
                {GizmoHandles()}
                {ViewBar()}
            </Row>
            {layout().docks.bottom.length > 0 ? <Splitter name="split:bottom" vertical onDrag={(e) => layout.update((l) => ({ ...l, bottom: clamp(l.bottom - (e.dy ?? 0), 60, 600) }))} /> : null}
            <Bottom />
            <StatusBar />
        </box>
    );
}

loadLayout();
mount(() => <Editor />);
refreshHierarchy();
refreshBottom();
