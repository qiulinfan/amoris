import { Button, Label, Panel, TextInput, mount, signal, ui } from "pocket";
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
