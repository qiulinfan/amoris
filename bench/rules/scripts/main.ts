// The rule workload of docs/spec/budgets.md 4: its components and its two rules.
import { game } from "amoris";
import { Armor, Attack, Health, Spot, Stats } from "./components";
import { damage, populate } from "./rules";

export default game({
    components: [Attack, Spot, Health, Armor, Stats],
    systems: [populate, damage],
});
