// The sailing game (charter 2.4.1): its components and rules.
import { game } from "pocket";
import { Cargo, Crew, Log, Tally } from "./components";
import { log, muster, takeAboard } from "./rules";

export default game({
    components: [Crew, Tally, Log, Cargo],
    systems: [muster, log, takeAboard],
});
