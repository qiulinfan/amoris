// The Pocket editor: a TSX program on Pocket UI that runs beside a project in the same runtime.
// It sees the world through the same commands scripts and agents use, so everything shown here
// (hierarchy, inspector, console, transcript) is also reachable by `ui_snapshot`, and every
// button is reachable by `ui_click`. Every edit is undoable (editor/history.ts) and the scene
// pane has a translate gizmo (editor/gizmo.ts) that agents drag with `ui.drag`.
import { Button, Checkbox, Label, Panel, Row, Slider, TextInput, command, mount, onFrame, onInput, physics, render, setProjectRoot, signal, terrain, theme, tilemap, ui, world } from "pocket";
import type { Bus, ComponentName, Described, Dim, Scene, Transform, UiEvent, VNode, WorldEvent } from "pocket";
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
interface SchemaField { name: string; type: string; doc: string }
interface SchemaComponent { name: string; doc: string; serialized: boolean; fields: SchemaField[]; default?: Record<string, unknown> }
// The panes live in three docks (left, right, bottom), each showing one of its panes at a time
// behind tabs; a tab dragged onto another dock moves its pane there. The widths keep their first
// names: `hierarchy` is the left dock's, `inspector` the right's.
interface Layout { hierarchy: number; inspector: number; bottom: number; docks: Record<Dock, Pane[]>; active: Record<Dock, Pane | "">; }
interface AssetRow { path: string; kind: "mesh" | "image" | "tilemap" | "audio" | "script" | "material" | "other"; bytes: number; loaded: boolean; importer?: "gltf" | "obj" | "stl" | "ply" | "blender" }
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
const DEFAULT_LAYOUT: Layout = { hierarchy: 240, inspector: 320, bottom: 200, docks: { left: ["hierarchy"], right: ["inspector"], bottom: [...TABS] }, active: { left: "hierarchy", right: "inspector", bottom: "console" } };

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
type SculptSettings = { mode: SculptMode; radius: number; strength: number; color: Rgb };
const sculpt = signal<SculptSettings | null>(null);   // terrain sculpting (or painting) in the scene pane while a Terrain is selected
const sculptSettings = signal<SculptSettings>({ mode: "raise", radius: 4, strength: 0.5, color: { r: 0.45, g: 0.35, b: 0.24 } });
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
let sculptStroke: { entity: number; paint: boolean; before: number[]; pos: { x: number; y: number }; target?: number; touches: number } | null = null;

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
        <box gap={4} padding={[4, 0]} name="tiles">
            <Row>
                <Label text="Tiles" size={13} />
                <box flex={1} />
                <Button label={b ? "Stop painting" : "Paint"} small primary={b !== null} name="paint" onClick={() => brush.set(b ? null : { layer, gid })} />
            </Row>
            <Row wrap gap={4}>
                {mapInfo.layers.map((l) => <Button key={l.name} label={`${l.name} (${l.tiles})`} small primary={l.name === layer} name={`layer:${l.name}`} onClick={() => brush.set({ layer: l.name, gid })} />)}
            </Row>
            <Row wrap gap={4}>
                <Button label="erase" small primary={gid === 0} name="tile:0" onClick={() => brush.set({ layer, gid: 0 })} />
                {tiles.map((g) => <Button key={g} label={String(g)} small primary={g === gid} name={`tile:${g}`} onClick={() => brush.set({ layer, gid: g })} />)}
            </Row>
            <Row gap={4}>
                <Label text={b ? `Click or drag in the scene: gid ${gid} on ${layer}. Revision ${mapInfo.revision}.` : `Revision ${mapInfo.revision}. Pick a layer and a tile to paint under the mouse.`} muted size={11} wrap flex={1} />
                <Button label="Save map" small name="save-map" onClick={() => saveMap(props.id)} />
            </Row>
        </box>
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
        if (b.mode === "paint" || b.mode === "erase") terrain.paint(hit.point.x, hit.point.z, b.mode === "paint" ? b.color : null, { entity: st.entity, radius: b.radius, amount: Math.min(b.strength * 0.5, 1) });
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
    if (st.paint) {
        const after = terrain.paints(st.entity).paint;
        history.record({ label: `paint the terrain (${touches})`, redo: () => { terrain.setPaints(after, st.entity); }, undo: () => { terrain.setPaints(before, st.entity); } });
    } else {
        const after = terrain.heights(st.entity).heights;
        history.record({ label: `sculpt the terrain (${touches})`, redo: () => { terrain.setHeights(after, st.entity); }, undo: () => { terrain.setHeights(before, st.entity); } });
    }
    historyVersion.update((v) => v + 1);
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
        <box gap={4} padding={[4, 0]} name="terrain-brush">
            <Row>
                <Label text="Sculpt" size={13} />
                <box flex={1} />
                <Button label={b ? "Stop sculpting" : "Sculpt"} small primary={b !== null} name="sculpt" onClick={() => sculpt.set(b ? null : set)} />
            </Row>
            <Row wrap gap={4}>
                {(["raise", "lower", "flatten", "smooth", "paint", "erase"] as const).map((m) => <Button key={m} label={m} small primary={set.mode === m} name={`sculpt:${m}`} onClick={() => pick({ mode: m })} />)}
            </Row>
            {set.mode === "paint" ? (
                <Row wrap gap={4} align="center">
                    <box width={18} height={18} radius={3} border={1} borderColor={theme.border} background={[set.color.r, set.color.g, set.color.b]} name="paint:swatch" />
                    {PAINTS.map(([label, c]) => <Button key={label} label={label} small primary={set.color.r === c.r && set.color.g === c.g && set.color.b === c.b} name={`paint:${label}`} onClick={() => pick({ color: c })} />)}
                </Row>
            ) : null}
            <Row gap={6}>
                <box width={56}><Label text="radius" muted size={12} /></box>
                <Slider value={set.radius} min={0.5} max={16} step={0.5} width={140} name="sculpt:radius" onInput={(v) => pick({ radius: v })} />
                <Label text={`${set.radius}`} muted size={12} />
            </Row>
            <Row gap={6}>
                <box width={56}><Label text="strength" muted size={12} /></box>
                <Slider value={set.strength} min={0.05} max={2} step={0.05} width={140} name="sculpt:strength" onInput={(v) => pick({ strength: v })} />
                <Label text={`${set.strength}`} muted size={12} />
            </Row>
            <Row gap={4}>
                <Label text={`${info.source === "noise" ? "Noise" : info.source}${info.edited ? ", sculpted" : ""}; ${info.resolution} by ${info.resolution}, ${info.lowest.toFixed(1)} to ${info.highest.toFixed(1)} high${info.painted > 0 ? `, ${Math.round(info.painted * 100)}% painted` : ""}. ${b ? "Click or drag on the ground." : ""}`} muted size={11} wrap flex={1} />
            </Row>
            <Row gap={4}>
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
        </box>
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
            const via = d.importer === "blender" ? `; read through Blender, kept as ${String(d.converted)}` : d.importer === "obj" ? "; an OBJ" : d.importer === "stl" ? "; an STL" : d.importer === "ply" ? "; a PLY" : "";
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
    const row = assetRows().find((r) => r.path === assetPick());
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
        sculptStroke = { entity: shaping, paint, before: paint ? terrain.paints(shaping).paint : terrain.heights(shaping).heights, pos: { x: e.x ?? 0, y: e.y ?? 0 }, touches: 0 };
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

function Toolbar() {
    const s = status();
    historyVersion();
    return (
        <Row padding={[6, 10]} gap={8} name="toolbar" height={40}>
            <Label text="Pocket" size={15} />
            <Label text={info.name} muted />
            <box width={12} />
            {paused() ? <Button label="Play" primary name="play" onClick={play} /> : <Button label="Pause" name="pause" onClick={pause} />}
            <Button label="Step" name="step" onClick={step} disabled={!paused()} />
            <Button label="Stop" danger name="stop" onClick={stop} disabled={!playing()} />
            <box width={12} />
            <Button label="Undo" name="undo" onClick={doUndo} disabled={!history.canUndo()} />
            <Button label="Redo" name="redo" onClick={doRedo} disabled={!history.canRedo()} />
            <box width={12} />
            <Button label="Save" name="save" onClick={saveScene} disabled={!info.scene} />
            <Button label="Spawn" name="spawn" onClick={spawnEntity} />
            <Button label="Clone" name="duplicate" onClick={duplicateSelected} disabled={selection().length === 0} />
            <Button label="Delete" name="delete" onClick={deleteSelected} disabled={selection().length === 0} />
            <box flex={1} />
        </Row>
    );
}

function Hierarchy(props: { width: Dim; grow?: boolean }) {
    const list = rows();
    const sel = selection();
    return (
        <Panel width={props.width} flex={props.grow ? 1 : undefined} scroll name="hierarchy" padding={2} gap={0}>
            {list.length === 0 ? <Label text="No entities. Press Play or Spawn." muted wrap /> : null}
            {list.map((r) => (
                <box key={r.id} onClick={(e) => select(r.id, e.mods?.includes("shift") || e.mods?.includes("meta") || e.mods?.includes("ctrl") ? "toggle" : "replace")} onDrag={() => undefined} onDragEnd={(e) => dropRow(r.id, e)} padding={[3, 6]} radius={3} background={sel.includes(r.id) ? theme.accent : null} name={`entity:${r.name}`}>
                    <Label text={`${"  ".repeat(r.depth)}${r.name}`} color={sel.includes(r.id) ? theme.accentText : theme.text} />
                </box>
            ))}
        </Panel>
    );
}

function fieldInputs(entity: number, comp: ComponentName, field: SchemaField, value: unknown) {
    if (Array.isArray(value)) {
        // A list field (records): edited as JSON, the whole array at once.
        return (
            <Row gap={4} key={field.name}>
                <Label text={`${field.name} (${value.length})`} muted />
                <box flex={1} />
                <TextInput value={JSON.stringify(value)} width={180} name={`${comp}.${field.name}`} onChange={(v) => setField(entity, comp, field.name, null, v, "json")} />
            </Row>
        );
    }
    if (value !== null && typeof value === "object") {
        const obj = value as Record<string, unknown>;
        return (
            <Row gap={4} key={field.name}>
                <Label text={field.name} muted />
                <box flex={1} />
                {Object.keys(obj).map((k) => (
                    <Row gap={2} key={k}>
                        <Label text={k} muted size={11} />
                        <TextInput value={formatNumber(obj[k])} width={58} name={`${comp}.${field.name}.${k}`} onChange={(v) => setField(entity, comp, field.name, k, v, "float")} />
                    </Row>
                ))}
            </Row>
        );
    }
    if (typeof value === "boolean") {
        return (
            <Row gap={4} key={field.name}>
                <Label text={field.name} muted />
                <box flex={1} />
                <Button label={value ? "true" : "false"} small name={`${comp}.${field.name}`} onClick={() => setField(entity, comp, field.name, null, value ? "false" : "true", "bool")} />
            </Row>
        );
    }
    return (
        <Row gap={4} key={field.name}>
            <Label text={field.name} muted />
            <box flex={1} />
            <TextInput value={typeof value === "number" ? formatNumber(value) : String(value ?? "")} width={120} name={`${comp}.${field.name}`} onChange={(v) => setField(entity, comp, field.name, null, v, typeof value === "number" ? "float" : "string")} />
        </Row>
    );
}

function formatNumber(v: unknown): string {
    if (typeof v !== "number") return String(v ?? "");
    return Number.isInteger(v) ? String(v) : v.toFixed(3).replace(/\.?0+$/, "");
}

function Inspector(props: { width: Dim; grow?: boolean }) {
    const d = described();
    const extra = selection().length - 1;
    if (!d) {
        return (
            <Panel width={props.width} flex={props.grow ? 1 : undefined} name="inspector">
                <Label text="Select an entity in the hierarchy or click it in the scene. Shift-click adds to the selection." muted wrap />
            </Panel>
        );
    }
    const present = Object.keys(d.components);
    const missing = schema.filter((c) => c.serialized && !present.includes(c.name));
    return (
        <Panel width={props.width} flex={props.grow ? 1 : undefined} scroll name="inspector" gap={8}>
            <Row gap={4}>
                <Label text="name" muted />
                <TextInput value={d.name} flex={1} name="entity-name" onChange={(v) => { if (v.length > 0) renameEntity(d.id, v); }} />
            </Row>
            <Label text={`${d.path}  #${d.id}`} muted size={11} />
            {present.includes("TileMap") ? <TileBrush id={d.id} /> : null}
            {present.includes("Terrain") ? <TerrainBrush id={d.id} /> : null}
            {schema.filter((c) => present.includes(c.name)).map((c) => (
                <box key={c.name} gap={4} padding={[4, 0]} borderColor={theme.border} border={0}>
                    <Row>
                        <Label text={c.name} size={13} />
                        <box flex={1} />
                        {c.serialized ? <Button label="remove" small name={`remove:${c.name}`} onClick={() => removeComponent(d.id, c.name as ComponentName)} /> : <Label text="derived" muted size={11} />}
                    </Row>
                    {c.fields.map((f) => fieldInputs(d.id, c.name as ComponentName, f, (d.components as Record<string, Record<string, unknown>>)[c.name]?.[f.name]))}
                </box>
            ))}
            {addingComponent() ? (
                <box gap={2}>
                    <Label text="Add component" muted />
                    <Row wrap gap={4}>
                        {missing.map((c) => <Button key={c.name} label={c.name} small name={`add:${c.name}`} onClick={() => { addComponent(d.id, c.name as ComponentName); addingComponent.set(false); }} />)}
                        <Button label="cancel" small onClick={() => addingComponent.set(false)} />
                    </Row>
                </box>
            ) : (
                <Button label="Add component" small name="add-component" onClick={() => addingComponent.set(true)} disabled={missing.length === 0} />
            )}
        </Panel>
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
            <Label key="hint" text={owners.length > 0 ? "Select an entity with a Timeline:" : "No entity has a Timeline yet: add one in the inspector and name its file."} muted size={12} />,
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
                <Label text={path ? `${path} is not in the project yet.` : "This Timeline names no file."} muted size={12} />
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
            <Label text={`${path}, ${duration.toFixed(2)} s`} size={12} />
            <Button label={tl.playing ? "Stop" : "Play"} small primary={tl.playing} name="tl:play" onClick={() => { command(tl.playing ? "timeline.stop" : "timeline.play", tl.playing ? { entity: id } : { entity: id, path, time: tl.time }); timelineVersion.update((n) => n + 1); }} />
            <Slider value={tl.time} min={0} max={duration} step={duration / 400} width={TIMELINE_WIDTH - 60} name="tl:time" onInput={seek} />
            <Label text={`${tl.time.toFixed(2)} s`} muted size={12} name="tl:now" />
        </Row>,
        ...tracks.map((tr, i) => (
            <Row key={`track${i}`} gap={8} name={`tl:track:${i}`}>
                <box width={190}><Label text={`${tr.entity && tr.entity !== "." ? tr.entity : "(this)"} ${tr.component}.${tr.field}`} size={12} /></box>
                <box width={TIMELINE_WIDTH + 8} height={16} background={theme.panelAlt} radius={3}>
                    {tr.keys.map((k, n) => (
                        <box key={n} name={`tl:key:${i}:${n}`} position="absolute" left={x(keyTime(k))} top={2} width={8} height={12} radius={2}
                            background={picked && picked.track === i && picked.key === n ? "#f0c060" : theme.accent} onClick={() => timelineKey.set({ path, track: i, key: n })} />
                    ))}
                    <box position="absolute" left={x(tl.time) + 3} top={0} width={2} height={16} background="#ff5050" />
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
                <Label text={`key at ${keyTime(tracks[picked.track].keys[picked.key]).toFixed(2)} s: ${JSON.stringify(Array.isArray(tracks[picked.track].keys[picked.key]) ? (tracks[picked.track].keys[picked.key] as unknown[]).slice(1) : tracks[picked.track].keys[picked.key])}`} size={12} />
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
            <Label text="A new track starts with the field's value now at the playhead; Key adds the value now to a track." muted size={11} wrap flex={1} />
        </Row>,
    ];
    return out.filter((n): n is VNode => n !== null);
}

function TabBody(t: Tab): VNode[] {
    let body;
    if (t === "timeline") {
        body = TimelineBody();
    } else if (t === "console") {
        body = logs().map((l) => <Label key={l.seq} text={`${l.tick !== undefined ? `[${l.tick}] ` : ""}${l.level} ${l.cat}: ${l.msg}`} size={12} color={l.level === "error" ? theme.danger : l.level === "warn" ? "#f0c060" : theme.text} />);
    } else if (t === "events") {
        body = recentEvents().map((e) => <Label key={e.seq} text={`#${e.seq} t${e.tick} ${e.type}${e.subject ? ` @${e.subject}` : ""}${e.cause ? ` <- #${e.cause}` : ""} ${e.data ? JSON.stringify(e.data) : ""}`} size={12} />);
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
                <box width={110}><Label text="action" muted size={12} /></box>
                <box width={150}><Label text="positive" muted size={12} /></box>
                <box width={150}><Label text="negative" muted size={12} /></box>
                <box width={150}><Label text="axis" muted size={12} /></box>
            </Row>,
            ...names.map((name) => (
                <Row key={name} gap={8} name={`action:${name}`}>
                    <box width={110}><Label text={name} size={12} /></box>
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
                <Label text="Keys are SDL names (Space, Left, A), pad:a, pad:leftx, mouse:x; commas between them. Saved to input.json in the project." muted size={12} wrap flex={1} />
            </Row>,
        ];
    } else if (t === "audio") {
        const pct = (v: number) => `${Math.round(v * 100)}%`;
        const m = master();
        body = [
            <Row key="master" gap={8} name="bus:master">
                <box width={90}><Label text="master" size={12} /></box>
                <Slider value={m.master_volume} max={2} step={0.05} width={150} name="master:volume" onInput={(v) => { command("audio.master", { volume: v }); refreshMixer(); }} />
                <box width={40}><Label text={pct(m.master_volume)} muted size={12} /></box>
                <Checkbox checked={m.muted} label="mute" name="master:muted" onChange={(c) => { command("audio.master", { muted: c }); refreshMixer(); }} />
                <Label text="The master is the player's (not saved); buses are the project's." muted size={12} />
            </Row>,
            ...buses().map((b) => (
                <Row key={b.name} gap={8} name={`bus:${b.name}`}>
                    <box width={90}><Label text={b.name} size={12} /></box>
                    <Slider value={b.volume} max={2} step={0.05} width={150} name={`bus:${b.name}:volume`} onInput={(v) => setBus(b.name, { volume: v })} />
                    <box width={40}><Label text={pct(b.volume)} muted size={12} /></box>
                    <Checkbox checked={b.muted} label="mute" name={`bus:${b.name}:muted`} onChange={(c) => setBus(b.name, { muted: c })} />
                    <Label text="low-pass" muted size={12} />
                    <Slider value={b.lowpass} step={0.05} width={80} name={`bus:${b.name}:lowpass`} onInput={(v) => setBus(b.name, { lowpass: v })} />
                    <Label text="high-pass" muted size={12} />
                    <Slider value={b.highpass} step={0.05} width={80} name={`bus:${b.name}:highpass`} onInput={(v) => setBus(b.name, { highpass: v })} />
                    <Label text={b.echo > 0 ? `echo ${b.echo.toFixed(2)} s` : "echo"} muted size={12} />
                    <Slider value={b.echo} max={1} step={0.05} width={80} name={`bus:${b.name}:echo`} onInput={(v) => setBus(b.name, { echo: v })} />
                    <Label text="room" muted size={12} />
                    <Slider value={b.reverb} max={2} step={0.1} width={60} name={`bus:${b.name}:reverb`} onInput={(v) => setBus(b.name, { reverb: v })} />
                    <Label text={`${b.voices} voice${b.voices === 1 ? "" : "s"}${b.duck_by ? ` · ducks under ${b.duck_by} to ${pct(b.duck_amount)} in ${b.duck_seconds} s${b.ducked ? `, now ${pct(b.duck)}` : ""}` : ""}`} muted size={12} />
                </Row>
            )),
            <Row key="save" gap={8}>
                <Button label="Refresh" small name="mixer:refresh" onClick={refreshMixer} />
                <Button label="Save mixer" small name="mixer:save" onClick={saveMixer} />
                <Label text="Changes are heard at once. A bus appears when a voice plays on it or project.toml [audio.buses] names it; Save writes audio.json beside project.toml." muted size={12} wrap flex={1} />
            </Row>,
        ];
    } else if (t === "assets") {
        const list = assetRows();
        const picked = assetPick();
        body = [
            <Row key="hint" gap={8} align="center">
                <Label text={list.length === 0 ? "No files under assets/." : picked ? `${picked}: ${assetInfo()}` : "Click a file to describe it; drag one onto the scene, or pick it and Place, to put it there."} muted size={12} name="asset-info" wrap flex={1} />
                {picked ? <Button label="Place" small name="asset:place" onClick={placePicked} /> : null}
                {picked && list.find((r) => r.path === picked)?.importer ? <Button label="Reimport" small name="asset:reimport" onClick={reimportPicked} /> : null}
            </Row>,
            ...list.map((r) => (
                <box key={r.path} name={`asset:${r.path}`} direction="row" align="center" padding={[2, 6]} gap={8} radius={3} background={picked === r.path ? theme.accent : null} onClick={() => pickAsset(r)} onDrag={() => undefined} onDragEnd={(e) => placeAsset(r, e)}>
                    {r.kind === "image" ? <box width={16} height={16} image={r.path} name={`thumb:${r.path}`} /> : null}
                    {r.kind === "mesh" && thumbFor(r.path) ? <box width={16} height={16} image={thumbFor(r.path)} name={`thumb:${r.path}`} /> : null}
                    <Label text={r.path} size={12} color={picked === r.path ? theme.accentText : theme.text} />
                    <Label text={r.kind} size={12} color={picked === r.path ? theme.accentText : theme.muted} />
                    {r.importer && r.importer !== "gltf" ? <Label text={r.importer === "blender" ? "via Blender" : r.importer.toUpperCase()} size={12} color={picked === r.path ? theme.accentText : theme.muted} name={`importer:${r.path}`} /> : null}
                    <Label text={r.bytes >= 1048576 ? `${(r.bytes / 1048576).toFixed(1)} MB` : r.bytes >= 1024 ? `${(r.bytes / 1024).toFixed(1)} KB` : `${r.bytes} B`} size={12} color={picked === r.path ? theme.accentText : theme.muted} />
                    {r.loaded ? <Label text="loaded" size={12} color={picked === r.path ? theme.accentText : theme.ok} /> : null}
                </box>
            )),
        ];
    } else if (t === "script") {
        const list = assetRows().filter((r) => r.kind === "script");
        const path = scriptPath();
        body = [
            <box key="script" direction="row" gap={8} flex={1}>
                <box direction="column" width={220} gap={1} overflow="scroll">
                    {list.length === 0 ? <Label text="No scripts under scripts/ or scenarios/." muted size={12} wrap /> : list.map((r) => (
                        <box key={r.path} name={`script:${r.path}`} padding={[2, 6]} radius={3} background={path === r.path ? theme.accent : null} onClick={() => openScript(r.path)}>
                            <Label text={r.path} size={12} color={path === r.path ? theme.accentText : theme.text} />
                        </box>
                    ))}
                </box>
                <box direction="column" flex={1} gap={4}>
                    <Row gap={8} align="center">
                        <Label text={path ? (scriptDirty() ? `${path} (modified)` : path) : "Open a script from the list."} size={12} name="script-path" />
                        <Button label="Save" small primary={scriptDirty()} name="script:save" onClick={saveScript} />
                        <Label text="Cmd/Ctrl+Return saves. pocket editor --watch rebuilds and reloads the project after a save." muted size={12} wrap flex={1} />
                    </Row>
                    <TextInput multiline flex={1} name="script:text" value={scriptText()} syntax={path || undefined} disabled={!path} onInput={(v) => { scriptText.set(v); scriptDirty.set(true); }} onChange={(v) => { scriptText.set(v); saveScript(); }} />
                    {typeErrors().length > 0 ? (
                        <box direction="column" gap={1} name="script:errors">
                            {typeErrors().slice(0, 6).map((d, i) => (
                                <Label key={i} text={`${d.file ?? ""}${d.line !== undefined ? `:${d.line}:${d.column ?? 0}` : ""}  ${d.message.split("\n")[0]}`} size={12} color={d.file === path ? theme.danger : theme.muted} />
                            ))}
                            {typeErrors().length > 6 ? <Label text={`and ${typeErrors().length - 6} more (the Console lists them all)`} muted size={12} /> : null}
                        </box>
                    ) : null}
                </box>
            </box>,
        ];
    } else {
        body = transcriptText().split("\n").map((line, i) => <Label key={i} text={line} size={12} />);
    }
    return body as VNode[];
}

/** A pane's tab label: the hierarchy counts its entities, the inspector names what it shows. */
function paneLabel(p: Pane): string {
    if (p === "hierarchy") return `Hierarchy (${rows().length})`;
    if (p === "inspector") {
        const d = described();
        const extra = selection().length - 1;
        return d ? `Inspector: ${d.name}${extra > 0 ? ` (+${extra} more)` : ""}` : "Inspector";
    }
    return PANE_LABELS[p];
}

/** A dock's tab: click shows its pane, a drag onto another dock (or near an edge of the scene) moves it there. */
function DockTab(props: { pane: Pane; active: boolean }) {
    const p = props.pane;
    return (
        <box name={`tab:${p}`} padding={[2, 8]} radius={4} background={props.active ? theme.accent : theme.panel} borderColor={theme.border} border={1}
            onClick={() => showPane(p)} onDrag={() => undefined}
            onDragEnd={(e) => { const to = e.x !== undefined && e.y !== undefined ? dockAt(e.x, e.y) : null; if (to) movePane(p, to); }}>
            <Label text={paneLabel(p)} size={12} color={props.active ? "#ffffff" : theme.text} />
        </box>
    );
}

/** A pane's own content at a dock's width; `grow` fills the height of a dock with tabs above it. */
function PaneView(p: Pane, width: Dim, grow = false): VNode {
    if (p === "hierarchy") return <Hierarchy width={width} grow={grow} />;
    if (p === "inspector") return <Inspector width={width} grow={grow} />;
    return (
        <box width={width} flex={grow ? 1 : undefined} direction="column" background={theme.panel} borderColor={theme.border} border={1} name={`pane:${p}`}>
            <box flex={1} overflow="scroll" padding={[4, 8]} gap={1} name={`${p}-body`}>
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
            <Row padding={[1, 4]} gap={3} wrap background={theme.panelAlt}>
                {panes.map((p) => <DockTab key={p} pane={p} active={p === active} />)}
            </Row>
            {PaneView(active, "100%", true)}
        </box>
    );
}

/** The bottom dock: its tabs and the status on one strip, the pane in front below (just the strip when it holds none). */
function Bottom() {
    const l = layout();
    const st = status();
    const panes = l.docks.bottom;
    const active = l.active.bottom;
    return (
        <box height={panes.length > 0 ? l.bottom : undefined} direction="column" background={theme.panel} borderColor={theme.border} border={1} name="bottom">
            <Row padding={[3, 6]} gap={4} background={theme.panelAlt}>
                {panes.map((p) => <DockTab key={p} pane={p} active={p === active} />)}
                <box flex={1} />
                <Label text={notice()} muted size={12} name="notice" />
                <box width={12} />
                <Label text={`tick ${st.tick}`} muted size={12} name="tick" />
                <Label text={`${st.entities} entities`} muted size={12} name="entities" />
                <Label text={st.hash} muted size={12} name="hash" />
            </Row>
            {active === "" ? null : active === "hierarchy" || active === "inspector" ? PaneView(active, "100%", true) : (
                <box flex={1} overflow="scroll" padding={[4, 8]} gap={1} name="bottom-body">
                    {TabBody(active)}
                </box>
            )}
        </box>
    );
}

/** The gizmo: absolute children of the main row, painted over the scene pane. Move handles at
 * the axis tips and the center; turns about Y, X and Z below to the left (R, RX, RZ); scales,
 * uniform and per axis, below to the right (S, SX, SY, SZ). */
/** The view bar over the scene pane's top-left corner: what the pane shows and how drags land. */
function ViewBar() {
    status();   // placed from the pane's rectangle, which settles over the first frames
    if (viewportRect.w === 0 || mainRect.w === 0) return null;
    return (
        <box position="absolute" left={viewportRect.x - mainRect.x + 8} top={8} direction="row" gap={4} padding={[3, 4]} radius={4} background={theme.panelAlt} name="viewbar">
            <Button label={overlays() ? "Overlays: on" : "Overlays"} small name="overlays" onClick={toggleOverlays} />
            <Button label={snap() ? "Snap: on" : "Snap"} small name="snap" onClick={toggleSnap} />
            <Button label={`${snapStep()}`} small name="snap_step" onClick={cycleSnapStep} />
            <Button label={localAxes() ? "Local" : "World"} small name="axes" onClick={toggleLocal} />
        </box>
    );
}

function GizmoHandles() {
    const g = gizmo();
    if (!g) return null;
    const handle = (axis: Axis, p: { x: number; y: number }, color: string, label: string) => (
        <box key={axis} name={`gizmo:${axis}`} position="absolute" left={p.x - 9} top={p.y - 9} width={18} height={18} radius={axis === "plane" ? 3 : 9} background={color} borderColor="#000000" border={1} justify="center" align="center"
            onMouseDown={() => gizmoDown(axis)} onDrag={gizmoDrag} onDragEnd={gizmoEnd}>
            <Label text={label} size={label.length > 1 ? 8 : 10} color="#101010" />
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
        handle("plane", g.center, "#f0f0f0", "+"),
        handle("x", g.x, "#e05050", "X"),
        handle("y", g.y, "#50c050", "Y"),
        handle("z", g.z, "#5080f0", "Z"),
        handle("rotate", clear(-30, 30), "#f0a030", "R"),
        handle("rotate_x", clear(-54, 30), "#f0a030", "RX"),
        handle("rotate_z", clear(-30, 54), "#f0a030", "RZ"),
        handle("scale", clear(30, 30), "#c080f0", "S"),
        handle("scale_x", clear(54, 30), "#c080f0", "SX"),
        handle("scale_y", clear(30, 54), "#c080f0", "SY"),
        handle("scale_z", clear(54, 54), "#c080f0", "SZ"),
    ];
}

function Splitter(props: { name: string; vertical?: boolean; onDrag: (e: UiEvent) => void }) {
    return <box name={props.name} width={props.vertical ? undefined : 6} height={props.vertical ? 6 : undefined} background={theme.border} onDrag={props.onDrag} onDragEnd={saveLayout} />;
}

function Editor() {
    return (
        <box width="100%" height="100%" direction="column" name="editor">
            <box background={theme.panelAlt} borderColor={theme.border} border={1}>
                <Toolbar />
            </box>
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
        </box>
    );
}

loadLayout();
mount(() => <Editor />);
refreshHierarchy();
refreshBottom();
