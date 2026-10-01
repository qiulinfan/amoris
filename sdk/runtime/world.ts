// World access for gameplay code: entities, components, tree, queries, scenes.
//
// Every call goes through one native command with JSON parameters; the same commands are served
// over HTTP to external agents, so what a script can do, an agent can do, with the same words.
import type { ComponentEnums, ComponentName, Components, Vec3 } from "./generated/components";

export type { ComponentName, Components, Vec2, Vec3, Vec4, Quat, Color } from "./generated/components";
export { componentNames, componentDefaults, derivedComponents } from "./generated/components";

declare const __pocket: { command(name: string, params?: unknown): unknown; __pack_data?: Float32Array; __pack_ids?: Float64Array };

/** Entity handle (a stable id) or a path such as "/Level/Player" or a bare name. */
export type Entity = number;
export type EntityRef = Entity | string;

export type DeepPartial<T> = { [K in keyof T]?: T[K] extends object ? DeepPartial<T[K]> : T[K] };
/** A component as a write gives it: a field with value names takes the name too (`Light.kind: "point"`); reads give numbers. */
export type ComponentInput<K extends ComponentName> = { [F in keyof Components[K]]: F extends keyof ComponentEnums[K] ? Components[K][F] | ComponentEnums[K][F] : Components[K][F] };
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
        const v = command<Components[K] | null>("world.get", { entity, component });
        return v === null ? undefined : v;
    },
    set<K extends ComponentName>(entity: EntityRef, component: K, value: DeepPartial<ComponentInput<K>>, cause?: number): void {
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
     * Read a model now and describe it: glTF, OBJ (with its MTL) and STL are read by the engine; .blend,
     * .fbx, .dae, .usd, .abc and .ply go through Blender once per file content into `.imported/` (`force`
     * converts again). Any mesh path works in a MeshRenderer or instantiateMesh without this; it is for
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

export const render = {
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
