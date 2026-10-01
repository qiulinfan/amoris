// Pocket for pi (and pi-compatible agents such as oh-my-pi): tools that drive a running Pocket
// runtime through its control server, for agents without MCP. Load it with
//   pi -e integrations/pi/pocket.ts
// or copy it into ~/.pi/agent/extensions/. The runtime is $POCKET_RPC_URL, or the url given to
// /pocket-attach; start one with `pocket run <project> -- --serve 4711 --paused` or `pocket editor <project> -- --serve 4711`.
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const LIMIT = 24000;   // characters of a result the model sees; the rest is cut with a note

export default function (pi: ExtensionAPI) {
    let url = (process.env.POCKET_RPC_URL ?? "").replace(/\/$/, "");

    async function call(method: string, params: unknown): Promise<any> {
        if (!url) throw new Error("no Pocket runtime: set POCKET_RPC_URL or run /pocket-attach <url>");
        const res = await fetch(`${url}/rpc`, {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params: params ?? {} }),
        });
        const body: any = await res.json();
        if (body.error) throw new Error(`${method}: ${body.error.message}`);
        return body.result;
    }

    // Several commands in one request (the control server answers a JSON-RPC batch in order); each
    // answer is its result or its error, so one failing call does not hide the others.
    async function batch(calls: Array<{ method: string; params?: unknown }>): Promise<any[]> {
        if (!url) throw new Error("no Pocket runtime: set POCKET_RPC_URL or run /pocket-attach <url>");
        const res = await fetch(`${url}/rpc`, {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify(calls.map((c, i) => ({ jsonrpc: "2.0", id: i + 1, method: c.method, params: c.params ?? {} }))),
        });
        const body: any[] = await res.json();
        return body.map((r, i) => (r.error ? { method: calls[i].method, error: r.error.message } : { method: calls[i].method, result: r.result }));
    }

    function text(result: any): string {
        // Text results (world.tree, transcript, ui.snapshot, commands {text}) as text, the rest as
        // compact JSON.
        const t = result && typeof result.text === "string" && Object.keys(result).length <= 3 ? result.text : JSON.stringify(result);
        return t.length > LIMIT
            ? `${t.slice(0, LIMIT)}\n[cut: ${t.length - LIMIT} more characters; ask for less: commands {family | search, text: true}, help {command | family}, world.schema {component | search}, world.tree {depth}, world.query {limit}, one entity]`
            : t;
    }

    pi.registerTool({
        name: "pocket",
        label: "Pocket",
        description:
            "Send one command to the running Pocket game engine and get its JSON result. Every engine feature is a command: " +
            "world.tree {depth} (the scene as text), world.query {with, name}, world.describe {entity}, world.spawn {name, components}, " +
            "world.set {entity, component, value}, world.destroy {entity}, step {ticks, until?} (until stops early: {event: \"coin.\"} or {state: \"score\", at_least: 3}), state, events.since {seq}, events.why {seq}, " +
            "transcript, render.visible, capture {path}, input.hold {action, ticks}, nav.path, physics.raycast, tilemap.*, audio.*, ui.* ... " +
            "project.brief first: the project's files, scene, components, actions, state and problems as one text. " +
            "`commands {text: true}` lists them all one line each (family or search narrows it); help {command} says how to call one; world.schema {component} gives a component's fields. Entities are ids or names/paths such as Player or /Level/Player. The runtime is paused: step advances it. " +
            "Several commands at once: `calls: [{method, params}, ...]` runs them in order in one request and answers each (its result or its error).",
        parameters: Type.Object({
            method: Type.Optional(Type.String({ description: "command name, e.g. world.tree" })),
            params: Type.Optional(Type.Record(Type.String(), Type.Unknown(), { description: "the command's parameters as an object" })),
            calls: Type.Optional(Type.Array(Type.Object({ method: Type.String(), params: Type.Optional(Type.Record(Type.String(), Type.Unknown())) }), { description: "several commands, run in order: [{method, params}, ...]" })),
        }),
        async execute(_id, p: { method?: string; params?: Record<string, unknown>; calls?: Array<{ method: string; params?: Record<string, unknown> }> }) {
            if (p.calls && p.calls.length > 0) {
                const results = await batch(p.calls);
                return { content: [{ type: "text", text: text(results) }], details: undefined };
            }
            if (!p.method) throw new Error("give a method (and params), or calls: [{method, params}, ...]");
            const result = await call(p.method, p.params ?? {});
            return { content: [{ type: "text", text: text(result) }], details: undefined };
        },
    });

    pi.registerTool({
        name: "pocket_apply",
        label: "Pocket apply",
        description: "After editing the game's scripts or project.toml: bundle and type-check them, reload the game and step it, in one call. Answers the type errors, the state after and any script error; a script that does not bundle leaves the running game as it was.",
        parameters: Type.Object({ ticks: Type.Optional(Type.Number({ description: "ticks to step after the reload (1)" })) }),
        async execute(_id: string, params: { ticks?: number }) {
            const result = await call("project.apply", params?.ticks === undefined ? {} : { ticks: params.ticks });
            return { content: [{ type: "text", text: text(result) }], details: undefined };
        },
    });

    pi.registerTool({
        name: "pocket_look",
        label: "Pocket look",
        description: "See the running game: renders the current frame to a PNG and returns it as an image, with the visible entities and their pixel bounds. With around (true, or an entity's name), a sheet of the scene or that entity from six sides instead (front, right, back, left, top, perspective).",
        parameters: Type.Object({ around: Type.Optional(Type.Union([Type.Boolean(), Type.String()])) }),
        async execute(_id: string, params: { around?: boolean | string }) {
            const dir = mkdtempSync(join(tmpdir(), "pocket-look-"));
            if (params?.around) {
                const path = join(dir, "views.png");
                const sheet = await call("render.views", typeof params.around === "string" ? { path, entity: params.around } : { path });
                return {
                    content: [
                        { type: "text", text: text({ views: sheet.views.map((v: { view: string }) => v.view), columns: sheet.columns, center: sheet.center, entity: sheet.entity }) },
                        { type: "image", data: readFileSync(path).toString("base64"), mimeType: "image/png" },
                    ],
                    details: undefined,
                };
            }
            const path = join(dir, "frame.png");
            const cap = await call("capture", { path });
            const visible = await call("render.visible", { limit: 20 });
            return {
                content: [
                    { type: "text", text: text({ width: cap.width, height: cap.height, visible: visible.visible ?? visible }) },
                    { type: "image", data: readFileSync(path).toString("base64"), mimeType: "image/png" },
                ],
                details: undefined,
            };
        },
    });

    pi.registerCommand("pocket-attach", {
        description: "Point the pocket tools at a running runtime's control server url",
        handler: async (arg, ctx) => {
            url = (arg ?? "").trim().replace(/\/$/, "");
            try {
                const state = await call("state", {});
                ctx.ui.notify(`Pocket: attached to ${url} at tick ${state.tick}`, "info");
            } catch (e) {
                ctx.ui.notify(`Pocket: ${(e as Error).message}`, "error");
            }
        },
    });
}
