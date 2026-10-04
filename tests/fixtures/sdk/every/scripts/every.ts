// A project component with one field of every type, and a compile-time proof that the value type
// its builder calls imply and the one the engine emits into components.d.ts are the same type.
import { component, field } from "pocket";
import type { Components, Entity, FieldDef } from "pocket";

export const Every = component("Every", {
    version: 1, doc: "One field of every type.",
    fields: {
        a_f64: field.f64(1.5, "A double."),
        a_i32: field.i32(-2, "A 32-bit integer."),
        a_u32: field.u32(3, "A 32-bit unsigned integer."),
        a_tick: field.tick(0, "A tick."),
        a_bool: field.bool(true, "A bool."),
        a_str: field.str("hi", "A string."),
        a_entity: field.entity("An entity."),
        a_vec2: field.vec2({ x: 1, y: 2 }, "A 2-vector."),
        a_vec3: field.vec3({ x: 1, y: 2, z: 3 }, "A 3-vector."),
        a_vec4: field.vec4({ x: 1, y: 2, z: 3, w: 4 }, "A 4-vector."),
        a_quat: field.quat({ x: 0, y: 0, z: 0, w: 1 }, "A rotation."),
        a_enum: field.enum(["calm", "cross"], "cross", "A mood."),
    },
});

/** The value type the builder's fields imply (an entity field holds an id or null). */
type FromBuilder<F> = {
    readonly [K in keyof F]: F[K] extends FieldDef<"entity", unknown>
        ? Entity | null
        : F[K] extends FieldDef<infer _T, infer V>
          ? V
          : never;
};
type Same<A, B> = [A] extends [B] ? ([B] extends [A] ? true : false) : false;

/** `true` only when the two are mutually assignable; `false` is a type error here. */
export const builderMatchesEmitter: Same<FromBuilder<typeof Every.def.fields>, Components["Every"]> = true;
