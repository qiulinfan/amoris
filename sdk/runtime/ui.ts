// Pocket UI for gameplay code and the editor.
//
// The engine keeps a retained element tree (boxes, text, inputs) laid out with flexbox and
// drawn over the scene. Scripts describe the interface as a tree of virtual nodes, usually with
// JSX; `mount()` renders it and, whenever a signal changes, renders it again and sends only the
// difference to the engine as `ui.apply` operations. Elements get engine events (click, input,
// keydown, ...) only when they declare a handler. Agents see the same interface as text through
// `ui.snapshot` and drive it with `ui.click` / `ui.type`, so anything a person can click, an
// agent can click.
declare const __pocket: { command(name: string, params?: unknown): unknown };

// ---------------------------------------------------------------------------------------------
// Low-level: operations and commands

export type UiOp =
    | ["create", number, "box" | "text" | "input"]
    | ["set", number, Record<string, unknown>]
    | ["append", number, number, number?]
    | ["remove", number]
    | ["text", number, string]
    | ["clear", number]
    | ["focus", number];

export interface UiEvent {
    type: "click" | "mousedown" | "mouseup" | "input" | "change" | "keydown" | "wheel" | "hover" | "focus" | "blur" | "drag" | "dragend";
    id: number;
    name?: string;
    x?: number;
    y?: number;
    dx?: number;
    dy?: number;
    startX?: number;
    startY?: number;
    button?: number;
    value?: string;
    key?: string;
    repeat?: boolean;
    consumed?: boolean;
    entered?: boolean;
}

export interface UiRect { x: number; y: number; w: number; h: number }

export interface UiNodeInfo {
    id: number;
    type: string;
    name?: string;
    text?: string;
    value?: string;
    rect: UiRect;
}

const ROOT = 1;

function cmd<T>(name: string, params?: unknown): T {
    return __pocket.command(name, params) as T;
}

export const ui = {
    /** Apply raw operations. `mount()` does this for you. */
    apply(ops: UiOp[]): void {
        if (ops.length > 0) cmd("ui.apply", { ops });
    },
    /** The interface as text: the same view agents get. */
    snapshot(options: { root?: number; depth?: number; maxNodes?: number; layout?: boolean; styles?: boolean } = {}): string {
        return cmd<{ text: string }>("ui.snapshot", { root: options.root, depth: options.depth, max_nodes: options.maxNodes, layout: options.layout, styles: options.styles }).text;
    },
    query(filter: { type?: string; text?: string; name?: string }): UiNodeInfo[] {
        return cmd<UiNodeInfo[]>("ui.query", filter);
    },
    describe(id: number): Record<string, unknown> {
        return cmd("ui.describe", { id });
    },
    hit(x: number, y: number): number {
        return cmd<{ id: number }>("ui.hit", { x, y }).id;
    },
    focus(id: number): void {
        cmd("ui.focus", { id });
    },
    /** Synthetic click on an element (by id) or a point, routed like real input. */
    click(target: number | { x: number; y: number }, button = 1): UiEvent[] {
        const params = typeof target === "number" ? { id: target, button } : { ...target, button };
        return cmd<{ events: UiEvent[] }>("ui.click", params).events;
    },
    type(text: string): UiEvent[] {
        return cmd<{ events: UiEvent[] }>("ui.type", { text }).events;
    },
    key(key: string): UiEvent[] {
        return cmd<{ events: UiEvent[] }>("ui.key", { key }).events;
    },
    stats(): { nodes: number; focused: number; hovered: number; paints: number } {
        return cmd("ui.stats");
    },
};

// ---------------------------------------------------------------------------------------------
// Signals: plain reactive values. Setting one schedules a re-render of every mounted tree.

interface SharedState {
    nextId: number;
    version: number;
    handlers: Map<number, Record<string, (e: UiEvent) => void>>;
    mounts: Mount[];
    seen: number;
    /** Where project interfaces mount by default (the editor points this at its scene pane). */
    projectRoot: number;
}

const shared: SharedState = (() => {
    const g = globalThis as unknown as { __pocket_ui?: SharedState };
    if (g.__pocket_ui === undefined) g.__pocket_ui = { nextId: 2, version: 1, handlers: new Map(), mounts: [], seen: 0, projectRoot: ROOT };
    return g.__pocket_ui;
})();

export interface Signal<T> {
    (): T;
    set(value: T): void;
    update(fn: (value: T) => T): void;
}

export function signal<T>(initial: T): Signal<T> {
    let value = initial;
    const read = (() => value) as Signal<T>;
    read.set = (next: T) => {
        if (Object.is(next, value)) return;
        value = next;
        shared.version++;
    };
    read.update = (fn: (v: T) => T) => read.set(fn(value));
    return read;
}

/** Mark every mounted tree for re-render (for state kept outside signals). */
export function invalidate(): void {
    shared.version++;
}

// ---------------------------------------------------------------------------------------------
// Virtual nodes

export type Dim = number | `${number}%` | "auto";
export type Edge = number | [number, number] | [number, number, number, number] | { top?: number; right?: number; bottom?: number; left?: number };
export type ColorValue = string | [number, number, number] | [number, number, number, number] | null;

export interface StyleProps {
    width?: Dim;
    height?: Dim;
    minWidth?: Dim;
    minHeight?: Dim;
    maxWidth?: Dim;
    maxHeight?: Dim;
    flex?: number;
    flexGrow?: number;
    flexShrink?: number;
    flexBasis?: Dim;
    direction?: "row" | "column" | "row-reverse" | "column-reverse";
    wrap?: boolean | "wrap" | "nowrap" | "wrap-reverse";
    justify?: "start" | "center" | "end" | "space-between" | "space-around" | "space-evenly";
    align?: "start" | "center" | "end" | "stretch" | "baseline";
    alignSelf?: "auto" | "start" | "center" | "end" | "stretch" | "baseline";
    margin?: Edge;
    padding?: Edge;
    gap?: number | [number, number];
    position?: "relative" | "absolute" | "static";
    left?: number | string;
    top?: number | string;
    right?: number;
    bottom?: number;
    overflow?: "visible" | "hidden" | "scroll";
    display?: "flex" | "none";
    background?: ColorValue;
    borderColor?: ColorValue;
    border?: number;
    radius?: number;
    opacity?: number;
    color?: ColorValue;
    fontSize?: number;
    textAlign?: "left" | "center" | "right";
    textWrap?: boolean;
    scrollTop?: number;
}

export interface EventProps {
    onClick?: (e: UiEvent) => void;
    onMouseDown?: (e: UiEvent) => void;
    onMouseUp?: (e: UiEvent) => void;
    onInput?: (e: UiEvent) => void;
    onChange?: (e: UiEvent) => void;
    onKeyDown?: (e: UiEvent) => void;
    onWheel?: (e: UiEvent) => void;
    onHover?: (e: UiEvent) => void;
    onFocus?: (e: UiEvent) => void;
    onBlur?: (e: UiEvent) => void;
    onDrag?: (e: UiEvent) => void;
    onDragEnd?: (e: UiEvent) => void;
}

export interface CommonProps extends StyleProps, EventProps {
    key?: string | number;
    /** Name shown in snapshots and queries, so agents can find the element. */
    name?: string;
    disabled?: boolean;
}

export interface BoxProps extends CommonProps { children?: unknown }
export interface TextProps extends CommonProps { children?: unknown; text?: string }
export interface InputProps extends CommonProps { value?: string; placeholder?: string }

export type VProps = Record<string, unknown> & { key?: string | number };
export type Component<P = VProps> = (props: P & { children?: VNode[] }) => VNode | VNode[] | string | number | null | undefined | boolean;
export type ElementType = "box" | "text" | "input" | typeof Fragment | Component<never>;

export interface VNode {
    type: ElementType;
    props: VProps;
    children: VNode[];
    key?: string | number;
    // Assigned when rendered.
    id?: number;
    text?: string;
}

export const Fragment = Symbol("Fragment");

function textNode(text: string): VNode {
    return { type: "text", props: {}, children: [], text };
}

function flatten(children: unknown[], out: VNode[]): void {
    for (const c of children) {
        if (c === null || c === undefined || typeof c === "boolean") continue;
        if (Array.isArray(c)) flatten(c, out);
        else if (typeof c === "string" || typeof c === "number") out.push(textNode(String(c)));
        else out.push(c as VNode);
    }
}

export function createElement(type: ElementType, props: VProps | null, ...children: unknown[]): VNode {
    const p = { ...(props ?? {}) } as VProps;
    delete (p as { children?: unknown }).children;
    const kids: VNode[] = [];
    flatten(children, kids);
    return { type, props: p, children: kids, key: p.key };
}

export const h = createElement;

// A text element whose children are strings joins them into its text.
function collectText(node: VNode): string {
    if (node.text !== undefined) return node.text;
    let s = typeof node.props.text === "string" ? node.props.text : "";
    for (const c of node.children) s += collectText(c);
    return s;
}

const eventNames: Record<string, string> = {
    onClick: "click", onMouseDown: "mousedown", onMouseUp: "mouseup", onInput: "input", onChange: "change", onKeyDown: "keydown",
    onWheel: "wheel", onHover: "hover", onFocus: "focus", onBlur: "focus", onDrag: "drag", onDragEnd: "drag",
};

// Expand components and fragments into a tree of intrinsic elements.
function expand(node: VNode): VNode[] {
    if (typeof node.type === "function") {
        const out = (node.type as Component)({ ...(node.props as object), children: node.children } as never);
        const kids: VNode[] = [];
        flatten(Array.isArray(out) ? out : [out], kids);
        const result: VNode[] = [];
        for (const k of kids) result.push(...expand(k));
        if (node.key !== undefined && result.length === 1 && result[0].key === undefined) result[0].key = node.key;
        return result;
    }
    if (node.type === Fragment) {
        const result: VNode[] = [];
        for (const k of node.children) result.push(...expand(k));
        return result;
    }
    if (node.type === "text") {
        return [{ type: "text", props: node.props, children: [], key: node.key, text: collectText(node) }];
    }
    const children: VNode[] = [];
    for (const k of node.children) children.push(...expand(k));
    return [{ type: node.type, props: node.props, children, key: node.key }];
}

function styleOf(node: VNode): Record<string, unknown> {
    const out: Record<string, unknown> = {};
    const on: string[] = [];
    for (const [k, v] of Object.entries(node.props)) {
        if (k === "key" || k === "children") continue;
        if (k in eventNames) {
            if (typeof v === "function") {
                const ev = eventNames[k];
                if (!on.includes(ev)) on.push(ev);
            }
            continue;
        }
        if (k === "text" && node.type === "text") continue;
        out[k] = v;
    }
    if (on.length > 0) out.on = on;
    return out;
}

function handlersOf(node: VNode): Record<string, (e: UiEvent) => void> | undefined {
    let out: Record<string, (e: UiEvent) => void> | undefined;
    for (const [k, v] of Object.entries(node.props)) {
        if (k in eventNames && typeof v === "function") {
            out ??= {};
            out[k] = v as (e: UiEvent) => void;
        }
    }
    return out;
}

function sameProps(a: Record<string, unknown>, b: Record<string, unknown>): boolean {
    const ka = Object.keys(a), kb = Object.keys(b);
    if (ka.length !== kb.length) return false;
    for (const k of ka) {
        const x = a[k], y = b[k];
        if (x === y) continue;
        if (typeof x === "object" && typeof y === "object" && x !== null && y !== null) {
            if (JSON.stringify(x) !== JSON.stringify(y)) return false;
            continue;
        }
        return false;
    }
    return true;
}

// ---------------------------------------------------------------------------------------------
// Reconciler: turns the difference between two expanded trees into engine operations.

const contextName: string = (() => {
    const g = globalThis as unknown as { __pocket_bundle?: unknown };
    return typeof g.__pocket_bundle === "string" ? g.__pocket_bundle : "main";
})();

class Mount {
    current: VNode[] = [];
    version = 0;
    context = contextName;
    parent: number;
    constructor(public render: () => unknown, public explicitParent: number | undefined) {
        this.parent = explicitParent ?? (contextName === "editor" ? ROOT : shared.projectRoot);
    }

    update(ops: UiOp[]): void {
        // Project interfaces follow the project root (the editor's scene pane) even when they
        // mounted before the editor existed.
        const desired = this.explicitParent ?? (contextName === "editor" ? ROOT : shared.projectRoot);
        if (desired !== this.parent) {
            this.parent = desired;
            for (const c of this.current) if (c.id !== undefined) ops.push(["append", desired, c.id]);
        }
        const root = createElement(Fragment, null, this.render());
        const next = expand(root);
        patchChildren(this.parent, this.current, next, ops);
        this.current = next;
    }

    unmount(ops: UiOp[]): void {
        for (const c of this.current) removeNode(c, ops);
        this.current = [];
    }
}

function createNode(node: VNode, ops: UiOp[]): number {
    const id = shared.nextId++;
    node.id = id;
    ops.push(["create", id, node.type as "box" | "text" | "input"]);
    const style = styleOf(node);
    if (Object.keys(style).length > 0) ops.push(["set", id, style]);
    if (node.type === "text" && node.text !== undefined && node.text !== "") ops.push(["text", id, node.text]);
    const handlers = handlersOf(node);
    if (handlers) shared.handlers.set(id, handlers);
    for (const c of node.children) {
        createNode(c, ops);
        ops.push(["append", id, c.id as number]);
    }
    return id;
}

function removeNode(node: VNode, ops: UiOp[]): void {
    if (node.id === undefined) return;
    forget(node);
    ops.push(["remove", node.id]);
}

function forget(node: VNode): void {
    if (node.id !== undefined) shared.handlers.delete(node.id);
    for (const c of node.children) forget(c);
}

function patchNode(prev: VNode, next: VNode, ops: UiOp[]): void {
    const id = prev.id as number;
    next.id = id;
    const ps = styleOf(prev), ns = styleOf(next);
    if (!sameProps(ps, ns)) {
        // Send the full style: the engine merges keys, and dropped keys need explicit resets.
        const patch: Record<string, unknown> = { ...ns };
        for (const k of Object.keys(ps)) if (!(k in ns)) patch[k] = null;
        ops.push(["set", id, patch]);
    }
    if (next.type === "text" && prev.text !== next.text) ops.push(["text", id, next.text ?? ""]);
    const handlers = handlersOf(next);
    if (handlers) shared.handlers.set(id, handlers);
    else shared.handlers.delete(id);
    patchChildren(id, prev.children, next.children, ops);
}

function keyOf(node: VNode, index: number): string {
    return node.key !== undefined ? `k:${String(node.key)}` : `i:${index}`;
}

function patchChildren(parent: number, prev: VNode[], next: VNode[], ops: UiOp[]): void {
    const prevByKey = new Map<string, VNode>();
    prev.forEach((n, i) => prevByKey.set(keyOf(n, i), n));
    const used = new Set<VNode>();
    // Order after patching: the ids of next children in order.
    const order: number[] = [];
    next.forEach((n, i) => {
        const old = prevByKey.get(keyOf(n, i));
        if (old && !used.has(old) && old.type === n.type) {
            used.add(old);
            patchNode(old, n, ops);
        } else {
            createNode(n, ops);
        }
        order.push(n.id as number);
    });
    for (const old of prev) if (!used.has(old)) removeNode(old, ops);
    // Re-append moved or new children so the engine's order matches. Appending an existing child
    // moves it; we only touch children whose position changed.
    const prevOrder = prev.filter((n) => used.has(n)).map((n) => n.id as number);
    let pi = 0;
    for (let i = 0; i < order.length; i++) {
        const id = order[i];
        if (prevOrder[pi] === id) {
            pi++;
            continue;
        }
        ops.push(["append", parent, id, i]);
        const at = prevOrder.indexOf(id);
        if (at >= 0) prevOrder.splice(at, 1);
    }
}

/**
 * Mount a render function under an element (the root by default). The function runs now and
 * again after any signal changes; only the differences reach the engine.
 */
export function mount(render: () => unknown, parent?: number): { unmount(): void; update(): void } {
    const m = new Mount(render, parent);
    shared.mounts.push(m);
    const ops: UiOp[] = [];
    m.update(ops);
    m.version = shared.version;
    ui.apply(ops);
    return {
        unmount() {
            const idx = shared.mounts.indexOf(m);
            if (idx >= 0) shared.mounts.splice(idx, 1);
            const rm: UiOp[] = [];
            m.unmount(rm);
            ui.apply(rm);
        },
        update() {
            const up: UiOp[] = [];
            m.update(up);
            m.version = shared.version;
            ui.apply(up);
        },
    };
}

/** Re-render mounted trees whose inputs changed. The SDK calls this every frame. */
export function flushUi(): void {
    if (shared.seen === shared.version) return;
    // A render may set signals; loop a few times, then give up to avoid a livelock.
    for (let round = 0; round < 4 && shared.seen !== shared.version; round++) {
        shared.seen = shared.version;
        const ops: UiOp[] = [];
        for (const m of shared.mounts) {
            m.update(ops);
            m.version = shared.version;
        }
        ui.apply(ops);
    }
}

/** Deliver engine UI events to handlers. The SDK calls this from __pocket_dispatch("ui"). */
export function dispatchUiEvents(events: UiEvent[]): void {
    for (const e of events) {
        const h = shared.handlers.get(e.id);
        if (!h) continue;
        switch (e.type) {
            case "click": h.onClick?.(e); break;
            case "mousedown": h.onMouseDown?.(e); break;
            case "mouseup": h.onMouseUp?.(e); break;
            case "input": h.onInput?.(e); break;
            case "change": h.onChange?.(e); break;
            case "keydown": h.onKeyDown?.(e); break;
            case "wheel": h.onWheel?.(e); break;
            case "hover": h.onHover?.(e); break;
            case "focus": h.onFocus?.(e); break;
            case "blur": h.onBlur?.(e); break;
            case "drag": h.onDrag?.(e); break;
            case "dragend": h.onDragEnd?.(e); break;
        }
    }
    flushUi();
}

/** Make project interfaces mount under an element (the editor's scene pane) instead of the root. */
export function setProjectRoot(id: number): void {
    if (shared.projectRoot === id) return;
    shared.projectRoot = id;
    shared.version++;
}

/** Unmount everything a script context mounted (used when a bundle is reloaded). */
export function unmountContext(name: string): void {
    const ops: UiOp[] = [];
    const keep: Mount[] = [];
    for (const m of shared.mounts) {
        if (m.context === name) m.unmount(ops);
        else keep.push(m);
    }
    shared.mounts.length = 0;
    shared.mounts.push(...keep);
    ui.apply(ops);
}

// ---------------------------------------------------------------------------------------------
// Ready-made pieces. Plain functions of props: no hidden state.

export const theme = {
    panel: "#1b1d23",
    panelAlt: "#22252c",
    border: "#3a3e48",
    text: "#e6e6e6",
    muted: "#9aa0ab",
    accent: "#4f8cff",
    accentText: "#ffffff",
    danger: "#e5484d",
    ok: "#3dbf6d",
    fontSize: 13,
};

export function Button(props: { label: string; onClick?: (e: UiEvent) => void; primary?: boolean; danger?: boolean; disabled?: boolean; name?: string; width?: Dim; small?: boolean }): VNode {
    const bg = props.disabled ? "#2a2d34" : props.primary ? theme.accent : props.danger ? theme.danger : theme.panelAlt;
    return h("box", {
        name: props.name ?? props.label,
        onClick: props.disabled ? undefined : props.onClick,
        background: bg,
        borderColor: props.primary || props.danger ? bg : theme.border,
        border: 1,
        radius: 4,
        padding: props.small ? [2, 8] : [5, 12],
        justify: "center",
        align: "center",
        width: props.width,
        disabled: props.disabled,
    }, h("text", { color: props.disabled ? theme.muted : props.primary || props.danger ? theme.accentText : theme.text, fontSize: props.small ? 12 : theme.fontSize }, props.label));
}

export function Label(props: { text: string; muted?: boolean; size?: number; align?: "left" | "center" | "right"; wrap?: boolean; color?: ColorValue; flex?: number; name?: string }): VNode {
    return h("text", { color: props.color ?? (props.muted ? theme.muted : theme.text), fontSize: props.size ?? theme.fontSize, textAlign: props.align, textWrap: props.wrap, flex: props.flex, name: props.name }, props.text);
}

export function Panel(props: { title?: string; children?: unknown; flex?: number; width?: Dim; height?: Dim; padding?: Edge; gap?: number; scroll?: boolean; name?: string; direction?: "row" | "column" }): VNode {
    return h("box", { name: props.name ?? props.title, background: theme.panel, borderColor: theme.border, border: 1, flex: props.flex, width: props.width, height: props.height, direction: "column", overflow: "hidden" },
        props.title !== undefined ? h("box", { background: theme.panelAlt, padding: [4, 8] }, h("text", { color: theme.muted, fontSize: 12 }, props.title)) : null,
        h("box", { flexGrow: 1, flexShrink: 1, padding: props.padding ?? 6, gap: props.gap ?? 4, overflow: props.scroll ? "scroll" : "hidden", direction: props.direction ?? "column" }, props.children));
}

export function TextInput(props: { value: string; onChange?: (value: string) => void; onInput?: (value: string) => void; placeholder?: string; width?: Dim; flex?: number; name?: string; disabled?: boolean }): VNode {
    return h("input", {
        name: props.name,
        value: props.value,
        placeholder: props.placeholder,
        width: props.width,
        flex: props.flex,
        disabled: props.disabled,
        color: theme.text,
        fontSize: theme.fontSize,
        onChange: props.onChange ? (e: UiEvent) => props.onChange?.(e.value ?? "") : undefined,
        onInput: props.onInput ? (e: UiEvent) => props.onInput?.(e.value ?? "") : undefined,
    });
}

export function Row(props: { children?: unknown; gap?: number; align?: StyleProps["align"]; justify?: StyleProps["justify"]; padding?: Edge; flex?: number; wrap?: boolean; height?: Dim; name?: string }): VNode {
    return h("box", { direction: "row", gap: props.gap ?? 6, align: props.align ?? "center", justify: props.justify, padding: props.padding, flex: props.flex, wrap: props.wrap, height: props.height, name: props.name }, props.children);
}

export function Column(props: { children?: unknown; gap?: number; align?: StyleProps["align"]; padding?: Edge; flex?: number; width?: Dim; name?: string; scroll?: boolean }): VNode {
    return h("box", { direction: "column", gap: props.gap ?? 4, align: props.align, padding: props.padding, flex: props.flex, width: props.width, name: props.name, overflow: props.scroll ? "scroll" : undefined }, props.children);
}
