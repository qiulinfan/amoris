// The sailing course (docs/spec/player.md): its components and rules. The player plays it through
// the player tools; the boat's controls and the sailing intents are the engine's.
import { game } from "pocket";
import { Cargo, Course, Crew, Mark, Tally } from "./components";
import { muster, rounding, takeAboard } from "./rules";

export default game({
    components: [Crew, Tally, Cargo, Mark, Course],
    systems: [muster, takeAboard, rounding],
});
