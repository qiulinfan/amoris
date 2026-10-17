import { game } from "amoris";
import { Crew, Tally, Log, Cargo } from "./components";
import { muster, log, takeAboard } from "./rules";
import { ShipPart, dressShip } from "./parts";

export default game({ components: [Crew, Tally, Log, Cargo, ShipPart],
    systems: [muster, log, takeAboard, dressShip] });
