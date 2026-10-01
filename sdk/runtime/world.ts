// World access for gameplay code: entities, components, tree, queries, scenes.
//
// Every call goes through one native command with JSON parameters; the same commands are served
// over HTTP to external agents, so what a script can do, an agent can do, with the same words.
// world.get and world.set of a component whose fields are all numbers (Transform, Velocity,
// Health, RigidBody2D, ...) skip the JSON: the numbers cross in a shared Float64Array, with the
// same values and the same checks, and anything else (a name for a value, a missing component, a
// cause) takes the command.
import { numericLayouts } from "./generated/components";
import type { Color, ComponentEnums, ComponentName, Components, Vec3 } from "./generated/components";

export type { ComponentName, Components, Vec2, Vec3, Vec4, Quat, Color } from "./generated/components";
export { componentNames, componentDefaults, derivedComponents } from "./generated/components";

declare const __pocket: {
    command(name: string, params?: unknown): unknown;
    __pack_data?: Float32Array;
    __pack_ids?: Float64Array;
    __nums?: Float64Array;
    get_nums?(entity: EntityRef, component: string): number;
    set_nums?(entity: EntityRef, component: string, count: number): number;
    component_index?(component: string): number;
    get_nums_at?(entity: number, component: number): number;
    set_nums_at?(entity: number, component: number, count: number): number;
    patch_nums_at?(entity: number, component: number, mask: number): number;
};

// A component's index in the world, asked once per name: the numbers-only natives take it in place
// of the name (an entity given by id; one given by name or path goes through get_nums).
const componentIndex = new Map<string, number>();
function indexOf(component: string): number {
    let i = componentIndex.get(component);
    if (i === undefined) {
        i = __pocket.component_index !== undefined ? __pocket.component_index(component) : -1;
        componentIndex.set(component, i);
    }
    return i;
}
function getNums(entity: EntityRef, component: string): number {
    if (typeof entity === "number" && __pocket.get_nums_at !== undefined) {
        const i = indexOf(component);
        if (i >= 0) return __pocket.get_nums_at(entity, i);
    }
    return __pocket.get_nums!(entity, component);
}
function setNums(entity: EntityRef, component: string, count: number): number {
    if (typeof entity === "number" && __pocket.set_nums_at !== undefined) {
        const i = indexOf(component);
        if (i >= 0) return __pocket.set_nums_at(entity, i, count);
    }
    return __pocket.set_nums!(entity, component, count);
}

type NumKind = "n" | "b" | "v2" | "v3" | "v4" | "q" | "c";
const PARTS: { readonly [K in Exclude<NumKind, "n" | "b">]: readonly string[] } = { v2: ["x", "y"], v3: ["x", "y", "z"], v4: ["x", "y", "z", "w"], q: ["x", "y", "z", "w"], c: ["r", "g", "b", "a"] };

type Layout = ReadonlyArray<readonly [string, NumKind]>;
type Reader = (nums: Float64Array) => Record<string, unknown>;
type Patcher = (nums: Float64Array, patch: Record<string, unknown>, keys: (o: object) => number) => number;

// Each component's reader and patcher are made once, as straight-line functions over its fields:
// an object of one shape built at once, a patch checked field by field without loops or arrays,
// which is what keeps a world.get and world.set a microsecond or so (sdk world.ts, numeric path).
const readers = new Map<Layout, Reader>();
const patchers = new Map<Layout, Patcher>();

function countKeys(o: object): number {
    let n = 0;
    // eslint-disable-next-line @typescript-eslint/no-unused-vars
    for (const _ in o) n++;
    return n;
}

function readNums(layout: Layout, nums: Float64Array): Record<string, unknown> {
    let read = readers.get(layout);
    if (read === undefined) {
        let i = 0;
        const fields: string[] = [];
        for (const [name, kind] of layout) {
            const key = JSON.stringify(name);
            if (kind === "n") fields.push(`${key}: n[${i++}]`);
            else if (kind === "b") fields.push(`${key}: n[${i++}] !== 0`);
            else fields.push(`${key}: { ${PARTS[kind].map((p) => `${p}: n[${i++}]`).join(", ")} }`);
        }
        read = new Function("n", `return { ${fields.join(", ")} };`) as Reader;
        readers.set(layout, read);
    }
    return read(nums);
}

// A patch written into the numbers: the mask of those it gives (bit i for number i), or 0 when it
// holds what only the command takes (a field the component lacks, a value's name, an array, a number
// that is not finite) or the component has more than 31 numbers.
function patchNums(layout: Layout, nums: Float64Array, patch: unknown): number {
    if (typeof patch !== "object" || patch === null || Array.isArray(patch)) return 0;
    let write = patchers.get(layout);
    if (write === undefined) {
        let i = 0;
        const lines = ["let used = 0, g = 0, m = 0, v, x;"];
        for (const [name, kind] of layout) {
            const key = JSON.stringify(name);
            if (kind === "n" || kind === "b") {
                lines.push(`v = p[${key}]; if (v !== undefined) { if (typeof v === "number" && Number.isFinite(v)) n[${i}] = v; else if (typeof v === "boolean") n[${i}] = v ? 1 : 0; else return 0; m |= ${1 << i}; used++; }`);
                i++;
                continue;
            }
            const parts = PARTS[kind];
            let body = `v = p[${key}]; if (v !== undefined) { if (typeof v !== "object" || v === null || Array.isArray(v)) return 0; g = 0;`;
            parts.forEach((part, k) => {
                body += ` x = v.${part}; if (x !== undefined) { if (typeof x !== "number" || !Number.isFinite(x)) return 0; n[${i + k}] = x; m |= ${1 << (i + k)}; g++; }`;
            });
            body += " if (g !== keys(v)) return 0; used++; }";
            lines.push(body);
            i += parts.length;
        }
        lines.push("return used === keys(p) ? m : 0;");
        write = i > 31 ? () => 0 : (new Function("n", "p", "keys", lines.join("\n")) as Patcher);
        patchers.set(layout, write);
    }
    return write(nums, patch as Record<string, unknown>, countKeys);
}

/** Entity handle (a stable id) or a path such as "/Level/Player" or a bare name. */
export type Entity = number;
export type EntityRef = Entity | string;

type DeepPartialField<V> = V extends object ? DeepPartial<V> : V;
export type DeepPartial<T> = { [K in keyof T]?: DeepPartialField<T[K]> };
// A colour field also takes "#rrggbb" (or #rgb, #rrggbbaa), as a colour picker shows it.
type FieldInput<T> = T extends Color ? Color | string : T;
/** A component as a write gives it: a field with value names takes the name too (`Light.kind: "point"`); reads give numbers. */
export type ComponentInput<K extends ComponentName> = { [F in keyof Components[K]]: F extends keyof ComponentEnums[K] ? Components[K][F] | ComponentEnums[K][F] : FieldInput<Components[K][F]> };
export type ComponentPatch = { [K in ComponentName]?: DeepPartial<ComponentInput<K>> };

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
        const layout = numericLayouts[component];
        if (layout !== undefined && __pocket.get_nums !== undefined) {
            const n = getNums(entity, component);
            if (n >= 0) return readNums(layout, __pocket.__nums!) as unknown as Components[K];
            if (n === -1) return undefined;   // the entity has no such component
        }
        const v = command<Components[K] | null>("world.get", { entity, component });
        return v === null ? undefined : v;
    },
    set<K extends ComponentName>(entity: EntityRef, component: K, value: DeepPartial<ComponentInput<K>>, cause?: number): void {
        const layout = numericLayouts[component];
        if (cause === undefined && layout !== undefined && __pocket.get_nums !== undefined) {
            if (typeof entity === "number" && __pocket.patch_nums_at !== undefined) {
                // The numbers the patch gives, laid over the component in the engine: one call.
                const i = indexOf(component);
                const mask = i >= 0 ? patchNums(layout, __pocket.__nums!, value) : 0;
                if (mask !== 0 && __pocket.patch_nums_at(entity, i, mask) >= 0) return;
            } else {
                // The component as it is, the patch over it, written back: when the entity has it and
                // the patch is all numbers.
                const n = getNums(entity, component);
                if (n >= 0 && patchNums(layout, __pocket.__nums!, value) !== 0 && setNums(entity, component, n) >= 0) return;
            }
        }
        command("world.set", { entity, component, value, cause, quiet: true });   // the answer's value is for agents; a script reads with get
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
    /** Move an entity under another (null: to the root); `keepWorld` leaves it where it stands in the world, its local transform taking up the difference. */
    reparent(entity: EntityRef, parent: EntityRef | null, options: { keepWorld?: boolean } = {}): void {
        command("world.reparent", { entity, parent, keep_world: options.keepWorld ?? false });
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
    /** Keep the world as it is now under a name, for diff. */
    mark(name = "default"): { mark: string; tick: number; entities: number } {
        return command("world.mark", { name });
    },
    /** What changed since a mark: entities spawned and destroyed, components added or removed, each changed field then and now. */
    diff(since = "default", limit = 50): { since: string; since_tick: number; tick: number; spawned: Array<{ id: Entity; path: string; components: string[] }>; destroyed: Array<{ id: Entity; path: string }>; changed: Array<{ id: Entity; path: string; changes: Record<string, unknown> }>; counts: { spawned: number; destroyed: number; changed: number } } {
        return command("world.diff", { since, limit });
    },
    summary(): { tick: number; entities: number; roots: number; components: Record<string, number>; events: number; hash: string } {
        return command("world.summary");
    },
    /** What is likely wrong with the world: each problem's severity, entity, component, what it does and a fix. */
    lint(limit?: number): { ok: boolean; errors: number; warnings: number; infos: number; problems: Array<{ severity: "error" | "warning" | "info"; entity?: number; path?: string; component: string; problem: string; fix: string }> } {
        return command("world.lint", limit === undefined ? {} : { limit });
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
    /**
     * A glTF file's node tree as entities under one root: one entity per node with the node's own
     * transform, the nodes with geometry drawing themselves alone (`MeshRenderer.node`), so the
     * file's parts move apart. Skinned files are refused (their joints place them). Returns the root.
     */
    instantiateMesh(path: string, options: { parent?: EntityRef; name?: string; position?: Vec3; cause?: number } = {}): Entity {
        return command<{ roots: Entity[] }>("world.instantiate", { mesh: path, parent: options.parent, name: options.name, position: options.position, cause: options.cause }).roots[0];
    },
    /**
     * Read a model now and describe it: glTF, OBJ (with its MTL), STL, PLY and voxel models (.vox,
     * .voxels) are read by the engine; .blend, .fbx, .dae, .usd and .abc go through Blender once per
     * file content into `.imported/` (`force` converts again). Any mesh path works in a MeshRenderer or instantiateMesh without this; it is for
     * seeing what a file holds (materials, lights, cameras, parts) before using it.
     */
    importModel(path: string, force = false): { path: string; importer: string; converted?: string; cached?: boolean; seconds?: number; blender_found: boolean; mesh: Record<string, unknown> } {
        return command("assets.import", { path, force });
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
    /** The whole world as a scene, or one entity with its descendants as a fragment for `instantiate`. */
    save(entity?: EntityRef): Scene {
        return command<Scene>("world.save", entity === undefined ? {} : { entity });
    },
    load(scene: Scene, clear = true): number {
        return command<{ entities: number }>("world.load", { scene, clear }).entities;
    },
    clear(): void {
        command("world.clear");
    },
};

export interface VisibleEntity {
    id: Entity;
    path?: string;
    name?: string;
    pixels: number;
    coverage: number;
    bounds: { x: number; y: number; width: number; height: number };
    center: { x: number; y: number };
    stale?: boolean;
}

/** Rendering queries: what is on screen, and where. */
export interface FrameComparison {
    match: boolean;
    /** The reference was missing (or `update` was asked) and this frame was written as it. */
    written: boolean;
    differing: number;
    fraction: number;
    tolerance: number;
    threshold: number;
    width: number;
    height: number;
    /** "size" when the reference has other dimensions. */
    reason?: string;
    /** The pixel rectangle holding every differing pixel (x1, y1 exclusive). */
    bounds?: { x0: number; y0: number; x1: number; y1: number };
    diff?: string;
}

export type PostEffectSpec = string | { shader?: string; code?: string; name?: string; params?: number[]; enabled?: boolean };

export const render = {
    /** The project's post effects, in order after the tonemap (docs/design/rendering.md, Post effects): shader files in the project or WGSL code, each defining `fn effect(uv: vec2f) -> vec4f`. Answers per effect ok or the compiler's message. */
    post(effects: PostEffectSpec[]): { ok: boolean; effects: Array<{ name: string; ok: boolean; error?: string }> } {
        return command("render.post", { effects });
    },
    /** The scene, or an entity and what is under it, from standard views at once into one PNG sheet (nothing moves). */
    views(path: string, options: { entity?: EntityRef; views?: Array<"front" | "back" | "right" | "left" | "top" | "bottom" | "perspective"> } = {}): { path: string; views: Array<{ view: string; eye: Vec3 }>; columns: number; rows: number; width: number; height: number; center: Vec3; radius: number; entity: string | null } {
        return command("render.views", { path, ...options });
    },
    stats(): { draw_calls: number; shadow_draws: number; shadows: boolean; instances: number; sprites: number; meshes: number; point_lights: number; has_camera: boolean; has_sun: boolean; camera?: Entity; msaa: number; id_draws: number } {
        return command("render.stats");
    },
    /** Entity under a pixel of the last frame (undefined for background). */
    pick(x: number, y: number): { id: Entity; path?: string; name?: string } | undefined {
        const r = command<{ id: Entity; path?: string; name?: string }>("render.pick", { x, y });
        return r.id === 0 ? undefined : r;
    },
    /** Pixel position of an entity's world position, or of any world point, in the last frame. */
    project(target: EntityRef | { x: number; y: number; z: number }): { visible: boolean; x?: number; y?: number; inside?: boolean } {
        return command("render.project", typeof target === "object" ? { point: target } : { entity: target });
    },
    /**
     * The world ray under a pixel of the last frame, and where it meets an axis plane: "xy" at
     * z = at (2D scenes), "xz" at y = at (a ground plane), "yz" at x = at.
     */
    unproject(x: number, y: number, plane: "xy" | "xz" | "yz" = "xy", at = 0): { origin: Vec3; direction: Vec3; hit: boolean; point?: Vec3; distance?: number } {
        return command("render.unproject", { x, y, plane, at }) as { origin: Vec3; direction: Vec3; hit: boolean; point?: Vec3; distance?: number };
    },
    /**
     * The last frame against a reference PNG in the project (docs/design/rendering.md, Comparing
     * frames): a pixel differs when a channel is off by more than `threshold` (16 of 255), and
     * the frame matches when at most `tolerance` (0.01) of the pixels differ. A missing reference
     * is written from this frame (`written`); `update` rewrites it; `diff` names a PNG to write
     * with the differing pixels in red over the dimmed reference.
     */
    compare(path: string, options: { tolerance?: number; threshold?: number; diff?: string; update?: boolean } = {}): FrameComparison {
        return command<FrameComparison>("render.compare", { path, ...options });
    },
    /** Multisampling of the color pass: 1 (off) or 4; project.toml [render] msaa = 4 sets the default. Ids keep one sample per pixel. */
    msaa(samples?: number): number {
        return command<{ msaa: number }>("render.msaa", samples === undefined ? {} : { samples }).msaa;
    },
    /** Shadow map settings (on by default when a sun exists; project.toml [render] shadows = false turns it off). `softness` is the sun's radius in degrees, its shadows sharp at their casters and softer farther off (0: sharp everywhere); `contact` marches `contact_length` units toward the sun through the depth buffer for the shadows too small for the maps. */
    shadows(settings: { enabled?: boolean; strength?: number; bias?: number; cascades?: number; distance?: number; softness?: number; contact?: boolean; contact_length?: number } = {}): { enabled: boolean; strength: number; bias: number; cascades: number; distance: number; softness: number; contact: boolean; contact_length: number } {
        return command("render.shadows", settings);
    },
    /**
     * Bloom: what is brighter than `threshold` (0..1) is blurred `radius` half-size texels wide and
     * added back at `strength`, so lights, emissive surfaces and white sprites glow. Off by default;
     * project.toml [render] bloom = true (with bloom_threshold, bloom_strength, bloom_radius) sets the default.
     */
    bloom(settings: { enabled?: boolean; threshold?: number; strength?: number; radius?: number } = {}): { enabled: boolean; threshold: number; strength: number; radius: number } {
        return command("render.bloom", settings);
    },
    /**
     * Grading, the frame's look applied last: `exposure` multiplies the frame, `filmic` rolls the top of the
     * range off so an exposure above 1 brightens without clipping, `temperature` warms (1) or cools (-1),
     * `contrast` and `saturation` scale around mid gray (1 as rendered), `tint` washes the result ({r, g, b}
     * or "#rrggbb"), `vignette` darkens the corners (0..1). Off by default; project.toml [render.grade] sets the default.
     */
    /**
     * How the HDR scene becomes the frame. The scene is lit in linear light in a half-float target, so
     * an emissive of 4 is four times white; `exposure` multiplies it, `auto_exposure` meters the frame and
     * brings its average to mid gray (`compensation` in EV on top, adapting at `speed` per second, the
     * metered average kept within `min_ev`..`max_ev`), then `operator` maps it to the screen: "none" clips
     * at white (2D art keeps its exact colors), "aces" and "agx" roll highlights off filmically, "neutral"
     * keeps hues and only compresses near white. `metered` is what the meter settled on after the last frame.
     * project.toml [render.tonemap] sets the default.
     */
    /**
     * Ambient occlusion: the light from all around (a Sky's, or the flat ambient) darkened where
     * geometry crowds a point, so a crate sits on the ground and a crevice is dark. `radius` is how far
     * around a point is looked at (world units), `intensity` how dark a crowded point gets. Off by
     * default; project.toml [render.ao] sets the default.
     */
    ao(settings: { enabled?: boolean; radius?: number; intensity?: number; samples?: number } = {}): { enabled: boolean; radius: number; intensity: number; samples: number } {
        return command("render.ao", settings);
    },
    tonemap(settings: { operator?: "none" | "aces" | "agx" | "neutral"; exposure?: number; auto_exposure?: boolean; compensation?: number; min_ev?: number; max_ev?: number; speed?: number } = {}): { operator: string; exposure: number; auto_exposure: boolean; compensation: number; min_ev: number; max_ev: number; speed: number; metered?: { exposure_ev: number; average_ev: number } } {
        return command("render.tonemap", settings);
    },
    grade(settings: { enabled?: boolean; exposure?: number; filmic?: boolean; temperature?: number; contrast?: number; saturation?: number; tint?: { r: number; g: number; b: number } | string; vignette?: number } = {}): { enabled: boolean; exposure: number; filmic: boolean; temperature: number; contrast: number; saturation: number; tint: { r: number; g: number; b: number }; vignette: number } {
        return command("render.grade", settings);
    },
    /** Entities visible in the last frame with their pixel counts and bounds; optionally writes a PNG of the id buffer. */
    ids(path?: string): { width: number; height: number; count: number; visible: VisibleEntity[] } {
        return command("render.ids", { path });
    },
    /** Engine overlays drawn as lines: colliders (green dynamic, gray static, blue kinematic, yellow triggers), joints, bounds, world axes. */
    debug(flags: { colliders?: boolean; joints?: boolean; bounds?: boolean; axes?: boolean; nav?: boolean; lights?: boolean; all?: boolean } = {}): { colliders: boolean; joints: boolean; bounds: boolean; axes: boolean; nav: boolean; lights: boolean; lines: number } {
        return command("render.debug", flags);
    },
    /** What the camera sees, largest first: coverage (fraction of the frame), pixel bounds, normalized center. */
    visible(limit = 50): VisibleEntity[] {
        return command<{ visible: VisibleEntity[] }>("render.visible", { limit }).visible;
    },
};

/** The run so far as stable regimes and grouped events (see docs/design/agent-perception.md). */
export function transcript(options: { since_tick?: number; until_tick?: number; max_lines?: number; tolerance?: number } = {}): string {
    return command<{ text: string }>("transcript", options).text;
}
