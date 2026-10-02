// A farm (docs/design/rendering.md, A day and Weather; docs/design/assets.md, Props): a fenced field
// of twelve plots under a running day and the weather. Stand by a plot and press E (or Space, or
// the pad's A) to sow it, again to water it from the can; a crop grows a stage every eight seconds
// it is watered, its soil drying over forty-five, and rain waters every plot at once. Ripe, it is
// harvested; the bin by the gate buys the harvest (5 coins each) and sells seeds (3 for 5 coins),
// the well fills the can. What each plot holds is the project's own Plot component
// (components.toml), so an agent reads and sets the field with world.query and world.set.
// WASD or the left stick walks, Q and R or the right stick turn the camera.
//   pocket run farm
//   pocket scenario farm
import { Label, audio, events, expose, input, mount, onStart, onTick, particles, repro, signal, world } from "pocket";
import type { Entity } from "pocket";

const SPEED = 4.5;
const GROW = 8;          // seconds watered a stage
const DRY = 45;          // seconds for wet soil to dry
const CAN = 5;           // waterings a full can holds
const PRICE = 5;         // coins a crop
const SEEDS_COST = 5;    // coins for three seeds
const DRY_SPELL = 75, RAIN_SPELL = 30;   // the weather's turn: seconds dry, then seconds of rain

interface Spot { kind: "plot" | "well" | "bin"; id: Entity; d: number }
interface Field { id: Entity; name: string; index: number; soil: Entity; plants: Entity[]; x: number; z: number; shown: { stage: number; water: number } }

let player = 0;
let body = 0;
let seen = 0;
let spell = 0;           // seconds into the weather's turn
let raining = false;
let lastHour = -1;
let focus: Spot | null = null;
const coins = signal(10);
const seeds = signal(6);
const harvest = signal(0);
const can = signal(3);
const day = signal(1);
const clock = signal("07:00");
const prompt = signal("");
const sky = signal("");

let field: Field[] = [];   // the plots, in the order of their names
const plots = () => world.query({ with: ["Plot", "Transform"] });
const plotOf = (id: Entity) => field.find((f) => f.id === id)!;

// A plot's plants and soil as its Plot says (whoever set it: the game, a scenario, an agent): hidden
// while bare, at their stage, the soil darker the wetter, redrawn when the stage or a twentieth of
// the water changes.
function show(f: Field) {
    const plot = world.get(f.id, "Plot")!;
    if (plot.stage !== f.shown.stage) {
        f.plants.forEach((plant, k) => world.set(plant, "MeshRenderer", { visible: plot.stage >= 0, mesh: `crop?stage=${Math.max(plot.stage, 0)}&seed=${(f.index * 9 + k) % 6}` }));
    }
    if (Math.abs(plot.water - f.shown.water) > 0.05 || (plot.water === 0) !== (f.shown.water === 0)) {
        const w = plot.water;
        world.set(f.soil, "MeshRenderer", { color: { r: 0.62 - 0.3 * w, g: 0.46 - 0.24 * w, b: 0.33 - 0.17 * w, a: 1 } });
    }
    f.shown = { stage: plot.stage, water: plot.water };
}

// What the player stands by: the nearest plot within reach, else the well or the bin.
function spotNear(): Spot | null {
    const p = world.get(player, "Transform")!.position;
    let best: Spot | null = null;
    for (const f of field) {
        const d = Math.hypot(f.x - p.x, f.z - p.z);
        if (d < 1.7 && (!best || d < best.d)) best = { kind: "plot", id: f.id, d };
    }
    if (best) return best;
    for (const [kind, name, reach] of [["well", "Farm/Well", 2.2], ["bin", "Farm/Bin", 2.0]] as const) {
        const id = world.find(name);
        if (!id) continue;
        const q = world.get(id, "Transform")!.position;
        const d = Math.hypot(q.x - p.x, q.z - p.z);
        if (d < reach) return { kind, id, d };
    }
    return null;
}

function promptFor(s: Spot | null): string {
    if (!s) return "";
    if (s.kind === "well") return can() < CAN ? "E: fill the can" : "The can is full";
    if (s.kind === "bin") {
        if (harvest() > 0) return `E: sell ${harvest()} for ${harvest() * PRICE}`;
        return coins() >= SEEDS_COST ? `E: buy 3 seeds for ${SEEDS_COST}` : "Nothing to sell";
    }
    const plot = world.get(s.id, "Plot")!;
    if (plot.stage < 0) return seeds() > 0 ? "E: sow" : "No seeds: buy some at the bin";
    if (plot.stage >= 3) return "E: harvest";
    if (plot.water > 0.5) return "Growing";
    return can() > 0 ? "E: water" : "The can is empty: fill it at the well";
}

function act(s: Spot) {
    const at = world.get(s.id, "Transform")!.position;
    if (s.kind === "well") {
        if (can() >= CAN) return;
        can.set(CAN);
        events.emit("can.filled", { can: can() }, { subject: player });
        audio.play("sfx:click");
        return;
    }
    if (s.kind === "bin") {
        if (harvest() > 0) {
            const n = harvest();
            coins.set(coins() + n * PRICE);
            harvest.set(0);
            events.emit("harvest.sold", { crops: n, coins: coins() }, { subject: player });
            audio.play("sfx:coin");
        } else if (coins() >= SEEDS_COST) {
            coins.set(coins() - SEEDS_COST);
            seeds.set(seeds() + 3);
            events.emit("seeds.bought", { seeds: seeds(), coins: coins() }, { subject: player });
            audio.play("sfx:select");
        }
        return;
    }
    const f = plotOf(s.id);
    const plot = world.get(s.id, "Plot")!;
    if (plot.stage < 0) {
        if (seeds() <= 0) return;
        seeds.set(seeds() - 1);
        world.set(s.id, "Plot", { stage: 0, growth: 0 });
        events.emit("crop.sown", { plot: f.name, seeds: seeds() }, { subject: s.id });
        audio.play("sfx:step");
    } else if (plot.stage >= 3) {
        world.set(s.id, "Plot", { stage: -1, growth: 0 });
        harvest.set(harvest() + 1);
        particles.burst("Bits", 24, { at: { x: at.x, y: 0.4, z: at.z } });
        events.emit("crop.harvested", { plot: f.name, harvest: harvest() }, { subject: s.id });
        audio.play("sfx:powerup?volume=0.4");
    } else {
        if (can() <= 0) return;
        can.set(can() - 1);
        world.set(s.id, "Plot", { water: 1 });
        events.emit("crop.watered", { plot: f.name, can: can() }, { subject: s.id });
        audio.play("sfx:step?volume=0.6&seed=3");
    }
    show(f);
}

onStart(() => {
    player = world.find("Player") ?? 0;
    body = world.find("Player/Body") ?? 0;
    world.spawn("Bits", { components: { Transform: {} } });
    particles.preset("Bits", "magic", { emitting: false });
    field = plots().map((row) => {
        const name = row.path.split("/").pop()!;
        const kids = world.describe(row.id).children;
        return {
            id: row.id, name, index: Number(name.slice(4)),
            soil: kids.find((k) => k.name === "Soil")!.id,
            plants: kids.filter((k) => k.name.startsWith("Plant")).map((k) => k.id),
            x: row.Transform!.position.x, z: row.Transform!.position.z,
            shown: { stage: -2, water: -1 },
        };
    }).sort((a, b) => a.index - b.index);
    for (const f of field) show(f);
    seen = events.lastSeq();
    mount(() => (
        <box position="absolute" left={0} top={0} width="100%" height="100%">
            <box position="absolute" left={14} top={12} padding={[6, 10]} radius={6} background="#00000070">
                <Label text={`Day ${day()}  ${clock()}${sky()}`} color="#ffe9a8" size={16} name="clock" />
            </box>
            <box position="absolute" right={14} top={12} padding={[6, 10]} radius={6} background="#00000070">
                <Label text={`Coins ${coins()}   Seeds ${seeds()}   Harvest ${harvest()}   Can ${can()}/${CAN}`} color="#ffffff" size={15} name="stock" />
            </box>
            {prompt() ? (
                <box position="absolute" left="50%" bottom={40} margin={[0, 0, 0, -130]} width={260} padding={[6, 10]} radius={6} background="#00000090">
                    <Label text={prompt()} color="#ffffff" size={15} name="prompt" />
                </box>
            ) : null}
        </box>
    ));
});

onTick(({ dt }) => {
    if (!player) return;
    for (const e of events.since(seen)) seen = e.seq;
    // The day: the hour from the Sky, a new day when it comes round past midnight.
    const t = world.get("Sky", "Sky")!.time_of_day;
    if (lastHour >= 0 && t < lastHour) day.set(day() + 1);
    lastHour = t;
    const minutes = Math.floor(t * 60 + 1e-3);
    clock.set(`${String(Math.floor(minutes / 60) % 24).padStart(2, "0")}:${String(minutes % 60).padStart(2, "0")}`);
    // The weather's turn: dry, then rain easing in and out; the soil of every plot drinks it.
    spell += dt;
    if (spell >= DRY_SPELL + RAIN_SPELL) spell -= DRY_SPELL + RAIN_SPELL;
    const into = spell - DRY_SPELL;
    const rain = into <= 0 ? 0 : Math.min(1, into / 4, (RAIN_SPELL - into) / 4);
    const weather = world.get("Weather", "Weather")!;
    if (Math.abs(weather.rain - rain) > 1e-4) world.set("Weather", "Weather", { rain: Math.max(rain, 0) });
    if ((rain > 0.05) !== raining) {
        raining = rain > 0.05;
        events.emit(raining ? "rain.started" : "rain.stopped", {});
    }
    sky.set(raining ? "  Rain" : "");
    const wet = world.get("Weather", "Weather")!.wet;
    // The field: wet soil feeds a crop toward its next stage and dries; rain waters what is sown.
    for (const row of plots()) {
        const f = plotOf(row.id);
        const plot = row.Plot!;
        let { stage, water, growth } = plot;
        if (stage >= 0 && wet > 0.3) water = 1;
        if (stage >= 0 && stage < 3 && water > 0) {
            growth += dt;
            if (growth >= GROW) {
                stage += 1;
                growth = 0;
                events.emit("crop.grew", { plot: f.name, stage }, { subject: row.id });
            }
        }
        water = Math.max(0, water - dt / DRY);
        if (stage !== plot.stage || Math.abs(water - plot.water) > 1e-6 || growth !== plot.growth) world.set(row.id, "Plot", { stage, water, growth });
        show(f);
    }
    // What the player stands by: outlined, with what E would do.
    const now = spotNear();
    if (focus?.id !== now?.id) {
        const outline = (s: Spot | null, on: boolean) => {
            if (!s) return;
            const target = s.kind === "plot" ? plotOf(s.id).soil : s.id;
            world.set(target, "MeshRenderer", { highlight: on ? { r: 1, g: 0.9, b: 0.4, a: 0.9 } : { r: 0, g: 0, b: 0, a: 0 } });
        };
        outline(focus, false);
        outline(now, true);
    }
    focus = now;
    prompt.set(promptFor(focus));
    if (input.pressed("use") && focus) act(focus);
    // Walk relative to where the camera looks.
    const c = world.get(player, "Character")!;
    const cam = world.get("Camera", "CameraRig")!;
    const yaw = (cam.yaw * Math.PI) / 180;
    const mx = input.axis("move_x"), mz = input.axis("move_z");
    const vx = (repro.cos(yaw) * mx + repro.sin(yaw) * mz) * SPEED;
    const vz = (-repro.sin(yaw) * mx + repro.cos(yaw) * mz) * SPEED;
    world.set(player, "Character", { velocity: { x: vx, y: c.velocity.y, z: vz } });
    if (body && Math.hypot(vx, vz) > 0.1) {
        const turn = repro.atan2(-vx, -vz);
        world.set(body, "Transform", { rotation: { x: 0, y: repro.sin(turn / 2), z: 0, w: repro.cos(turn / 2) } });
    }
});

expose("coins", () => coins());
expose("seeds", () => seeds());
expose("harvest", () => harvest());
expose("can", () => can());
expose("day", () => day());
expose("raining", () => raining);
expose("focus", () => (focus ? (focus.kind === "plot" ? plotOf(focus.id).name : focus.kind) : ""));
expose("prompt", () => prompt());
expose("sown", () => world.query({ with: ["Plot"], where: "Plot.stage >= 0", fields: [] }).length);
expose("ripe", () => world.query({ with: ["Plot"], where: "Plot.stage == 3", fields: [] }).length);
expose("dry", () => world.query({ with: ["Plot"], where: "Plot.stage >= 0 and Plot.water == 0", fields: [] }).length);
expose("player.x", () => Number(world.get(player, "Transform")!.position.x.toFixed(2)));
expose("player.z", () => Number(world.get(player, "Transform")!.position.z.toFixed(2)));
