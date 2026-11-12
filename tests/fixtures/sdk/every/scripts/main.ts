import { game } from "amoris";
import { bodies } from "./bodies";
import { Every } from "./every";
import { all } from "./rules";

export default game({ components: [Every], systems: [all, bodies] });
