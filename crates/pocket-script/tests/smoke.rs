#![cfg(feature = "transpile")]
mod common;

const COMPONENTS: &str = r#"
import { component, field } from "pocket";
export const Crate = component("Crate", {
    version: 1, doc: "A crate adrift.",
    fields: { value: field.u32(1, "Points the crate is worth."), x: field.f64(0, "Position.") },
});
export const Tally = component("Tally", {
    version: 1, doc: "The count of crates taken.",
    fields: { taken: field.u32(0, "Crates taken so far.") },
});
"#;

const MAIN: &str = r#"
import { game, system } from "pocket";
import { Crate, Tally } from "./components";
const setup = system({
    name: "setup", phase: "update", doc: "Spawns the crates and the tally.", when: "start",
    run(ctx) {
        ctx.world.spawn({ Tally: {} });
        for (let i = 0; i < 5; i++) ctx.world.spawn({ Crate: { value: i + 1, x: i * 2 } });
    },
});
const drift = system({
    name: "drift", phase: "update", doc: "Crates drift and are taken past x = 10.",
    queries: { crates: { with: ["Crate"] } },
    run(ctx, { crates }) {
        const x = crates.cols.Crate.x;
        const tallyEntity = ctx.single("Tally");
        let taken = ctx.world.get(tallyEntity, "Tally").taken;
        crates.each((row, e) => {
            x[row] += 0.5 + ctx.rng.next() * Math.sin(ctx.time);
            if (x[row] > 10) {
                ctx.world.despawn(e);
                taken += 1;
                ctx.emit("crate.taken", { taken }, { subject: e });
            }
        });
        ctx.world.set(tallyEntity, "Tally", { taken });
    },
});
export default game({ components: [Crate, Tally], systems: [setup, drift] });
"#;

#[test]
fn a_small_game_runs() {
    let set = common::compiled(&[
        ("scripts/components.ts", COMPONENTS),
        ("scripts/main.ts", MAIN),
    ]);
    let mut sim = common::sim_with(&set, Default::default(), 7);
    let mut hashes = Vec::new();
    for _ in 0..40 {
        let r = common::step(&mut sim);
        assert!(r.errors.is_empty(), "{:#?}", r.errors);
        hashes.push(common::hash(sim.world()));
    }
    let mut again = common::sim_with(&set, Default::default(), 7);
    for (t, h) in hashes.iter().enumerate() {
        common::step(&mut again);
        assert_eq!(*h, common::hash(again.world()), "tick {}", t + 1);
    }
    println!("{:?}", pocket_script::scripts::last_steps(sim.world()));
}
