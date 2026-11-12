// Intents and executors (script-host.md 5.5): an intent with every kind of parameter, target and
// failure, and an executor using every member of its context, type-checked by tsc. Not part of
// the game (a game without [player] may not declare intents); the lines after a
// `@ts-expect-error` are mistakes the types refuse.
import { type Entity, type ExecutorContext, type SystemContext, executor, intent, param } from "amoris";

export const carry = intent("carry", {
    doc: "Carry the target; with keep, stay beside it.",
    params: {
        speed_mps: param.number("How fast.", { default: 2, min: 0.5, max: 8 }),
        tries: param.integer("How often to try.", { default: 3, min: 1 }),
        run: param.bool("Run.", { default: false }),
        side: param.choice(["left", "right"], "Which side.", { default: "left" }),
        note: param.text("A note.", { default: "" }),
        must: param.number("A required parameter."),
    },
    target: { entityOrPoint: ["crate"] },
    channels: ["body", "hands"],
    canHold: true,
    timeoutS: 60,
    failures: [
        { code: "carry.blocked", when: "Something is in the way.", detail: ["by"], template: "Blocked by {by}." },
        "perception.target_lost",
    ],
    progress: [
        { name: "distance_m", doc: "Metres left.", unit: { unit: "metres" }, precision: 1 },
        { name: "phase", doc: "What it does now.", unit: { unit: "enum", values: ["going", "carrying"] }, precision: 0 },
        { name: "share", doc: "Done so far.", unit: { unit: "fraction", min: 0, max: 1 }, precision: 2 },
    ],
});

export const point = intent("point", { doc: "Point somewhere.", target: "point", channels: ["hands"], timeoutS: 5 });

export const carrier = executor({
    name: "carrier",
    phase: "update",
    doc: "Carries out carry and point.",
    intents: [carry, point],
    run(ctx: ExecutorContext) {
        const r: number = ctx.rng.next() + ctx.tick + ctx.dt + ctx.time;
        const name: string = ctx.system;
        for (const it of ctx.intents) {
            const id: number = it.id;
            const status: "active" | "holding" = it.status;
            const tag: string | undefined = it.tag;
            const started: number = it.started_tick;
            const speed = it.params.speed_mps;
            const readings = it.progress.map((p) => p.name);
            const view = ctx.view(it);
            const pos = view.instrument("pos_m");
            const target = it.target;
            if (target !== undefined && "id" in target) {
                const p = view.percept(target.id);
                const kind: string | undefined = p?.kind;
                const range: number | undefined = p?.range_m;
                const seen: boolean = p?.visibility === "seen";
                void [kind, range, seen, p?.facts.alongside, p?.pos_m.x, p?.bearing_deg, p?.name];
                ctx.controls(it).pulse("primary", target.id);
            }
            for (const n of view.nearby({ kinds: ["crate"], within_m: 10, visibility: ["seen"], limit: 4 })) {
                void n.id;
            }
            for (const e of view.eventsSince(0)) {
                void [e.seq, e.world_seq, e.tick, e.kind, e.subject, e.data.left];
            }
            ctx.controls(it).set("forward", 1);
            ctx.controls(it).set("run", true);
            ctx.controls(it).set("slot", "2");
            ctx.progress(it, { distance_m: 3, phase: "going" });
            ctx.setState(it, { seen: [1, 2], at: null });
            if (it.state === null) ctx.reached(it, { share: 1 });
            else if (speed === 0) ctx.succeed(it);
            else ctx.reject(it, "carry.blocked", { by: "a wall" });
            void [id, status, tag, started, readings, pos, r, name];
        }
    },
});

export function mistakes(x: ExecutorContext, s: SystemContext, e: Entity): void {
    // @ts-expect-error an executor's context has no world
    x.world.get(e, "Every");
    // @ts-expect-error nor events to emit
    x.emit("every.ran");
    // @ts-expect-error an intent's target is "none", "point", {entity} or {entityOrPoint}
    intent("bad", { doc: "", target: { kinds: ["crate"] }, channels: ["body"], timeoutS: 1 });
    // @ts-expect-error every intent has a default timeout
    intent("bad", { doc: "", channels: ["body"] });
    // @ts-expect-error a choice's default is one of its values
    param.choice(["a", "b"], "", { default: "c" });
    // @ts-expect-error an executor lists intents, not their names
    executor({ name: "bad", phase: "update", doc: "", intents: ["carry"], run() {} });
    // ctx.intent takes an actor, the intent, its parameters and where it aims.
    s.intent(e, "carry", { must: 1 }, { target: e, tag: "mine" });
    s.intent(e, "point", {}, { target: { x: 1, z: 2 } });
    // @ts-expect-error a target is an entity or a point
    s.intent(e, "point", {}, { target: "here" });
}
