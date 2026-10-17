// The debugger's test game.
import { game } from "amoris";
import { Gauge } from "./components";
import { measure, probe } from "./rules";

export default game({
    components: [Gauge],
    systems: [measure, probe],
});
