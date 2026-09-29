// A heads-up display and a pause menu built with Pocket UI. The interface is data: agents read
// it with `ui.snapshot` and press its buttons with `ui.click`, exactly like a player would.
import { Button, Checkbox, Choice, Label, Panel, Row, Slider, TextInput, audio, expose, log, mount, onInput, onTick, signal, world } from "pocket";

const score = signal(0);
const health = signal(100);
const paused = signal(false);
const playerName = signal("Player");
const message = signal("Spin the crate with the buttons or walk with WASD.");
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
                <Label text={`Score ${score()}`} name="score" size={16} />
                <Label text={`Health ${health()}`} name="health" size={16} color={health() < 40 ? "#ff6b6b" : "#e6e6e6"} />
            </Row>
            {showBar() ? (
                <box width={200} height={10} background="#00000080" radius={5} name="health-bar">
                    <box width={`${health()}%`} height="100%" background={health() < 40 ? "#e5484d" : "#3dbf6d"} radius={5} />
                </box>
            ) : null}
            <Label text={message()} muted />
            <Label text="中文 · 日本語 · 한국어 · Ünïcödé: one font, shaped by HarfBuzz" muted size={13} name="scripts" />
        </box>
    );
}

function Controls() {
    return (
        <Row gap={6} name="controls">
            <Button label="Spin left" onClick={() => { spin -= 1; }} />
            <Button label="Spin right" onClick={() => { spin += 1; }} />
            <Button label="Score +10" name="score-button" onClick={() => score.update((s) => s + 10)} />
            <Button label="Hurt" danger onClick={() => health.update((h) => Math.max(0, h - HURT[difficulty()]))} />
            <Button label="Heal" onClick={() => health.set(100)} />
            <Button label={paused() ? "Resume" : "Pause"} primary name="pause" onClick={() => paused.update((p) => !p)} />
        </Row>
    );
}

function PauseMenu() {
    if (!paused()) return null;
    return (
        <box position="absolute" left={0} top={0} width="100%" height="100%" background="#00000099" justify="center" align="center" name="overlay">
            <Panel title="Paused" width={320} gap={10} padding={14} name="pause-menu">
                <Label text="Your name" muted />
                <TextInput value={playerName()} onInput={(v) => playerName.set(v)} placeholder="name" name="name-input" />
                <Row gap={10}>
                    <Label text="Volume" muted />
                    <Slider value={volume()} step={0.05} width={180} name="volume" autofocus onInput={(v) => { volume.set(v); audio.bus("main", { volume: v }); }} />
                    <Label text={`${Math.round(volume() * 100)}%`} muted size={12} />
                </Row>
                <Row gap={10}>
                    <Label text="Difficulty" muted />
                    <Choice value={difficulty()} options={["easy", "normal", "hard"] as const} labels={{ easy: "Easy", normal: "Normal", hard: "Hard" }} width={180} name="difficulty" onChange={(d) => difficulty.set(d)} />
                </Row>
                <Checkbox checked={showBar()} label="Show the health bar" name="show-bar" onChange={(c) => showBar.set(c)} />
                <Row justify="end">
                    <Button label="Resume" primary onClick={() => paused.set(false)} />
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
        if (e.type === "key_down" && !e.repeat) message.set(`Key ${e.key}`);
    }
});

log("ui sample started");
