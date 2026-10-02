// Scenarios for the farm sample (docs/design/scenarios.md): sowing, watering and the stage a crop
// grows, a harvest sold at the bin, seeds bought there, the well filling the can, and rain watering
// every sown plot at once.
//   pocket scenario farm
import { expect, scenario, world } from "pocket";

const put = (x: number, z: number) => world.set("Player", "Transform", { position: { x, y: 0.9, z } });
const plot = (n: number) => world.get(`Farm/Plot${n}`, "Plot")!;
// The plots stand 2.4 apart in rows of four; the player steps onto the path just in front of one.
const before = (n: number) => put((n % 4 - 1.5) * 2.4, -3 + (Math.floor(n / 4) - 1) * 2.4 + 0.9);

scenario("a seed sown and watered grows a stage in eight seconds", (g) => {
    g.check(() => before(9), "the player by plot 9");
    g.wait(0.1);
    g.check(() => expect(g.state("prompt")).toBe("E: sow"), "it says E sows");
    g.press("use");
    g.wait(0.1);
    g.check(() => {
        expect(plot(9).stage).toBe(0);
        expect(g.state("seeds")).toBe(5);
        expect(g.state("prompt")).toBe("E: water");
    }, "sown, a seed fewer");
    g.press("use");
    g.wait(0.1);
    g.check(() => {
        expect(plot(9).water).toBeGreaterThan(0.95);
        expect(g.state("can")).toBe(2);
        expect(g.count("crop.watered")).toBe(1);
    }, "watered from the can");
    g.until(() => plot(9).stage === 1, { timeout: 8.5, label: "a stage on" });
    g.check(() => expect(g.count("crop.grew")).toBe(1), "it grew once");
});

scenario("a ripe crop is harvested and sold at the bin", (g) => {
    g.check(() => {
        world.set("Farm/Plot2", "Plot", { stage: 3 });
        before(2);
    }, "plot 2 ripe, the player by it");
    g.wait(0.1);
    g.check(() => expect(g.state("prompt")).toBe("E: harvest"), "it says E harvests");
    g.press("use");
    g.wait(0.1);
    g.check(() => {
        expect(plot(2).stage).toBe(-1);
        expect(g.state("harvest")).toBe(1);
    }, "harvested, the plot bare again");
    g.check(() => put(-8, 5.6), "to the bin");
    g.wait(0.1);
    g.check(() => expect(g.state("prompt")).toBe("E: sell 1 for 5"), "it offers 5");
    g.press("use");
    g.wait(0.1);
    g.check(() => {
        expect(g.state("coins")).toBe(15);
        expect(g.state("harvest")).toBe(0);
        expect(g.count("harvest.sold")).toBe(1);
    }, "sold for 5 coins");
    g.press("use");
    g.wait(0.1);
    g.check(() => {
        expect(g.state("seeds")).toBe(9);
        expect(g.state("coins")).toBe(10);
    }, "and three seeds bought for 5");
});

scenario("the well fills the can", (g) => {
    g.check(() => put(8, 5.8), "the player by the well");
    g.wait(0.1);
    g.check(() => expect(g.state("prompt")).toBe("E: fill the can"), "it offers to fill the can");
    g.press("use");
    g.wait(0.1);
    g.check(() => {
        expect(g.state("can")).toBe(5);
        expect(g.count("can.filled")).toBe(1);
    }, "five waterings in it");
});

scenario("rain waters every sown plot and leaves the bare ones", (g) => {
    g.check(() => {
        world.set("Farm/Plot0", "Plot", { stage: 1, water: 0 });
        world.set("Farm/Plot5", "Plot", { stage: 0, water: 0 });
        world.set("Weather", "Weather", { wet: 0.6 });
    }, "two plots sown and dry, the ground wet");
    g.wait(0.1);
    g.check(() => {
        expect(plot(0).water).toBeGreaterThan(0.95);
        expect(plot(5).water).toBeGreaterThan(0.95);
        expect(plot(1).water).toBe(0);
    }, "both sown plots watered, a bare one not");
});
