// Answers the menu's presses (docs/spec/ui.md 7): a press arrives as `ui.pressed` with the button
// as its subject in the tick after it; the Sound toggle relabels itself and the status line says
// what was pressed and how often.
import { component, field, game, system } from "amoris";

export const MenuButton = component("MenuButton", {
    version: 1, doc: "What a menu button does.",
    fields: { action: field.enum(["start", "sound", "settings", "quit"], "start", "Its action.") },
});

export const Status = component("Status", { version: 1, doc: "The status line.", fields: {} });

export const menu = system({
    name: "menu", phase: "update", doc: "Relabels the toggle and reports each press.",
    events: ["ui."],
    run(ctx) {
        for (const e of ctx.events) {
            if (e.kind !== "ui.pressed" || e.subject === null) continue;
            const data = e.data as { on: boolean; presses: number };
            const button = ctx.world.get(e.subject, "MenuButton");
            if (button?.action === "sound") {
                ctx.world.set(e.subject, "UiText", { text: `Sound: ${data.on ? "on" : "off"}` });
            }
            const label = ctx.world.get(e.subject, "UiText")?.text ?? "?";
            const status = ctx.single("Status");
            ctx.world.set(status, "UiText", { text: `按下 ${button?.action ?? label} ×${data.presses}` });
        }
    },
});

export default game({ components: [MenuButton, Status], systems: [menu] });
