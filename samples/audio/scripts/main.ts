// Sound: a looping hum from the scene (AudioSource with autoplay), a beep every second and a
// click when the crate lands, all driven by the simulation tick so a headless run reports the
// same voices, positions and finish events as a window with a sound card.
import { audio, events, expose, log, onStart, onTick, world } from "pocket";

let beeps = 0;
let finished = 0;
let crate = 0;
let y = 3;
let vy = 0;

onStart(() => {
    crate = world.spawn("Crate", { components: { Transform: { position: { x: 1.5, y, z: 0 }, scale: { x: 0.6, y: 0.6, z: 0.6 } }, MeshRenderer: { mesh: "cube", color: { r: 0.9, g: 0.6, b: 0.3, a: 1 } } } });
    log("audio devices", audio.stats());
});

onTick((t) => {
    if (t.tick % 60 === 30) {
        audio.play("assets/beep.wav", { volume: 0.5, pitch: 1 + (beeps % 3) * 0.25, tag: "beep" });
        beeps++;
    }
    vy -= 9.8 * t.dt;
    y += vy * t.dt;
    if (y < 0.3) {
        y = 0.3;
        if (vy < -1) audio.play("assets/click.wav", { volume: 0.8, entity: crate });
        vy = -vy * 0.5;
    }
    world.set(crate, "Transform", { position: { y } });
    for (const e of events.recent(5)) if (e.type === "audio.finished" && e.tick === t.tick) finished++;
});

expose("beeps", () => beeps);
expose("voices", () => audio.voices().length);
expose("hum.position", () => Number((audio.voices().find((v) => v.clip === "assets/hum.wav")?.position ?? -1).toFixed(3)));
