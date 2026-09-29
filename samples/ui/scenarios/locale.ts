// The UI sample's words by language (docs/design/localization.md): the same interface in English
// and in Chinese, switched from the pause menu's language choice or by a script, every text from
// the tables, plurals by count.
//   pocket scenario ui
import { command, expect, i18n, scenario, t } from "pocket";

function text(name: string): string {
    const nodes = command("ui.query", { name }) as Array<{ text?: string }>;
    return nodes[0]?.text ?? "";
}

function click(name: string): void {
    const nodes = command("ui.query", { name }) as Array<{ id: number }>;
    command("ui.click", { id: nodes[0].id });
}

scenario("the interface speaks English, then Chinese, then English again", (g) => {
    g.wait(0.1);
    g.check(() => {
        expect(i18n.language()).toBe("en");
        expect(text("score")).toBe("Score 0");
        expect(text("coins")).toBe("No coins yet");
        expect(t("hud.coins", { count: 1 })).toBe("1 coin");
        expect(t("hud.coins", { count: 3 })).toBe("3 coins");
        expect(t("no.such.key")).toBe("no.such.key");              // a missing key shows itself
        const check = i18n.check() as { complete: boolean };
        expect(check.complete).toBe(true);                       // every English key has its Chinese
    }, "English");
    g.check(() => { click("score-button"); click("score-button"); }, "twenty points");
    g.check(() => i18n.use("zh"), "switch to Chinese");
    g.wait(0.1);
    g.check(() => {
        expect(i18n.language()).toBe("zh");
        expect(text("score")).toBe("得分 20");
        expect(text("coins")).toBe("2 枚金币");
        expect(text("hint")).toBe("用按钮转动箱子，或用 WASD 行走。");
    }, "Chinese");
    g.check(() => i18n.use("en"), "back to English");
    g.wait(0.1);
    g.check(() => expect(text("coins")).toBe("2 coins"), "English again");
});
