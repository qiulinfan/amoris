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
function snapshot(): Record<string, ActionState> {
    return registry.actions as Record<string, ActionState>;
}

/** Called by the SDK with each tick's snapshot. */
export function setActionSnapshot(actions: Record<string, ActionState> | undefined): void {
    if (actions) registry.actions = actions;
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
    action(name: string): ActionState {
        return snapshot()[name] ?? empty;
    },
    down(name: string): boolean {
        return (snapshot()[name] ?? empty).down;
    },
    /** True on the tick the action went down. */
    pressed(name: string): boolean {
        return (snapshot()[name] ?? empty).pressed;
    },
    released(name: string): boolean {
        return (snapshot()[name] ?? empty).released;
    },
    /** -1..1 for axis-style actions (negative/positive keys or a pad stick). */
    axis(name: string): number {
        return (snapshot()[name] ?? empty).value;
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
