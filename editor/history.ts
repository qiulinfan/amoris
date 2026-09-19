// Undo/redo for the editor: every edit the interface makes to the world is recorded as a pair of
// closures. Entities get new ids when they come back, so edits that create entities keep the
// current id in shared state and later edits look it up through it.
export interface Edit {
    label: string;
    undo(): void;
    redo(): void;
}

const undos: Edit[] = [];
const redos: Edit[] = [];
const LIMIT = 200;

/** Record an edit that has already been applied. */
export function record(edit: Edit): void {
    undos.push(edit);
    if (undos.length > LIMIT) undos.shift();
    redos.length = 0;
}

/** Apply `redo` now and record it. */
export function perform(label: string, redo: () => void, undo: () => void): void {
    redo();
    record({ label, undo, redo });
}

export function undo(): string | undefined {
    const e = undos.pop();
    if (!e) return undefined;
    e.undo();
    redos.push(e);
    return e.label;
}

export function redo(): string | undefined {
    const e = redos.pop();
    if (!e) return undefined;
    e.redo();
    undos.push(e);
    return e.label;
}

export function canUndo(): boolean {
    return undos.length > 0;
}

export function canRedo(): boolean {
    return redos.length > 0;
}

export function nextUndo(): string | undefined {
    return undos[undos.length - 1]?.label;
}

export function nextRedo(): string | undefined {
    return redos[redos.length - 1]?.label;
}

export function clear(): void {
    undos.length = 0;
    redos.length = 0;
}

export function describe(): { undo: string[]; redo: string[] } {
    return { undo: undos.map((e) => e.label), redo: redos.map((e) => e.label) };
}
