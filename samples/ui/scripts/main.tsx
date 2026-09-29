// A heads-up display and a pause menu built with Pocket UI. The interface is data: agents read
// it with `ui.snapshot` and press its buttons with `ui.click`, exactly like a player would. Its
// words come from locales/en.json and locales/zh.json by key (`t`); the menu switches language.
import { Button, Checkbox, Choice, Label, Panel, Row, Slider, TextInput, audio, expose, i18n, log, mount, onInput, onTick, signal, t, world } from "pocket";

const score = signal(0);
const health = signal(100);
const paused = signal(false);
const playerName = signal("Player");
// The last key pressed, shown in the hint's place ("" shows the hint).
const message = signal("");
// Settings from the pause menu: the volume is the main bus's, the difficulty sets how much a hurt takes.
const volume = signal(1);
const showBar = signal(true);
const difficulty = signal<"easy" | "normal" | "hard">("normal");
const HURT = { easy: 8, normal: 15, hard: 25 };
let spin = 0;
let crate = 0;

function Hud() {
    return (
        <box position="absolute" left={12} top={12} gap={6} name="hud">
            <Row gap={12}>
                <Label text={`${playerName()}`} size={16} />
                <Label text={t("hud.score", { score: score() })} name="score" size={16} />
                <Label text={t("hud.health", { health: health() })} name="health" size={16} color={health() < 40 ? "#ff6b6b" : "#e6e6e6"} />
                <Label text={t("hud.coins", { count: Math.floor(score() / 10) })} name="coins" size={16} />
            </Row>
            {showBar() ? (
                <box width={200} height={10} background="#00000080" radius={5} name="health-bar">
                    <box width={`${health()}%`} height="100%" background={health() < 40 ? "#e5484d" : "#3dbf6d"} radius={5} />
                </box>
            ) : null}
            <Label text={message() === "" ? t("hud.hint") : t("hud.key", { key: message() })} muted name="hint" />
            <Label text="中文 · 日本語 · 한국어 · Ünïcödé: one font, shaped by HarfBuzz" muted size={13} name="scripts" />
        </box>
    );
}

function Controls() {
    return (
        <Row gap={6} name="controls">
            <Button label={t("controls.spin_left")} onClick={() => { spin -= 1; }} />
            <Button label={t("controls.spin_right")} onClick={() => { spin += 1; }} />
            <Button label={t("controls.score_up")} name="score-button" onClick={() => score.update((s) => s + 10)} />
            <Button label={t("controls.hurt")} danger onClick={() => health.update((h) => Math.max(0, h - HURT[difficulty()]))} />
            <Button label={t("controls.heal")} onClick={() => health.set(100)} />
            <Button label={paused() ? t("controls.resume") : t("controls.pause")} primary name="pause" onClick={() => paused.update((p) => !p)} />
        </Row>
    );
}

function PauseMenu() {
    if (!paused()) return null;
    return (
        <box position="absolute" left={0} top={0} width="100%" height="100%" background="#00000099" justify="center" align="center" name="overlay">
            <Panel title={t("menu.title")} width={320} gap={10} padding={14} name="pause-menu">
                <Label text={t("menu.name")} muted />
                <TextInput value={playerName()} onInput={(v) => playerName.set(v)} placeholder={t("menu.name_placeholder")} name="name-input" />
                <Row gap={10}>
                    <Label text={t("menu.volume")} muted />
                    <Slider value={volume()} step={0.05} width={180} name="volume" autofocus onInput={(v) => { volume.set(v); audio.bus("main", { volume: v }); }} />
                    <Label text={`${Math.round(volume() * 100)}%`} muted size={12} />
                </Row>
                <Row gap={10}>
                    <Label text={t("menu.difficulty")} muted />
                    <Choice value={difficulty()} options={["easy", "normal", "hard"] as const} labels={{ easy: t("menu.easy"), normal: t("menu.normal"), hard: t("menu.hard") }} width={180} name="difficulty" onChange={(d) => difficulty.set(d)} />
                </Row>
                <Checkbox checked={showBar()} label={t("menu.show_bar")} name="show-bar" onChange={(c) => showBar.set(c)} />
                <Row gap={10}>
                    <Label text={t("menu.language")} muted />
                    <Choice value={i18n.language()} options={i18n.languages().languages} labels={Object.fromEntries(i18n.languages().languages.map((l) => [l, t(`language.${l}`)]))} width={180} name="language" onChange={(l) => i18n.use(l)} />
                </Row>
                <Row justify="end">
                    <Button label={t("menu.resume")} primary onClick={() => paused.set(false)} />
                </Row>
            </Panel>
        </box>
    );
}

mount(() => (
    <>
        <Hud />
        <box position="absolute" left={12} bottom={12}>
            <Controls />
        </box>
        <PauseMenu />
    </>
));

expose("score", () => score());
expose("health", () => health());
expose("paused", () => paused());
expose("volume", () => volume());
expose("difficulty", () => difficulty());
expose("show_bar", () => showBar());
expose("language", () => i18n.language());

let yaw = 0;
onTick((t) => {
    if (crate === 0) crate = world.find("Crate") ?? 0;
    if (paused() || crate === 0) return;
    yaw += spin * t.dt * 2;
    const half = yaw / 2;
    world.set(crate, "Transform", { rotation: { x: 0, y: Math.sin(half), z: 0, w: Math.cos(half) } });
});

onInput((events) => {
    for (const e of events) {
        if (e.ui !== undefined) continue;  // the interface took it
        if (e.type === "key_down" && e.key === "Escape") paused.update((p) => !p);
        // A pad: Start opens and closes the menu, B closes it (the menu takes the d-pad and A while it has the focus).
        if (e.type === "pad_button" && e.pressed && (e.button === "start" || (e.button === "b" && paused()))) paused.update((p) => !p);
        if (e.type === "key_down" && !e.repeat && e.key !== undefined) message.set(e.key);
    }
});

log("ui sample started");
