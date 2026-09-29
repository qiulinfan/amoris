import { Button, Checkbox, Choice, Label, Panel, Slider, TextInput, command, mount, signal, ui } from "pocket";
import { expect, test } from "pocket/test";

const count = signal(0);
const name = signal("");

function Counter() {
    return (
        <Panel title="Counter" name="counter" width={200}>
            <Label text={`Count: ${count()}`} name="count-label" />
            <Button label="Add" name="add" onClick={() => count.update((c) => c + 1)} />
            <TextInput value={name()} placeholder="your name" name="name" onInput={(v) => name.set(v)} />
            {count() >= 2 ? <Label text="Two or more" name="two" /> : null}
        </Panel>
    );
}

test("jsx mounts into the engine tree", () => {
    mount(() => <Counter />);
    const snap = ui.snapshot();
    expect(snap).toContain("box#");
    expect(snap).toContain("counter");
    expect(snap).toContain('"Count: 0"');
    expect(ui.query({ name: "add" }).length).toBe(1);
    expect(ui.query({ text: "Two or more" }).length).toBe(0);
});

test("synthetic clicks reach handlers and re-render only the difference", () => {
    const add = ui.query({ name: "add" })[0];
    const before = ui.stats().nodes;
    const events = ui.click(add.id);
    expect(events.length).toBe(1);
    expect(events[0].type).toBe("click");
    expect(count()).toBe(1);
    expect(ui.snapshot()).toContain('"Count: 1"');
    expect(ui.stats().nodes).toBe(before);  // text changed in place
    ui.click(add.id);
    expect(count()).toBe(2);
    expect(ui.query({ text: "Two or more" }).length).toBe(1);
    expect(ui.stats().nodes).toBe(before + 1);  // the conditional label appeared
    const label = ui.query({ name: "count-label" })[0];
    expect(label.text).toBe("Count: 2");
});

test("typing into an input updates the signal", () => {
    const input = ui.query({ name: "name" })[0];
    ui.click(input.id);
    ui.type("Ada");
    expect(name()).toBe("Ada");
    ui.key("Backspace");
    expect(name()).toBe("Ad");
    expect(ui.query({ name: "name" })[0].value).toBe("Ad");
});

test("hit testing sees layout", () => {
    const add = ui.query({ name: "add" })[0];
    const hitId = ui.hit(add.rect.x + add.rect.w / 2, add.rect.y + add.rect.h / 2);
    // The click lands on the label inside the button; the button is its parent.
    const hit = ui.describe(hitId) as { parent: number; id: number };
    expect(hit.id === add.id || hit.parent === add.id).toBeTruthy();
    expect(ui.hit(1000, 1000)).toBe(0);
});

const volume = signal(0.5);
const volumeLive = signal(0.5);
const subtitles = signal(false);
const difficulty = signal<"easy" | "normal" | "hard">("normal");

function Settings() {
    return (
        <Panel title="Settings" name="settings" width={260}>
            <Slider value={volume()} min={0} max={1} step={0.1} width={214} name="volume" onInput={(v) => volumeLive.set(v)} onChange={(v) => { volume.set(v); volumeLive.set(v); }} />
            <Checkbox checked={subtitles()} label="Subtitles" name="subtitles" onChange={(c) => subtitles.set(c)} />
            <Choice value={difficulty()} options={["easy", "normal", "hard"] as const} labels={{ easy: "Easy", normal: "Normal", hard: "Hard" }} name="difficulty" onChange={(d) => difficulty.set(d)} />
        </Panel>
    );
}

test("a slider follows a press and a drag, snaps to its step and steps with the keys", () => {
    mount(() => <Settings />);
    const slider = ui.query({ name: "volume" })[0];
    const r = slider.rect;
    // The track runs between the thumb's centre at either end: 7 points in from each side.
    const track = r.w - 14;
    ui.click({ x: r.x + 7 + track * 0.8, y: r.y + r.h / 2 });
    expect(volume()).toBe(0.8);
    // A drag from there to a fifth of the way: the live value follows, the change lands on release.
    ui.drag({ x: r.x + 7 + track * 0.8, y: r.y + r.h / 2 }, -track * 0.6, 0, 3);
    expect(volume()).toBe(0.2);
    expect(volumeLive()).toBe(0.2);
    // Past the end is the end.
    ui.drag({ x: r.x + 7 + track * 0.2, y: r.y + r.h / 2 }, -500, 0, 2);
    expect(volume()).toBe(0);
    // With the focus: Right steps by the step, End goes to the top, Space does not jump to the middle.
    ui.focus(slider.id);
    ui.key("Right");
    expect(volume()).toBe(0.1);
    ui.key("End");
    expect(volume()).toBe(1);
    ui.key("Space");
    expect(volume()).toBe(1);
    ui.key("Left");
    expect(volume()).toBe(0.9);
});

test("a checkbox flips on a click and on Space, and a choice steps and wraps", () => {
    const box = ui.query({ name: "subtitles" })[0];
    ui.click(box.id);
    expect(subtitles()).toBe(true);
    ui.focus(box.id);
    ui.key("Space");
    expect(subtitles()).toBe(false);
    const next = ui.query({ name: "difficulty:next" })[0];
    ui.click(next.id);
    expect(difficulty()).toBe("hard");
    ui.click(next.id);
    expect(difficulty()).toBe("easy");   // wraps
    expect(ui.query({ text: "Easy" }).length).toBe(1);
    ui.click(ui.query({ name: "difficulty:prev" })[0].id);
    expect(difficulty()).toBe("hard");
    const choice = ui.query({ name: "difficulty" })[0];
    ui.focus(choice.id);
    ui.key("Right");
    expect(difficulty()).toBe("easy");
    ui.key("Left");
    expect(difficulty()).toBe("hard");
});

test("a pad walks a menu: autofocus takes it, the d-pad moves and sets, A presses", () => {
    const level = signal(0.5);
    const hints = signal(false);
    mount(() => (
        <Panel title="Pad menu" name="pad-menu" width={260}>
            <Slider value={level()} step={0.1} width={214} name="pad-level" autofocus onChange={(v) => level.set(v)} />
            <Checkbox checked={hints()} label="Hints" name="pad-hints" onChange={(c) => hints.set(c)} />
        </Panel>
    ));
    const press = (button: string) => {
        command("input.pad", { pad: 0, button, pressed: true });
        command("input.pad", { pad: 0, button, pressed: false });
    };
    press("dpad_right");   // the slider took the focus when it appeared; Right steps it
    expect(level()).toBe(0.6);
    expect(ui.stats().focused).toBe(ui.query({ name: "pad-level" })[0].id);
    press("dpad_down");
    expect(ui.stats().focused).toBe(ui.query({ name: "pad-hints" })[0].id);
    press("a");
    expect(hints()).toBe(true);
    press("b");            // B leaves the menu
    expect(ui.stats().focused).toBe(0);
});
