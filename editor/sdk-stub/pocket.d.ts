// A stand-in for the `pocket` module's declarations (docs/spec/script-host.md 5.1), loaded into the
// script editor's TypeScript worker until sdk/ exposes the generated .d.ts. Component values and
// columns are loose here; the generated declarations type them per component.
declare module "pocket" {
  /** The generated SDK brands it; the stub keeps a plain number. */
  export type Entity = number;
  export type ComponentName = string;
  export type Data = null | boolean | number | string | Data[] | { [key: string]: Data };
  export type ScriptPhase = "update";

  export interface Rng {
    next(): number;
    int(lo: number, hi: number): number;
    range(lo: number, hi: number): number;
    chance(p: number): boolean;
    pick<T>(items: readonly T[]): T;
    shuffle<T>(items: T[]): T[];
    weighted(weights: readonly number[]): number;
    normal(mean: number, sd: number): number;
    fill(out: Float64Array): void;
  }

  export interface GameEvent {
    readonly seq: number;
    readonly tick: number;
    readonly kind: string;
    readonly subject: Entity | null;
    readonly data: Data;
    readonly cause: number | null;
  }

  export interface World {
    exists(e: Entity): boolean;
    has(e: Entity, c: ComponentName): boolean;
    get(e: Entity, c: ComponentName): Readonly<Record<string, any>> | undefined;
    set(e: Entity, c: ComponentName, patch: Record<string, unknown>): void;
    insert(e: Entity, c: ComponentName, value?: Record<string, unknown>): void;
    remove(e: Entity, c: ComponentName): void;
    despawn(e: Entity): void;
    spawn(components: Record<ComponentName, Record<string, unknown>>): Entity;
  }

  /** One Float64Array per numeric slot: `cols.Transform.position.x[row]`. */
  export type Column = Float64Array & { [axis: string]: Float64Array };
  export type ComponentColumns = { readonly [field: string]: Column };

  export interface QuerySpec {
    readonly with: readonly ComponentName[];
    readonly without?: readonly ComponentName[];
    /** "Transform.position": only these columns. */
    readonly fields?: readonly string[];
  }

  export interface QueryResult {
    readonly len: number;
    readonly ids: Float64Array;
    readonly cols: { readonly [component: string]: ComponentColumns };
    id(row: number): Entity;
    row(e: Entity): number;
    each(fn: (row: number, e: Entity) => void): void;
  }

  export interface SystemContext {
    readonly tick: number;
    readonly dt: number;
    readonly time: number;
    readonly system: string;
    readonly world: World;
    readonly rng: Rng;
    readonly events: readonly GameEvent[];
    readonly part: { entity(e: Entity): unknown };
    rngFor(entity: Entity): Rng;
    rngNamed(...parts: unknown[]): Rng;
    rngTimeless(...parts: unknown[]): Rng;
    single(component: ComponentName): Entity;
    query(spec: QuerySpec): QueryResult;
    emit(kind: string, data?: Data, options?: { subject?: Entity; cause?: number }): void;
    intent(actor: Entity, kind: string, params?: Data): void;
  }

  export interface SystemDef<Q extends Record<string, QuerySpec>> {
    name: string;
    phase: ScriptPhase;
    doc: string;
    when?: "start" | { every: number; offset?: number };
    queries?: Q;
    events?: readonly string[];
    run(ctx: SystemContext, queries: { [K in keyof Q]: QueryResult }): void;
  }

  export interface FieldDef<T> {
    readonly __field: T;
  }

  export const field: {
    f64(def: number, doc: string): FieldDef<number>;
    i32(def: number, doc: string): FieldDef<number>;
    u32(def: number, doc: string): FieldDef<number>;
    tick(def: number, doc: string): FieldDef<number>;
    bool(def: boolean, doc: string): FieldDef<boolean>;
    entity(doc: string): FieldDef<Entity | null>;
    str(def: string, doc: string): FieldDef<string>;
    enumOf<V extends string>(variants: readonly V[], def: V, doc: string): FieldDef<V>;
    vec2(def: { x: number; y: number }, doc: string): FieldDef<{ x: number; y: number }>;
    vec3(def: { x: number; y: number; z: number }, doc: string): FieldDef<{ x: number; y: number; z: number }>;
    quat(def: { x: number; y: number; z: number; w: number }, doc: string): FieldDef<{ x: number; y: number; z: number; w: number }>;
  };

  export interface ComponentDecl {
    readonly name: string;
  }

  export function component(name: string, def: { version: number; doc: string; fields: Record<string, FieldDef<unknown>> }): ComponentDecl;
  export function system<Q extends Record<string, QuerySpec>>(def: SystemDef<Q>): unknown;
  export function executor(def: { name: string; phase: ScriptPhase; doc: string; intents: readonly string[]; run(ctx: unknown): void }): unknown;
  export function game(def: { components?: ComponentDecl[]; systems?: unknown[]; retired?: unknown[] }): unknown;
}
