// The debugger's evaluation-scope test game.
import { game } from "amoris";
import { Heading } from "./components";
import { steer } from "./rules";

export default game({
    components: [Heading],
    systems: [steer],
});
