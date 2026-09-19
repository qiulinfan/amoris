// The Pocket editor: a TSX program on Pocket UI that runs beside a project in the same runtime.
// It sees the world through the same commands scripts and agents use, so everything shown here
// (hierarchy, inspector, console, transcript) is also reachable by `ui_snapshot`, and every
// button is reachable by `ui_click`.
import { Button, Label, Panel, Row, TextInput, command, mount, onFrame, onInput, render, setProjectRoot, signal, theme, ui, world } from "pocket";
import type { ComponentName, Described, UiEvent, WorldEvent } from "pocket";
import { applyOrbit, orbitFromCamera } from "./orbit";
import type { Orbit } from "./orbit";

// ------------------------------------------------------------------------------------ state
interface TreeRow { id: number; name: string; path: string; depth: number }
interface LogRow { seq: number; tick?: number; level: string; cat: string; msg: string }
interface SchemaField { name: string; type: string; doc: string }
interface SchemaComponent { name: string; doc: string; serialized: boolean; fields: SchemaField[]; default?: Record<string, unknown> }

const info = command<{ name: string; scene: string | null; contexts: string[]; window: { width: number; height: number } }>("project.info");
const schema = command<{ components: SchemaComponent[] }>("world.schema").components;

const selected = signal(0);
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

let editScene: unknown = null;
let orbit: Orbit | undefined;
let lastViewport = "";
let frame = 0;

// ------------------------------------------------------------------------------------ data
function refreshHierarchy(): void {
    const list = world.query({ fields: [] }).map((r) => r.path);
    list.sort();
    const out: TreeRow[] = [];
    for (const r of world.query({ fields: [] })) {
        const parts = r.path.split("/").filter((p) => p.length > 0);
        out.push({ id: r.id, name: parts[parts.length - 1] ?? String(r.id), path: r.path, depth: parts.length - 1 });
    }
    out.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
    rows.set(out);
    if (selected() !== 0 && !out.some((r) => r.id === selected())) select(0);
}

function refreshSelected(): void {
    const id = selected();
    if (id === 0) {
        described.set(null);
        return;
    }
    try {
        described.set(world.describe(id));
    } catch {
        select(0);
    }
}

function select(id: number): void {
    selected.set(id);
    addingComponent.set(false);
    refreshSelected();
}

function refreshBottom(): void {
    const t = tab();
    if (t === "console") logs.set(command<LogRow[]>("log.tail", { n: 40 }));
    else if (t === "events") recentEvents.set(command<WorldEvent[]>("events.recent", { n: 40 }));
    else transcriptText.set(command<{ text: string }>("transcript", { max_lines: 30 }).text);
}

// ------------------------------------------------------------------------------------ actions
function play(): void {
    if (!playing()) {
        editScene = world.save();
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
    const id = world.spawn("Entity", { parent, components: { Transform: {} } });
    refreshHierarchy();
    select(id);
}

function deleteSelected(): void {
    if (selected() === 0) return;
    world.destroy(selected());
    select(0);
    refreshHierarchy();
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
    try {
        world.set(entity, comp, patch as never);
        refreshSelected();
    } catch (e) {
        notice.set(`Set failed: ${String(e)}`);
    }
}

function pixelScale(): number {
    const root = ui.describe(1) as { rect: { w: number } };
    const win = command<{ window: { width: number } }>("project.info").window;
    return root.rect.w > 0 ? win.width / root.rect.w : 1;
}

function onViewportDown(e: UiEvent): void {
    const s = pixelScale();
    const hit = render.pick(Math.floor((e.x ?? 0) * s), Math.floor((e.y ?? 0) * s));
    select(hit ? hit.id : 0);
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
    if (vp) {
        const key = `${Math.round(vp.rect.x)},${Math.round(vp.rect.y)},${Math.round(vp.rect.w)},${Math.round(vp.rect.h)}`;
        if (key !== lastViewport) {
            lastViewport = key;
            command("render.viewport", { x: vp.rect.x, y: vp.rect.y, w: vp.rect.w, h: vp.rect.h });
            setProjectRoot(vp.id);
        }
    }
});

onInput((events) => {
    for (const e of events) {
        if (e.ui !== undefined && e.type !== "key_down") continue;
        if (e.type === "key_down" && e.ui === undefined) {
            if (e.key === "Space") { if (paused()) play(); else pause(); }
            else if (e.key === "Delete" || e.key === "Backspace") deleteSelected();
            else if (e.key === "Escape") select(0);
        }
    }
});

// ------------------------------------------------------------------------------------ views
function Toolbar() {
    const s = status();
    return (
        <Row padding={[6, 10]} gap={8} name="toolbar" height={40}>
            <Label text="Pocket" size={15} />
            <Label text={info.name} muted />
            <box width={12} />
            {paused() ? <Button label="Play" primary name="play" onClick={play} /> : <Button label="Pause" name="pause" onClick={pause} />}
            <Button label="Step" name="step" onClick={step} disabled={!paused()} />
            <Button label="Stop" danger name="stop" onClick={stop} disabled={!playing()} />
            <box width={12} />
            <Button label="Save scene" name="save" onClick={saveScene} disabled={!info.scene} />
            <Button label="Spawn" name="spawn" onClick={spawnEntity} />
            <Button label="Delete" name="delete" onClick={deleteSelected} disabled={selected() === 0} />
            <box flex={1} />
            <Label text={`tick ${s.tick}`} muted name="tick" />
            <Label text={`${s.entities} entities`} muted name="entities" />
            <Label text={s.hash} muted name="hash" />
        </Row>
    );
}

function Hierarchy() {
    const list = rows();
    return (
        <Panel title={`Hierarchy (${list.length})`} width={240} scroll name="hierarchy" padding={2} gap={0}>
            {list.length === 0 ? <Label text="No entities. Press Play or Spawn." muted wrap /> : null}
            {list.map((r) => (
                <box key={r.id} onClick={() => select(r.id)} padding={[3, 6]} radius={3} background={selected() === r.id ? theme.accent : null} name={`entity:${r.name}`}>
                    <Label text={`${"  ".repeat(r.depth)}${r.name}`} color={selected() === r.id ? theme.accentText : theme.text} />
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
    if (!d) {
        return (
            <Panel title="Inspector" width={320} name="inspector">
                <Label text="Select an entity in the hierarchy or click it in the scene." muted wrap />
            </Panel>
        );
    }
    const present = Object.keys(d.components);
    const missing = schema.filter((c) => c.serialized && !present.includes(c.name));
    return (
        <Panel title={`Inspector: ${d.name}`} width={320} scroll name="inspector" gap={8}>
            <Row gap={4}>
                <Label text="name" muted />
                <TextInput value={d.name} flex={1} name="entity-name" onChange={(v) => { if (v.length > 0) { world.rename(d.id, v); refreshHierarchy(); refreshSelected(); } }} />
            </Row>
            <Label text={`${d.path}  #${d.id}`} muted size={11} />
            {schema.filter((c) => present.includes(c.name)).map((c) => (
                <box key={c.name} gap={4} padding={[4, 0]} borderColor={theme.border} border={0}>
                    <Row>
                        <Label text={c.name} size={13} />
                        <box flex={1} />
                        {c.serialized ? <Button label="remove" small name={`remove:${c.name}`} onClick={() => { world.remove(d.id, c.name as ComponentName); refreshSelected(); }} /> : <Label text="derived" muted size={11} />}
                    </Row>
                    {c.fields.map((f) => fieldInputs(d.id, c.name as ComponentName, f, (d.components as Record<string, Record<string, unknown>>)[c.name]?.[f.name]))}
                </box>
            ))}
            {addingComponent() ? (
                <box gap={2}>
                    <Label text="Add component" muted />
                    <Row wrap gap={4}>
                        {missing.map((c) => <Button key={c.name} label={c.name} small name={`add:${c.name}`} onClick={() => { world.set(d.id, c.name as ComponentName, {} as never); addingComponent.set(false); refreshSelected(); }} />)}
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
    const tabButton = (id: typeof t, label: string) => <Button label={label} small primary={t === id} name={`tab:${id}`} onClick={() => { tab.set(id); refreshBottom(); }} />;
    let body;
    if (t === "console") {
        body = logs().map((l) => <Label key={l.seq} text={`${l.tick !== undefined ? `[${l.tick}] ` : ""}${l.level} ${l.cat}: ${l.msg}`} size={12} color={l.level === "error" ? theme.danger : l.level === "warn" ? "#f0c060" : theme.text} />);
    } else if (t === "events") {
        body = recentEvents().map((e) => <Label key={e.seq} text={`#${e.seq} t${e.tick} ${e.type}${e.subject ? ` @${e.subject}` : ""}${e.cause ? ` <- #${e.cause}` : ""} ${e.data ? JSON.stringify(e.data) : ""}`} size={12} />);
    } else {
        body = transcriptText().split("\n").map((line, i) => <Label key={i} text={line} size={12} />);
    }
    return (
        <box height={200} direction="column" background={theme.panel} borderColor={theme.border} border={1} name="bottom">
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

function Editor() {
    return (
        <box width="100%" height="100%" direction="column" name="editor">
            <box background={theme.panelAlt} borderColor={theme.border} border={1}>
                <Toolbar />
            </box>
            <Row flex={1} gap={0} align="stretch">
                <Hierarchy />
                <box flex={1} name="viewport" onMouseDown={onViewportDown} onDrag={onViewportDrag} onWheel={onViewportWheel} overflow="hidden" />
                <Inspector />
            </Row>
            <Bottom />
        </box>
    );
}

mount(() => <Editor />);
refreshHierarchy();
refreshBottom();
