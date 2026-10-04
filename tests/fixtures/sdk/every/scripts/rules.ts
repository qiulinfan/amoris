// One system that uses every API of the prelude (script-host.md 5.1), with the types it expects.
import { system } from "pocket";
import type { BoolColumn, Entity, EntityColumn, EnumColumn, GameEvent, NotInFields, Rng, Vec3Columns, Vec4Columns } from "pocket";

export const all = system({
    name: "every_api", phase: "update", doc: "Uses every API of the prelude.",
    when: { every: 2, offset: 1 },
    events: ["every."],
    queries: {
        rows: {
            with: ["Every", "Transform"],
            without: ["Velocity"],
            fields: ["Every.a_f64", "Every.a_bool", "Every.a_entity", "Every.a_enum", "Every.a_vec3", "Transform.position"],
        },
        whole: { with: ["Every"] },
        none: { with: ["Every"], fields: [] },
    },
    run(ctx, { rows, whole, none }) {
        const f: Float64Array = rows.cols.Every.a_f64;
        const b: BoolColumn = rows.cols.Every.a_bool;
        const en: EntityColumn = rows.cols.Every.a_entity;
        const k: EnumColumn = rows.cols.Every.a_enum;
        const v: Vec3Columns = rows.cols.Every.a_vec3;
        const p: Vec3Columns = rows.cols.Transform.position;
        const ticks: Float64Array = whole.cols.Every.a_tick;
        const quat: Vec4Columns = whole.cols.Every.a_quat;
        rows.each((r, e) => {
            f[r] = f[r] + ctx.dt * v.x[r] + p.y[r] + ticks.length + quat.w.length;
            b[r] = b[r] === 1 ? 0 : 1;
            en[r] = en[r] === 0 ? e : 0;
            k[r] = 1;
            const id: Entity = rows.id(rows.row(e));
            f[r] += id === e ? 0 : 1;
        });
        for (let r = 0; r < whole.len; r++) f[0] += whole.ids[r] > 0 ? 1 : 0;
        const e = ctx.single("Every");
        const got = ctx.world.get(e, "Every");
        if (got) {
            const s: string = got.a_str;
            const mood: "calm" | "cross" = got.a_enum;
            const who: Entity | null = got.a_entity;
            const w: number = got.a_quat.w + got.a_vec2.y + got.a_vec4.z + got.a_i32 + got.a_tick;
            ctx.world.set(e, "Every", {
                a_u32: got.a_u32 + 1,
                a_vec3: { y: w },
                a_enum: mood === "calm" ? "cross" : "calm",
                a_entity: who,
                a_str: s,
                a_bool: !got.a_bool,
            });
        }
        const spawned = ctx.world.spawn({ Every: { a_str: "new" }, Transform: { position: { x: 1, y: 2, z: 3 } } });
        ctx.world.insert(spawned, "Velocity", { linear: { x: 1 } });
        ctx.world.remove(spawned, "Velocity");
        if (ctx.world.exists(spawned) && ctx.world.has(spawned, "Every")) ctx.world.despawn(spawned);
        ctx.emit("every.ran", { tick: ctx.tick, time: ctx.time, system: ctx.system }, { subject: e });
        const seen: readonly GameEvent[] = ctx.events;
        const streams: Rng[] = [ctx.rng, ctx.rngFor(e), ctx.rngNamed("loot", ctx.part.entity(e), 3), ctx.rngTimeless("x")];
        const draws: number = streams[0].next() + streams[1].range(0, 1) + streams[2].int(1, 6) + streams[3].normal(0, 1)
            + streams[0].weighted([1, 2]) + (streams[0].chance(0.5) ? 1 : 0) + seen.length + Math.random();
        const picked: string = streams[0].pick(["a", "b"]);
        const order: number[] = streams[0].shuffle([1, 2, 3]);
        streams[0].fill(new Float64Array(2));
        const q = ctx.query({ with: ["Transform"], fields: ["Transform.position"] });
        const x: Float64Array = q.cols.Transform.position.x;
        const every = ctx.query({ with: ["Every"] });
        const y: Float64Array = every.cols.Every.a_tick;
        // `fields: []` hands over the rows and no column.
        const gone: NotInFields<"Every.a_f64"> = none.cols.Every.a_f64;
        const empty = ctx.query({ with: ["Every"], fields: [] });
        const gone2: NotInFields<"Every.a_tick"> = empty.cols.Every.a_tick;
        f[0] += draws + picked.length + order.length + x.length + y.length + none.len + empty.len;
        void [gone, gone2];
    },
});
