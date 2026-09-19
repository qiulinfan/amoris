// World access for gameplay code: entities, components, tree, queries, scenes.
//
// Every call goes through one native command with JSON parameters; the same commands are served
// over HTTP to external agents, so what a script can do, an agent can do, with the same words.
import type { ComponentName, Components } from "./generated/components";

export type { ComponentName, Components, Vec2, Vec3, Vec4, Quat, Color } from "./generated/components";
export { componentNames, componentDefaults, derivedComponents } from "./generated/components";

declare const __pocket: { command(name: string, params?: unknown): unknown; __pack_data?: Float32Array; __pack_ids?: Float64Array };

/** Entity handle (a stable id) or a path such as "/Level/Player" or a bare name. */
export type Entity = number;
export type EntityRef = Entity | string;

export type DeepPartial<T> = { [K in keyof T]?: T[K] extends object ? DeepPartial<T[K]> : T[K] };
export type ComponentPatch = { [K in ComponentName]?: DeepPartial<Components[K]> };

export function command<T = unknown>(name: string, params?: unknown): T {
    return __pocket.command(name, params) as T;
}

export interface SpawnOptions {
    parent?: EntityRef;
    components?: ComponentPatch;
    /** Sequence number of the event that caused this spawn, for the causal log. */
    cause?: number;
}

export interface Packed {
    count: number;
    stride: number;
    /** Offset of each field within one entity's stride. */
    layout: Record<string, number>;
    data: Float32Array;
    ids: Float64Array;
}

export interface TreeOptions {
    root?: EntityRef;
    depth?: number;
    max_entities?: number;
    values?: boolean;
    components?: ComponentName[];
}

export interface QueryOptions {
    with?: ComponentName[];
    without?: ComponentName[];
    name?: string;
    under?: EntityRef;
    fields?: ComponentName[];
    limit?: number;
}

export type QueryRow = { id: Entity; path: string } & { [K in ComponentName]?: Components[K] };

export interface Described {
    id: Entity;
    name: string;
    path: string;
    parent?: Entity;
    children: Array<{ id: Entity; name: string }>;
    components: { [K in ComponentName]?: Components[K] };
}

export interface Scene {
    format: "pocket-scene";
    version: number;
    entities: SceneEntity[];
}

export interface SceneEntity {
    name: string;
    components?: ComponentPatch;
    children?: SceneEntity[];
}

export const world = {
    spawn(name: string, options: SpawnOptions = {}): Entity {
        return command<{ id: Entity }>("world.spawn", { name, ...options }).id;
    },
    destroy(entity: EntityRef, cause?: number): void {
        command("world.destroy", { entity, cause });
    },
    get<K extends ComponentName>(entity: EntityRef, component: K): Components[K] | undefined {
        const v = command<Components[K] | null>("world.get", { entity, component });
        return v === null ? undefined : v;
    },
    set<K extends ComponentName>(entity: EntityRef, component: K, value: DeepPartial<Components[K]>, cause?: number): void {
        command("world.set", { entity, component, value, cause });
    },
    remove(entity: EntityRef, component: ComponentName, cause?: number): void {
        command("world.remove", { entity, component, cause });
    },
    has(entity: EntityRef, component: ComponentName): boolean {
        return command<boolean>("world.has", { entity, component });
    },
    describe(entity: EntityRef): Described {
        return command<Described>("world.describe", { entity });
    },
    find(path: string): Entity | undefined {
        const v = command<Entity | null>("world.find", { path });
        return v === null ? undefined : v;
    },
    children(entity: EntityRef): Entity[] {
        return command<Entity[]>("world.children", { entity });
    },
    roots(): Entity[] {
        return command<Entity[]>("world.roots");
    },
    reparent(entity: EntityRef, parent: EntityRef | null): void {
        command("world.reparent", { entity, parent });
    },
    rename(entity: EntityRef, name: string): void {
        command("world.rename", { entity, name });
    },
    /** The AI-native tree: one line per entity with the fields that differ from defaults. */
    tree(options: TreeOptions = {}): string {
        return command<{ text: string }>("world.tree", options).text;
    },
    query(options: QueryOptions): QueryRow[] {
        return command<{ entities: QueryRow[] }>("world.query", options).entities;
    },
    summary(): { tick: number; entities: number; roots: number; components: Record<string, number>; events: number; hash: string } {
        return command("world.summary");
    },
    /**
     * Spawn a prefab (a scene fragment file under the project, or an inline scene object) and
     * return its root entity. `components` merge onto the root; `parent` places it in the tree.
     */
    instantiate(prefab: string | Scene, options: { parent?: EntityRef; name?: string; components?: ComponentPatch; cause?: number } = {}): Entity {
        const params: Record<string, unknown> = { parent: options.parent, name: options.name, components: options.components, cause: options.cause };
        if (typeof prefab === "string") params.prefab = prefab;
        else params.scene = prefab;
        return command<{ roots: Entity[] }>("world.instantiate", params).roots[0];
    },
    /** Write an entity and its descendants as a prefab file under the project directory. */
    savePrefab(entity: EntityRef, path: string): { path: string; entities: number } {
        return command("world.save_prefab", { entity, path });
    },
    /** Load a scene file by project-relative path (replacing the world unless clear is false). */
    loadScene(path: string, clear = true): number {
        return command<{ entities: number }>("world.load", { path, clear }).entities;
    },
    /**
     * Numeric fields of one component for every matching entity as typed arrays shared with the
     * engine: `data` holds `stride` floats per entity in field order (see `layout`), `ids` the
     * entity ids. Edit `data` in place and call `unpack()` to write it back. The views are valid
     * until the next `pack()`.
     */
    pack(component: ComponentName, fields: string[], options: { with?: ComponentName[]; without?: ComponentName[]; name?: string; under?: EntityRef; limit?: number } = {}): Packed {
        const r = command<{ count: number; stride: number; layout: Record<string, number> }>("world.pack", { component, fields, ...options });
        const data = __pocket.__pack_data as Float32Array;
        const ids = __pocket.__pack_ids as Float64Array;
        return { count: r.count, stride: r.stride, layout: r.layout, data: data.subarray(0, r.count * r.stride), ids: ids.subarray(0, r.count) };
    },
    /** Write the packed `data` back to the entities of the last `pack()` (or the first `count` rows). */
    unpack(count?: number): void {
        command("world.unpack", { count });
    },
    save(): Scene {
        return command<Scene>("world.save");
    },
    load(scene: Scene, clear = true): number {
        return command<{ entities: number }>("world.load", { scene, clear }).entities;
    },
    clear(): void {
        command("world.clear");
    },
};

/** Rendering queries: what is on screen, and where. */
export const render = {
    stats(): { draw_calls: number; meshes: number; point_lights: number; has_camera: boolean; has_sun: boolean; camera?: Entity } {
        return command("render.stats");
    },
    /** Entity under a pixel of the last frame (undefined for background). */
    pick(x: number, y: number): { id: Entity; path?: string; name?: string } | undefined {
        const r = command<{ id: Entity; path?: string; name?: string }>("render.pick", { x, y });
        return r.id === 0 ? undefined : r;
    },
    /** Pixel position of an entity's world position in the last frame. */
    project(entity: EntityRef): { visible: boolean; x?: number; y?: number; inside?: boolean } {
        return command("render.project", { entity });
    },
    /** Entities visible in the last frame with their pixel counts; optionally writes a PNG. */
    ids(path?: string): { width: number; height: number; visible: Array<{ id: Entity; pixels: number; path?: string }> } {
        return command("render.ids", { path });
    },
};

/** The run so far as stable regimes and grouped events (see docs/design/agent-perception.md). */
export function transcript(options: { since_tick?: number; until_tick?: number; max_lines?: number; tolerance?: number } = {}): string {
    return command<{ text: string }>("transcript", options).text;
}
