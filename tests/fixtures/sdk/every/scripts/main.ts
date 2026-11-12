import { game } from "amoris";
import { bodies } from "./bodies";
import { Every } from "./every";
import { hierarchy } from "./hierarchy";
import { all } from "./rules";
import { scenes } from "./scenes";

export default game({ components: [Every], systems: [all, bodies, hierarchy, scenes] });
