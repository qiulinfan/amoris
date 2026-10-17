// The web workload (crates/amoris-script/tests/web.rs): a regatta that exercises columns of every
// kind, the RNG streams, the replaced Math functions, events, spawns and despawns.
import { game } from "amoris";
import { Boat, Crate, Wind } from "./components";
import { setup, wind, sail, collect } from "./sailing";
import { retarget, names, lottery } from "./rules";

export default game({
    components: [Boat, Crate, Wind],
    systems: [setup, wind, retarget, sail, collect, names, lottery],
});
