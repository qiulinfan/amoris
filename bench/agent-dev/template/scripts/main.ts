import { game } from "pocket";
import { Cargo, Command, DamageInput, DashInput, DashState, Progress, Vitals, Wallet } from "./components";
import { collect, countParcels } from "./collect";
import { moveCourier } from "./dash";
import { updateProgress } from "./progress";
import { applyDamage } from "./damage";

export default game({
    components: [Cargo, Command, DamageInput, DashInput, DashState, Progress, Vitals, Wallet],
    systems: [countParcels, moveCourier, collect, updateProgress, applyDamage],
});
