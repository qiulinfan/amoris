// On-screen controls for touch screens (docs/design/input.md, On-screen controls): a stick that sets
// one or two actions' values as it tilts, and buttons that hold an action while a finger is on them.
// They read the fingers' own events (any number at once), feed the action map through input.axis,
// and draw themselves with the interface; a game reads the same actions it reads from the keyboard
// and a pad, and an agent can drive them with input.axis or input.touch.
import { own } from "./registry";
import { command } from "./world";
import { h, mount } from "./ui";
import type { InputEvent } from "./pocket";

/** A place on the window: a corner's distances in points, the control's center from the left or right and the top or bottom edge. */
export interface Anchor {
    left?: number;
    right?: number;
    top?: number;
    bottom?: number;
}

export interface StickOptions extends Anchor {
    /** The action its tilt across sets (-1 left .. 1 right). */
    x?: string;
    /** The action its tilt down the screen sets (-1 up .. 1 down; `invertY` turns it the other way). */
    y?: string;
    invertY?: boolean;
    /** How far the knob travels, in points (60). */
    radius?: number;
    /** The share of the radius that counts as no tilt (0.15). */
    deadZone?: number;
}

export interface ButtonOptions extends Anchor {
    /** The action held while a finger is on it. */
    action: string;
    /** Its radius in points (36). */
    radius?: number;
    label?: string;
}

interface Stick extends Required<Pick<StickOptions, "radius" | "deadZone" | "invertY">> { opts: StickOptions; finger: number; dx: number; dy: number; sent: [number, number] }
interface Button { opts: ButtonOptions; radius: number; fingers: Set<number>; sent: boolean }

const sticks: Stick[] = [];
const buttons: Button[] = [];
let size = { width: 0, height: 0 };
let shown = false;
let always = false;
let drawn: { update(): void } | undefined;

function center(a: Anchor): { x: number; y: number } {
    const x = a.left !== undefined ? a.left : size.width - (a.right ?? 0);
    const y = a.top !== undefined ? a.top : size.height - (a.bottom ?? 0);
    return { x, y };
}

function setAxis(action: string | undefined, value: number): void {
    if (action) command("input.axis", { action, value });
}

let busy = false;

function onTouch(events: InputEvent[]): void {
    // input.axis hands its own event back to the input handlers at once: not taken in again.
    if (busy) return;
    busy = true;
    try {
        feed(events);
    } finally {
        busy = false;
    }
}

function feed(events: InputEvent[]): void {
    for (const e of events) {
        if (e.type === "resize" && e.width !== undefined && e.height !== undefined) size = { width: e.width, height: e.height };
        if (e.type !== "touch_down" && e.type !== "touch_move" && e.type !== "touch_up") continue;
        if (!shown) {
            shown = true;
            drawn?.update();
        }
        const f = e.finger ?? 0;
        const x = e.x ?? 0, y = e.y ?? 0;
        if (e.type === "touch_down") {
            for (const s of sticks) {
                const c = center(s.opts);
                if (s.finger < 0 && Math.hypot(x - c.x, y - c.y) <= s.radius * 1.6) { s.finger = f; break; }
            }
            for (const b of buttons) {
                const c = center(b.opts);
                if (Math.hypot(x - c.x, y - c.y) <= b.radius) b.fingers.add(f);
            }
        }
        for (const s of sticks) {
            if (s.finger !== f) continue;
            if (e.type === "touch_up") { s.finger = -1; s.dx = 0; s.dy = 0; continue; }
            const c = center(s.opts);
            let dx = (x - c.x) / s.radius, dy = (y - c.y) / s.radius;
            const len = Math.hypot(dx, dy);
            if (len > 1) { dx /= len; dy /= len; }
            s.dx = dx;
            s.dy = dy;
        }
        if (e.type === "touch_up") for (const b of buttons) b.fingers.delete(f);
    }
    // Only what changed goes to the action map.
    for (const s of sticks) {
        const len = Math.hypot(s.dx, s.dy);
        const k = len <= s.deadZone ? 0 : (len - s.deadZone) / (1 - s.deadZone) / Math.max(len, 1e-6);
        const vx = Number((s.dx * k).toFixed(3)), vy = Number((s.dy * k * (s.invertY ? -1 : 1)).toFixed(3));
        if (vx !== s.sent[0]) { s.sent[0] = vx; setAxis(s.opts.x, vx); }
        if (vy !== s.sent[1]) { s.sent[1] = vy; setAxis(s.opts.y, vy); }
    }
    for (const b of buttons) {
        const on = b.fingers.size > 0;
        if (on !== b.sent) { b.sent = on; setAxis(b.opts.action, on ? 1 : 0); }
    }
    drawn?.update();
}

function draw(): unknown {
    if (!shown && !always) return null;
    const parts: unknown[] = [];
    for (const s of sticks) {
        const c = center(s.opts);
        const r = s.radius;
        parts.push(h("box", { position: "absolute", left: c.x - r, top: c.y - r, width: r * 2, height: r * 2, radius: r, background: "#ffffff22", border: 2, borderColor: "#ffffff55", pointerEvents: "none" }));
        parts.push(h("box", { position: "absolute", left: c.x + s.dx * r - r * 0.45, top: c.y + s.dy * r - r * 0.45, width: r * 0.9, height: r * 0.9, radius: r * 0.45, background: s.finger >= 0 ? "#ffffff99" : "#ffffff55", pointerEvents: "none" }));
    }
    for (const b of buttons) {
        const c = center(b.opts);
        const r = b.radius;
        parts.push(h("box", { position: "absolute", left: c.x - r, top: c.y - r, width: r * 2, height: r * 2, radius: r, background: b.fingers.size > 0 ? "#ffffff88" : "#ffffff33", border: 2, borderColor: "#ffffff66", justify: "center", align: "center", pointerEvents: "none" },
            b.opts.label ? h("text", { color: "#ffffff", fontSize: Math.round(r * 0.6), pointerEvents: "none" }, b.opts.label) : null));
    }
    return h("box", { position: "absolute", left: 0, top: 0, right: 0, bottom: 0, pointerEvents: "none", name: "touch-controls" }, ...parts);
}

function ensure(): void {
    if (drawn) return;
    const info = command<{ width: number; height: number }>("window.info", {});
    size = { width: info.width, height: info.height };
    own.input.push(onTouch);
    drawn = mount(draw);
}

export const touch = {
    /** A stick: drag from near it to tilt; sets `x` and `y` actions from -1 to 1 (input.axis). */
    stick(options: StickOptions): void {
        ensure();
        sticks.push({ opts: options, radius: options.radius ?? 60, deadZone: options.deadZone ?? 0.15, invertY: options.invertY ?? false, finger: -1, dx: 0, dy: 0, sent: [0, 0] });
        drawn?.update();
    },
    /** A button: holds `action` while a finger is on it. */
    button(options: ButtonOptions): void {
        ensure();
        buttons.push({ opts: options, radius: options.radius ?? 36, fingers: new Set(), sent: false });
        drawn?.update();
    },
    /** Draw the controls from the start rather than from the first touch (on a phone, or to look at them). */
    show(on = true): void {
        always = on;
        drawn?.update();
    },
};
