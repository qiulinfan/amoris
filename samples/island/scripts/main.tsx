// An island sea (docs/design/water.md, Oceans; docs/design/physics.md, Boats): a sailing boat on an
// ocean that runs to the horizon, round a wooded island and two islets, under volumetric clouds and
// a running day, with sixteen crates adrift to gather. W and S (or the left stick) drive the boat
// ahead and astern, A and D put the rudder over, E (or Space, or the pad's A) sets or furls the
// sail: it takes the steady wind toward +x, best across it, nothing heading into it. Bring the boat
// alongside a crate to take it aboard. The boat is the engine's Boat component, so an agent sails
// it with world.set as well as with the actions.
//   pocket run island
//   pocket scenario island
import { Label, audio, events, expose, input, mount, onStart, onTick, signal, world } from "pocket";

const REACH = 3;          // how near a crate must float to be taken aboard

let boat = 0;
let total = 0;
let lastThrottle = 0, lastSteer = 0;
let shown = "boat?length=3.4&furled=1";
const WIND_X = 1, WIND_Z = 0;   // where the Breeze blows to (toward +x)
const taken = signal(0);
const speed = signal(0);
const sailing = signal(false);
const clock = signal("09:00");
const done = signal(false);

const crates = () => world.query({ with: ["Transform"], under: "Crates", fields: ["Transform"] });

onStart(() => {
    boat = world.find("Boat") ?? 0;
    total = crates().length;
    mount(() => (
        <box position="absolute" left={0} top={0} width="100%" height="100%">
            <box position="absolute" left={14} top={12} padding={[6, 10]} radius={6} background="#00000070">
                <Label text={`${clock()}   Crates ${taken()}/${total}`} color="#ffe9a8" size={16} name="tally" />
            </box>
            <box position="absolute" right={14} top={12} padding={[6, 10]} radius={6} background="#00000070">
                <Label text={`${speed().toFixed(1)} u/s   Sail ${sailing() ? "set" : "furled"}   Wind east`} color="#ffffff" size={15} name="log" />
            </box>
            <box position="absolute" left="50%" bottom={34} margin={[0, 0, 0, -170]} width={340} padding={[6, 10]} radius={6} background="#00000080">
                <Label text={done() ? "Every crate aboard!" : "W/S drive   A/D rudder   E sail"} color="#ffffff" size={15} name="help" />
            </box>
        </box>
    ));
});

onTick(() => {
    if (!boat) return;
    // The day's hour from the Sky.
    const minutes = Math.floor(world.get("Sky", "Sky")!.time_of_day * 60 + 1e-3);
    clock.set(`${String(Math.floor(minutes / 60) % 24).padStart(2, "0")}:${String(minutes % 60).padStart(2, "0")}`);
    // The controls: written when the player's input changes, so what an agent or a scenario sets
    // on the Boat stays until the player takes the helm again.
    const throttle = -input.axis("move_z"), steer = input.axis("move_x");
    if (throttle !== lastThrottle || steer !== lastSteer) world.set(boat, "Boat", { throttle, steer });
    lastThrottle = throttle;
    lastSteer = steer;
    if (input.pressed("use")) {
        const set = world.get(boat, "Boat")!.sail < 0.5;
        world.set(boat, "Boat", { sail: set ? 1 : 0 });
        events.emit(set ? "sail.set" : "sail.furled", {}, { subject: boat });
        audio.play("sfx:click");
    }
    const b = world.get(boat, "Boat")!;
    // The sail shows set or rolled on its boom, as the Boat says, the boom let out to leeward: close
    // in heading near the wind, far out running before it (in steps of ten degrees, so the few
    // meshes it takes are made once).
    const q = world.get(boat, "Transform")!.rotation;
    const ax = -2 * (q.x * q.z + q.w * q.y), az = -(1 - 2 * (q.x * q.x + q.y * q.y));   // the bow's way on the ground
    const with_ = ax * WIND_X + az * WIND_Z;            // 1 the wind astern, -1 ahead
    const lee = Math.sign(-az * WIND_X + ax * WIND_Z) || 1;   // the wind blows to starboard (+1) or to port
    const boom = Math.round((lee * (15 + 65 * (with_ + 1) / 2)) / 10) * 10;
    const mesh = b.sail >= 0.5 ? `boat?length=3.4&boom=${boom}` : "boat?length=3.4&furled=1";
    if (mesh !== shown) world.set(boat, "MeshRenderer", { mesh });
    shown = mesh;
    sailing.set(b.sail >= 0.5);
    speed.set(Math.abs(b.speed));
    // Crates alongside come aboard.
    const p = world.get(boat, "Transform")!.position;
    for (const c of crates()) {
        const q = c.Transform!.position;
        if (Math.hypot(q.x - p.x, q.z - p.z) > REACH || Math.abs(q.y - p.y) > 3) continue;
        world.destroy(c.id);
        taken.set(taken() + 1);
        events.emit("crate.taken", { taken: taken(), left: total - taken() }, { subject: boat });
        audio.play("sfx:coin");
        if (taken() === total) {
            done.set(true);
            events.emit("crates.all", { taken: taken() });
            audio.play("sfx:powerup");
        }
    }
});

expose("taken", () => taken());
expose("left", () => total - taken());
expose("speed", () => Number(world.get(boat, "Boat")!.speed.toFixed(2)));
expose("afloat", () => world.get(boat, "Boat")!.afloat);
expose("sailing", () => sailing());
expose("boat.x", () => Number(world.get(boat, "Transform")!.position.x.toFixed(2)));
expose("boat.z", () => Number(world.get(boat, "Transform")!.position.z.toFixed(2)));
