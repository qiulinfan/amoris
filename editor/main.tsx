// The Pocket editor: a TSX program on Pocket UI that runs beside a project in the same runtime.
// It sees the world through the same commands scripts and agents use, so everything shown here
// (hierarchy, inspector, console, transcript) is also reachable by `ui_snapshot`, and every
// button is reachable by `ui_click`. Every edit is undoable (editor/history.ts) and the scene
// pane has a translate gizmo (editor/gizmo.ts) that agents drag with `ui.drag`.
import { Button, Label, Panel, Row, TextInput, command, mount, onFrame, onInput, render, setProjectRoot, signal, theme, ui, world } from "pocket";
import type { ComponentName, Described, Scene, Transform, UiEvent, WorldEvent } from "pocket";
import { applyOrbit, orbitFromCamera } from "./orbit";
import type { Orbit } from "./orbit";
import * as history from "./history";
import { axisDelta, layoutFor, multiplyQuat, planeDelta, yawQuat } from "./gizmo";
import type { Axis, GizmoLayout } from "./gizmo";

// ------------------------------------------------------------------------------------ state
interface TreeRow { id: number; name: string; path: string; depth: number }
interface LogRow { seq: number; tick?: number; level: string; cat: string; msg: string }
interface SchemaField { name: string; type: string; doc: string }
interface SchemaComponent { name: string; doc: string; serialized: boolean; fields: SchemaField[]; default?: Record<string, unknown> }
interface Layout { hierarchy: number; inspector: number; bottom: number }
interface GizmoView { center: { x: number; y: number }; x: { x: number; y: number }; y: { x: number; y: number }; z: { x: number; y: number } }

const LAYOUT_PATH = ".pocket/editor.json";
const DEFAULT_LAYOUT: Layout = { hierarchy: 240, inspector: 320, bottom: 200 };

const info = command<{ name: string; scene: string | null; contexts: string[]; window: { width: number; height: number } }>("project.info");
const schema = command<{ components: SchemaComponent[] }>("world.schema").components;

const selection = signal<number[]>([]);   // ordered; the last one is the primary selection
const playing = signal(false);
const paused = signal(true);
const tab = signal<"console" | "events" | "transcript">("console");
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

let editScene: unknown = null;
let orbit: Orbit | undefined;
let lastViewport = "";
let viewportRect = { x: 0, y: 0, w: 0, h: 0 };
let mainRect = { x: 0, y: 0, w: 0, h: 0 };
let frame = 0;
let dragState: { ids: number[]; before: Map<number, Transform>; layout: GizmoLayout; axis: Axis; turned: number; scaled: number } | null = null;

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
    else transcriptText.set(command<{ text: string }>("transcript", { max_lines: 30 }).text);
}

// ------------------------------------------------------------------------------------ layout persistence
function loadLayout(): void {
    try {
        const r = command<{ text: string }>("project.read", { path: LAYOUT_PATH });
        const j = JSON.parse(r.text) as { layout?: Partial<Layout>; tab?: "console" | "events" | "transcript" };
        if (j.layout) layout.set({ ...DEFAULT_LAYOUT, ...j.layout });
        if (j.tab === "console" || j.tab === "events" || j.tab === "transcript") tab.set(j.tab);
    } catch {
        // No saved layout yet.
    }
}

function saveLayout(): void {
    try {
        command("project.write", { path: LAYOUT_PATH, json: { layout: layout(), tab: tab() } });
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
    else if (type !== "string") {
        const n = Number(raw);
        if (!Number.isFinite(n)) return;
        value = n;
    }
    const patch: Record<string, unknown> = sub === null ? { [field]: value } : { [field]: { [sub]: value } };
    const before = world.get(entity, comp) as Record<string, unknown> | undefined;
    // Blur re-reports the input's value; an unchanged value is not an edit.
    const current = before === undefined ? undefined : sub === null ? before[field] : (before[field] as Record<string, unknown> | undefined)?.[sub];
    if (current === value) return;
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

function onViewportDown(e: UiEvent): void {
    const s = pixelScale();
    const hit = render.pick(Math.floor((e.x ?? 0) * s), Math.floor((e.y ?? 0) * s));
    const toggle = e.mods?.includes("shift") || e.mods?.includes("meta") || e.mods?.includes("ctrl");
    if (hit) select(hit.id, toggle ? "toggle" : "replace");
    else if (!toggle) select(0);
    orbit = orbitFromCamera();
}

function onViewportDrag(e: UiEvent): void {
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
    dragState = { ids, before, layout: lay, axis, turned: 0, scaled: 1 };
}

function gizmoDrag(e: UiEvent): void {
    if (!dragState) return;
    const s = pixelScale();
    const dx = (e.dx ?? 0) * s, dy = (e.dy ?? 0) * s;
    if (dragState.axis === "rotate") {
        // Horizontal drag turns the selection around the world Y axis, 100 px per radian.
        dragState.turned += dx * 0.01;
        const q = yawQuat(dragState.turned);
        for (const [id, before] of dragState.before) if (alive(id)) world.set(id, "Transform", { rotation: multiplyQuat(q, before.rotation) });
    } else if (dragState.axis === "scale") {
        // Drag right to grow, left to shrink; uniform, relative to the size at the start of the drag.
        dragState.scaled = Math.max(0.01, dragState.scaled * Math.exp(dx * 0.005));
        const k = dragState.scaled;
        for (const [id, before] of dragState.before) if (alive(id)) world.set(id, "Transform", { scale: { x: before.scale.x * k, y: before.scale.y * k, z: before.scale.z * k } });
    } else {
        const delta = dragState.axis === "plane" ? planeDelta(selected(), dx, dy) : axisDelta(dragState.layout, dragState.axis, dx, dy);
        for (const id of dragState.ids) {
            const t = world.get(id, "Transform");
            if (!t) continue;
            world.set(id, "Transform", { position: { x: t.position.x + delta.x, y: t.position.y + delta.y, z: t.position.z + delta.z } });
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
            <Button label="Save scene" name="save" onClick={saveScene} disabled={!info.scene} />
            <Button label="Spawn" name="spawn" onClick={spawnEntity} />
            <Button label="Duplicate" name="duplicate" onClick={duplicateSelected} disabled={selection().length === 0} />
            <Button label="Delete" name="delete" onClick={deleteSelected} disabled={selection().length === 0} />
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
                <box key={r.id} onClick={(e) => select(r.id, e.mods?.includes("shift") || e.mods?.includes("meta") || e.mods?.includes("ctrl") ? "toggle" : "replace")} padding={[3, 6]} radius={3} background={sel.includes(r.id) ? theme.accent : null} name={`entity:${r.name}`}>
                    <Label text={`${"  ".repeat(r.depth)}${r.name}`} color={sel.includes(r.id) ? theme.accentText : theme.text} />
                </box>
            ))}
        </Panel>
    );
}

function fieldInputs(entity: number, comp: ComponentName, field: SchemaField, value: unknown) {
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
    } else {
        body = transcriptText().split("\n").map((line, i) => <Label key={i} text={line} size={12} />);
    }
    return (
        <box height={layout().bottom} direction="column" background={theme.panel} borderColor={theme.border} border={1} name="bottom">
            <Row padding={[3, 6]} gap={4} background={theme.panelAlt}>
                {tabButton("console", "Console")}
                {tabButton("events", "Events")}
                {tabButton("transcript", "Transcript")}
                <box flex={1} />
                <Label text={notice()} muted size={12} name="notice" />
            </Row>
            <box flex={1} overflow="scroll" padding={[4, 8]} gap={1} name="bottom-body">
                {body}
            </box>
        </box>
    );
}

/** Translate gizmo: absolute children of the main row, painted over the scene pane. */
function GizmoHandles() {
    const g = gizmo();
    if (!g) return null;
    const handle = (axis: Axis, p: { x: number; y: number }, color: string, label: string) => (
        <box key={axis} name={`gizmo:${axis}`} position="absolute" left={p.x - 9} top={p.y - 9} width={18} height={18} radius={axis === "plane" ? 3 : 9} background={color} borderColor="#000000" border={1} justify="center" align="center"
            onMouseDown={() => gizmoDown(axis)} onDrag={gizmoDrag} onDragEnd={gizmoEnd}>
            <Label text={label} size={10} color="#101010" />
        </box>
    );
    return [
        handle("plane", g.center, "#f0f0f0", "+"),
        handle("x", g.x, "#e05050", "X"),
        handle("y", g.y, "#50c050", "Y"),
        handle("z", g.z, "#5080f0", "Z"),
        handle("rotate", { x: g.center.x - 30, y: g.center.y + 30 }, "#f0a030", "R"),
        handle("scale", { x: g.center.x + 30, y: g.center.y + 30 }, "#c080f0", "S"),
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
                <box flex={1} name="viewport" onMouseDown={onViewportDown} onDrag={onViewportDrag} onWheel={onViewportWheel} overflow="hidden" />
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
