// Floater forces come from physics.buoyancy and current GPU field samples, without a script
// moving the bodies. The four crates each contribute 27 volume points to the shared readback.
import { game } from "amoris";

export default game({ components: [], systems: [] });
