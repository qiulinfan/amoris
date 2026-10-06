import { game } from "pocket";
import { Helm, helm } from "./helm";
import { Crew, Tally, Log, Cargo } from "./components";
import { muster, log, takeAboard } from "./rules";
import { ShipPart, dressShip } from "./parts";

export default game({ components: [Helm, Crew, Tally, Log, Cargo, ShipPart],
    systems: [helm, muster, log, takeAboard, dressShip] });
