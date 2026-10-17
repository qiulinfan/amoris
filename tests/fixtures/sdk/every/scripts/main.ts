import { game } from "amoris";
import { Every } from "./every";
import { all } from "./rules";

export default game({ components: [Every], systems: [all] });
