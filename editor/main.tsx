// The Pocket editor: a TSX program on Pocket UI that runs beside a project in the same runtime.
// It sees the world through the same commands scripts and agents use, so everything shown here
// (hierarchy, inspector, console, transcript) is also reachable by `ui_snapshot`, and every
// button is reachable by `ui_click`. Every edit is undoable (editor/history.ts) and the scene
// pane has a translate gizmo (editor/gizmo.ts) that agents drag with `ui.drag`.
import { Button, Label, Panel, Row, TextInput, command, mount, onFrame, onInput, render, setProjectRoot, signal, theme, tilemap, ui, world } from "pocket";
import type { ComponentName, Described, Scene, Transform, UiEvent, WorldEvent } from "pocket";
import { applyOrbit, orbitFromCamera } from "./orbit";
import type { Orbit } from "./orbit";
import * as history from "./history";
import { axisDelta, axisQuat, layoutFor, multiplyQuat, planeDelta } from "./gizmo";
import type { Axis, GizmoLayout } from "./gizmo";
import type { Vec3 } from "pocket";

// ------------------------------------------------------------------------------------ state
interface TreeRow { id: number; name: string; path: string; depth: number }
interface LogRow { seq: number; tick?: number; level: string; cat: string; msg: string }
interface SchemaField { name: string; type: string; doc: string }
interface SchemaComponent { name: string; doc: string; serialized: boolean; fields: SchemaField[]; default?: Record<string, unknown> }
interface Layout { hierarchy: number; inspector: number; bottom: number }
interface AssetRow { path: string; kind: "mesh" | "image" | "tilemap" | "audio" | "script" | "other"; bytes: number; loaded: boolean }
type Tab = "console" | "events" | "transcript" | "assets" | "input" | "script";
const TABS: Tab[] = ["console", "events", "transcript", "assets", "input", "script"];
interface ActionBindings { positive?: string[]; negative?: string[]; axis?: string[]; deadzone?: number }
interface GizmoView { center: { x: number; y: number }; x: { x: number; y: number }; y: { x: number; y: number }; z: { x: number; y: number } }

const LAYOUT_PATH = ".pocket/editor.json";
const DEFAULT_LAYOUT: Layout = { hierarchy: 240, inspector: 320, bottom: 200 };

const info = command<{ name: string; scene: string | null; contexts: string[]; window: { width: number; height: number } }>("project.info");
const schema = command<{ components: SchemaComponent[] }>("world.schema").components;

const selection = signal<number[]>([]);   // ordered; the last one is the primary selection
const playing = signal(false);
const paused = signal(true);
const overlays = signal(false);   // colliders and joints drawn as lines in the scene pane
const snap = signal(false);       // gizmo drags land on the grid: snapStep units, 15 degrees, quarter scales
const snapStep = signal(0.5);     // the grid a snapped move lands on, cycled by the toolbar
const SNAP_STEPS = [0.1, 0.25, 0.5, 1, 2];
const SNAP_ANGLE = Math.PI / 12, SNAP_SCALE = 0.25;
const tab = signal<Tab>("console");
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
const actions = signal<Record<string, ActionBindings>>({});   // the input map, shown and edited by the Input tab
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
let dragState: { ids: number[]; before: Map<number, Transform>; layout: GizmoLayout; axis: Axis; turned: number; scaled: number; moved: Vec3 } | null = null;
let stroke: { entity: number; layer: string; pos: { x: number; y: number }; cells: Map<string, { tile_x: number; tile_y: number; was: number; gid: number }> } | null = null;

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

function refreshBottom(): void {
    const t = tab();
    if (t === "console") logs.set(command<LogRow[]>("log.tail", { n: 40 }));
    else if (t === "events") recentEvents.set(command<WorldEvent[]>("events.recent", { n: 40 }));
    else if (t === "assets" || t === "script") assetRows.set(command<AssetRow[]>("assets.list"));
    else if (t === "input") actions.set(command<Record<string, ActionBindings>>("input.describe"));
    else transcriptText.set(command<{ text: string }>("transcript", { max_lines: 30 }).text);
}

// ------------------------------------------------------------------------------------ layout persistence
function loadLayout(): void {
    try {
        const r = command<{ text: string }>("project.read", { path: LAYOUT_PATH });
        const j = JSON.parse(r.text) as { layout?: Partial<Layout>; tab?: Tab; snap?: boolean; snap_step?: number };
        if (j.layout) layout.set({ ...DEFAULT_LAYOUT, ...j.layout });
        if (j.tab !== undefined && TABS.includes(j.tab)) tab.set(j.tab);
        if (j.snap === true) snap.set(true);
        if (typeof j.snap_step === "number" && SNAP_STEPS.includes(j.snap_step)) snapStep.set(j.snap_step);
    } catch {
        // No saved layout yet.
    }
}

function saveLayout(): void {
    try {
        command("project.write", { path: LAYOUT_PATH, json: { layout: layout(), tab: tab(), snap: snap(), snap_step: snapStep() } });
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
            const clips = Array.isArray(d.clips) ? d.clips.length : 0;
            return `${String(d.vertices)} vertices, ${String(d.triangles)} triangles, ${String(d.submeshes)} submeshes, ${String(d.nodes)} nodes${clips > 0 ? `, ${clips} clips` : ""}`;
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

/** An asset row dropped on the scene pane: an entity under the pointer, one undoable edit. */
function placeAsset(row: AssetRow, e: UiEvent): void {
    if (e.x === undefined || e.y === undefined) return;
    // Inside the scene pane (by its rectangle: the gizmo's handles float over it and must not count as elsewhere).
    if (e.x < viewportRect.x || e.x >= viewportRect.x + viewportRect.w || e.y < viewportRect.y || e.y >= viewportRect.y + viewportRect.h) return;
    const made = assetComponents(row);
    if (!made) { notice.set(`${row.path}: nothing in the runtime reads this kind of file`); return; }
    const s = pixelScale();
    const ray = render.unproject(e.x * s, e.y * s, made.plane, 0);
    const position = ray.hit && ray.point ? ray.point : { x: 0, y: 0, z: 0 };
    const stem = (row.path.split("/").pop() ?? row.path).replace(/\.[^.]+$/, "");
    const name = uniqueName(stem, siblingNames(undefined));
    const ref = { id: 0, fragment: undefined as Scene | undefined };
    edit(
        `Place ${name}`,
        () => {
            ref.id = ref.fragment ? world.instantiate(ref.fragment) : world.spawn(name, { components: { Transform: { position }, ...made.components } as never });
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
    notice.set(`${name} placed at ${position.x.toFixed(2)}, ${position.y.toFixed(2)}, ${position.z.toFixed(2)}${ray.hit ? "" : " (the pointer's ray misses the plane)"}`);
}

function onViewportDown(e: UiEvent): void {
    endStroke();
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
}

function onViewportDrag(e: UiEvent): void {
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
    if (id === 0 || mainRect.w === 0) {
        if (gizmo() !== null) gizmo.set(null);
        return;
    }
    const lay = layoutFor(id);
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
    const lay = layoutFor(primary);
    if (!lay) return;
    const before = new Map<number, Transform>();
    for (const id of ids) before.set(id, clone(world.get(id, "Transform")!));
    dragState = { ids, before, layout: lay, axis, turned: 0, scaled: 1, moved: { x: 0, y: 0, z: 0 } };
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
        // Horizontal drag turns the selection around a world axis (R: Y, RX: X, RZ: Z), 100 px per radian.
        dragState.turned += dx * 0.01;
        const about = axis === "rotate" ? "y" : axis === "rotate_x" ? "x" : "z";
        const q = axisQuat(about, snapping ? snapTo(dragState.turned, SNAP_ANGLE) : dragState.turned);
        for (const [id, before] of dragState.before) if (alive(id)) world.set(id, "Transform", { rotation: multiplyQuat(q, before.rotation) });
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
        const onAxis = (a: "x" | "y" | "z") => axis === "plane" || axis === a;
        const step = snapStep();
        const at = (v: number, a: "x" | "y" | "z") => (snapping && onAxis(a) ? snapTo(v + m[a], step) : v + m[a]);
        for (const [id, before] of dragState.before) if (alive(id)) world.set(id, "Transform", { position: { x: at(before.position.x, "x"), y: at(before.position.y, "y"), z: at(before.position.z, "z") } });
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
    if (frame % 15 === 1) {
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
    render.debug({ colliders: on, joints: on });
    overlays.set(on);
}

function toggleSnap(): void {
    snap.set(!snap());
    saveLayout();
    notice.set(snap() ? `Snap on: moves to ${snapStep()} units, turns to 15 degrees, scales to quarters` : "Snap off");
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
            <box width={12} />
            <Button label={overlays() ? "Overlays: on" : "Overlays"} name="overlays" onClick={toggleOverlays} />
            <Button label={snap() ? "Snap: on" : "Snap"} name="snap" onClick={toggleSnap} />
            <Button label={`${snapStep()}`} name="snap_step" onClick={cycleSnapStep} />
            <box flex={1} />
            <Label text={`tick ${s.tick}`} muted name="tick" />
            <Label text={`${s.entities} entities`} muted name="entities" />
            <Label text={s.hash} muted name="hash" />
        </Row>
    );
}

function Hierarchy() {
    const list = rows();
    const sel = selection();
    return (
        <Panel title={`Hierarchy (${list.length})`} width={layout().hierarchy} scroll name="hierarchy" padding={2} gap={0}>
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

function Inspector() {
    const d = described();
    const extra = selection().length - 1;
    if (!d) {
        return (
            <Panel title="Inspector" width={layout().inspector} name="inspector">
                <Label text="Select an entity in the hierarchy or click it in the scene. Shift-click adds to the selection." muted wrap />
            </Panel>
        );
    }
    const present = Object.keys(d.components);
    const missing = schema.filter((c) => c.serialized && !present.includes(c.name));
    return (
        <Panel title={`Inspector: ${d.name}${extra > 0 ? ` (+${extra} more)` : ""}`} width={layout().inspector} scroll name="inspector" gap={8}>
            <Row gap={4}>
                <Label text="name" muted />
                <TextInput value={d.name} flex={1} name="entity-name" onChange={(v) => { if (v.length > 0) renameEntity(d.id, v); }} />
            </Row>
            <Label text={`${d.path}  #${d.id}`} muted size={11} />
            {present.includes("TileMap") ? <TileBrush id={d.id} /> : null}
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

function Bottom() {
    const t = tab();
    const tabButton = (id: typeof t, label: string) => <Button label={label} small primary={t === id} name={`tab:${id}`} onClick={() => { tab.set(id); refreshBottom(); saveLayout(); }} />;
    let body;
    if (t === "console") {
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
    } else if (t === "assets") {
        const list = assetRows();
        const picked = assetPick();
        body = [
            <Label key="hint" text={list.length === 0 ? "No files under assets/." : picked ? `${picked}: ${assetInfo()}` : "Click a file to describe it; drag one onto the scene to place it."} muted size={12} name="asset-info" wrap />,
            ...list.map((r) => (
                <box key={r.path} name={`asset:${r.path}`} direction="row" align="center" padding={[2, 6]} gap={8} radius={3} background={picked === r.path ? theme.accent : null} onClick={() => pickAsset(r)} onDrag={() => undefined} onDragEnd={(e) => placeAsset(r, e)}>
                    {r.kind === "image" ? <box width={16} height={16} image={r.path} name={`thumb:${r.path}`} /> : null}
                    <Label text={r.path} size={12} color={picked === r.path ? theme.accentText : theme.text} />
                    <Label text={r.kind} size={12} color={picked === r.path ? theme.accentText : theme.muted} />
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
                    <TextInput multiline flex={1} name="script:text" value={scriptText()} disabled={!path} onInput={(v) => { scriptText.set(v); scriptDirty.set(true); }} onChange={(v) => { scriptText.set(v); saveScript(); }} />
                </box>
            </box>,
        ];
    } else {
        body = transcriptText().split("\n").map((line, i) => <Label key={i} text={line} size={12} />);
    }
    return (
        <box height={layout().bottom} direction="column" background={theme.panel} borderColor={theme.border} border={1} name="bottom">
            <Row padding={[3, 6]} gap={4} background={theme.panelAlt}>
                {tabButton("console", "Console")}
                {tabButton("events", "Events")}
                {tabButton("transcript", "Transcript")}
                {tabButton("assets", "Assets")}
                {tabButton("input", "Input")}
                {tabButton("script", "Script")}
                <box flex={1} />
                <Label text={notice()} muted size={12} name="notice" />
            </Row>
            <box flex={1} overflow="scroll" padding={[4, 8]} gap={1} name="bottom-body">
                {body}
            </box>
        </box>
    );
}

/** The gizmo: absolute children of the main row, painted over the scene pane. Move handles at
 * the axis tips and the center; turns about Y, X and Z below to the left (R, RX, RZ); scales,
 * uniform and per axis, below to the right (S, SX, SY, SZ). */
function GizmoHandles() {
    const g = gizmo();
    if (!g) return null;
    const handle = (axis: Axis, p: { x: number; y: number }, color: string, label: string) => (
        <box key={axis} name={`gizmo:${axis}`} position="absolute" left={p.x - 9} top={p.y - 9} width={18} height={18} radius={axis === "plane" ? 3 : 9} background={color} borderColor="#000000" border={1} justify="center" align="center"
            onMouseDown={() => gizmoDown(axis)} onDrag={gizmoDrag} onDragEnd={gizmoEnd}>
            <Label text={label} size={label.length > 1 ? 8 : 10} color="#101010" />
        </box>
    );
    return [
        handle("plane", g.center, "#f0f0f0", "+"),
        handle("x", g.x, "#e05050", "X"),
        handle("y", g.y, "#50c050", "Y"),
        handle("z", g.z, "#5080f0", "Z"),
        handle("rotate", { x: g.center.x - 30, y: g.center.y + 30 }, "#f0a030", "R"),
        handle("rotate_x", { x: g.center.x - 54, y: g.center.y + 30 }, "#f0a030", "RX"),
        handle("rotate_z", { x: g.center.x - 30, y: g.center.y + 54 }, "#f0a030", "RZ"),
        handle("scale", { x: g.center.x + 30, y: g.center.y + 30 }, "#c080f0", "S"),
        handle("scale_x", { x: g.center.x + 54, y: g.center.y + 30 }, "#c080f0", "SX"),
        handle("scale_y", { x: g.center.x + 30, y: g.center.y + 54 }, "#c080f0", "SY"),
        handle("scale_z", { x: g.center.x + 54, y: g.center.y + 54 }, "#c080f0", "SZ"),
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
                <Hierarchy />
                <Splitter name="split:hierarchy" onDrag={(e) => layout.update((l) => ({ ...l, hierarchy: clamp(l.hierarchy + (e.dx ?? 0), 120, 600) }))} />
                <box flex={1} name="viewport" onMouseDown={onViewportDown} onMouseUp={onViewportUp} onDrag={onViewportDrag} onDragEnd={onViewportUp} onWheel={onViewportWheel} overflow="hidden" />
                <Splitter name="split:inspector" onDrag={(e) => layout.update((l) => ({ ...l, inspector: clamp(l.inspector - (e.dx ?? 0), 160, 700) }))} />
                <Inspector />
                {GizmoHandles()}
            </Row>
            <Splitter name="split:bottom" vertical onDrag={(e) => layout.update((l) => ({ ...l, bottom: clamp(l.bottom - (e.dy ?? 0), 60, 600) }))} />
            <Bottom />
        </box>
    );
}

loadLayout();
mount(() => <Editor />);
refreshHierarchy();
refreshBottom();
