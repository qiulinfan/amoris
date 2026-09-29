// Actions instead of keys. `input.map` binds names to keys, gamepad buttons ("pad:a", "pad:dpad_left")
// and axes ("pad:leftx"); every tick carries a snapshot, so gameplay reads `input.axis("move_x")`
// and `input.pressed("jump")`. Agents drive the same actions with `input.hold`.
import { registry } from "./registry";

declare const __pocket: { command(name: string, params?: unknown): unknown };

export type Binding = string | string[] | { positive?: string[]; negative?: string[]; axis?: string[]; keys?: string[]; deadzone?: number };

export interface ActionState {
    down: boolean;
    pressed: boolean;
    released: boolean;
    value: number;
}

// The snapshot lives on the shared registry: the bundle whose dispatch runs the tick (the last
// one loaded) fills it, and every bundle's copy of this module reads the same object.
// In a lockstep game every player has their own states (player 0's are `actions`); without one,
// only player 0 has any input, and the others' actions are all at rest.
function snapshot(player = 0): Record<string, ActionState> {
    if (player === 0) return registry.actions as Record<string, ActionState>;
    return (registry.players?.[player] ?? {}) as Record<string, ActionState>;
}

/** Called by the SDK with each tick's snapshot (and every player's, in a lockstep game). */
export function setActionSnapshot(actions: Record<string, ActionState> | undefined, players?: Array<Record<string, ActionState>>): void {
    if (actions) registry.actions = actions;
    registry.players = players;
}

function cmd<T>(name: string, params?: unknown): T {
    return __pocket.command(name, params) as T;
}

const empty: ActionState = { down: false, pressed: false, released: false, value: 0 };

export const input = {
    /** Define (or redefine) the action map. Existing actions keep their live state. */
    map(actions: Record<string, Binding>): void {
        cmd("input.map", { actions });
        registry.actions = cmd<Record<string, ActionState>>("input.actions");
    },
    /** An action's state; `player` picks whose in a lockstep game (docs/design/networking.md), 0 by default. */
    action(name: string, player = 0): ActionState {
        return snapshot(player)[name] ?? empty;
    },
    down(name: string, player = 0): boolean {
        return (snapshot(player)[name] ?? empty).down;
    },
    /** True on the tick the action went down. */
    pressed(name: string, player = 0): boolean {
        return (snapshot(player)[name] ?? empty).pressed;
    },
    released(name: string, player = 0): boolean {
        return (snapshot(player)[name] ?? empty).released;
    },
    /** -1..1 for axis-style actions (negative/positive keys or a pad stick). */
    axis(name: string, player = 0): number {
        return (snapshot(player)[name] ?? empty).value;
    },
    /** Shake a gamepad (by index) for `ms` milliseconds, the low and high motors in 0..1; false without one. */
    rumble(pad = 0, low = 1, high = 1, ms = 200): boolean {
        return cmd<{ rumbled: boolean }>("input.rumble", { pad, low, high, ms }).rumbled;
    },
    /** Play steps of motor strengths (0..1) and lengths one after another on the tick clock, `repeat` times, replacing what the pad was playing; a step with both motors at 0 is a pause. False without a pad that can. */
    rumblePattern(pad: number, pattern: { low?: number; high?: number; ms: number }[], repeat = 1): boolean {
        return cmd<{ rumbled: boolean }>("input.rumble", { pad, pattern, repeat }).rumbled;
    },
    stopRumble(pad = 0): void {
        cmd("input.rumble", { pad, stop: true });
    },
    /** Fresh snapshot straight from the engine (the tick snapshot is what gameplay should use). */
    actions(): Record<string, ActionState> {
        registry.actions = cmd<Record<string, ActionState>>("input.actions");
        return snapshot();
    },
    describe(): Record<string, { positive: string[]; negative?: string[]; axis?: string[]; deadzone?: number }> {
        return cmd("input.describe");
    },
    /** Hold a key, or an action's first key (`sign: -1` for its negative direction), for `ticks` ticks. Synthetic input, journaled. */
    hold(target: { key: string } | { action: string; sign?: -1 | 1 }, ticks = 1): void {
        cmd("input.hold", { ...target, ticks });
    },
    press(target: { key: string } | { action: string }): void {
        cmd("input.press", target);
    },
};
