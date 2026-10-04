// Mistakes the SDK's types must refuse (script-host.md 12, test 7): every line after a
// `@ts-expect-error` is a type error, and `tsc` fails on one that is not. Never called.
import { system } from "pocket";
import type { SystemContext } from "pocket";

export function mistakes(ctx: SystemContext): void {
    const none = ctx.query({ with: ["Every"], fields: [] });
    // @ts-expect-error `fields: []` hands over no column
    const col: Float64Array = none.cols.Every.a_f64;
    // @ts-expect-error a field of a component the query does not name
    ctx.query({ with: ["Every"], fields: ["Transform.position"] });
    // @ts-expect-error a component scripts cannot name
    ctx.world.get(ctx.single("Every"), "Model");
    // @ts-expect-error event kinds are lowercase
    ctx.emit("Every.Ran");
    // @ts-expect-error no empty word
    ctx.emit("every..ran");
    // @ts-expect-error no word starting with a digit
    ctx.emit("every.2nd");
    // @ts-expect-error at least two words
    ctx.emit("every");
    // @ts-expect-error an engine prefix
    ctx.emit("script.ran");
    // @ts-expect-error a system name starts with a lowercase letter
    system({ name: "1every", phase: "update", doc: "A bad name.", run() {} });
    // @ts-expect-error and holds lowercase letters, digits and _ only
    system({ name: "every/api", phase: "update", doc: "A bad name.", run() {} });
    void col;
}
