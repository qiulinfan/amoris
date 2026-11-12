// The mock's component registry: what `world.schema` answers. Engine components are written in the
// shape `schemars` gives the Rust types (crates/amoris-physics: Transform, Velocity, RigidBody,
// Collider, Boat, Sail, Sea, Wind), project components in the shape `ComponentSchema::json_schema`
// gives TypeScript components (samples/sailing/scripts/components.ts). The editor builds its
// inspector from these alone.
//
// The visual components (Model, Light, Camera, Splat; crates/amoris-assets/src/visual.rs) are copied
// by hand from the host's own `world.schema` answer: its fields in its order, its defaults, its
// enum values in its spelling (the longest docs shortened). tools/edit-host.ts compares them with a
// real host's when it runs against one, so a copy that drifts fails there.

export interface ComponentInfo {
  name: string;
  origin: "Engine" | "Project";
  version: number;
  doc: string;
  schema: Record<string, unknown>;
}

const MAX_ENTITY_ID = 2 ** 53 - 1;
const SCHEMA = "https://json-schema.org/draft/2020-12/schema";

const entityIdDef = {
  type: "integer",
  minimum: 1,
  maximum: MAX_ENTITY_ID,
  description: "An entity's id: a whole number from 1 to 2^53 - 1, never reused.",
};

/** `{default}` when the field has one (the host's schemas carry each field's default). */
const withDefault = (value: unknown) => (value === undefined ? {} : { default: value });
const v3 = (description: string, value?: [number, number, number]) => ({
  ...withDefault(value),
  description,
  type: "array",
  items: { type: "number", format: "double" },
  minItems: 3,
  maxItems: 3,
});
const v4 = (description: string, value?: [number, number, number, number]) => ({
  ...withDefault(value),
  description,
  type: "array",
  items: { type: "number", format: "double" },
  minItems: 4,
  maxItems: 4,
});
const num = (description: string, extra: Record<string, unknown> = {}) => ({
  description,
  type: "number",
  format: "double",
  ...extra,
});
const bool = (description: string, value?: boolean) => ({ ...withDefault(value), description, type: "boolean" });
const text = (description: string, value?: string) => ({ ...withDefault(value), description, type: "string" });
/** A number the host leaves out unless set (`Option<f64>`). */
const optional = (description: string) => ({ default: null, description, type: ["number", "null"], format: "double" });
const unit = (value: string, description?: string) => ({
  type: "string",
  const: value,
  ...(description ? { description } : {}),
});

function engine(
  name: string,
  doc: string,
  properties: Record<string, unknown>,
  defs?: Record<string, unknown>,
  version = 1,
): ComponentInfo {
  return {
    name,
    origin: "Engine",
    version,
    doc,
    schema: {
      $schema: SCHEMA,
      title: name,
      description: doc,
      type: "object",
      properties,
      additionalProperties: false,
      ...(defs ? { $defs: defs } : {}),
    },
  };
}

type ProjectField =
  | ["f64" | "u32" | "i32" | "bool" | "entity" | "str" | "vec3", string]
  | ["enum", string, string[]];

function project(name: string, doc: string, fields: Record<string, ProjectField>): ComponentInfo {
  const props: Record<string, unknown> = {};
  for (const [field, spec] of Object.entries(fields)) {
    const [ty, description] = spec;
    let s: Record<string, unknown>;
    switch (ty) {
      case "f64":
        s = { type: "number" };
        break;
      case "u32":
        s = { type: "integer", format: "uint32", minimum: 0 };
        break;
      case "i32":
        s = { type: "integer", format: "int32" };
        break;
      case "bool":
        s = { type: "boolean" };
        break;
      case "entity":
        s = { type: ["integer", "null"], minimum: 1, maximum: MAX_ENTITY_ID };
        break;
      case "str":
        s = { type: "string" };
        break;
      case "vec3":
        s = {
          type: "object",
          properties: { x: { type: "number" }, y: { type: "number" }, z: { type: "number" } },
          required: ["x", "y", "z"],
          additionalProperties: false,
        };
        break;
      case "enum":
        s = { type: "string", enum: spec[2] };
        break;
    }
    props[field] = { ...s, description };
  }
  return {
    name,
    origin: "Project",
    version: 1,
    doc,
    schema: {
      title: name,
      description: doc,
      type: "object",
      properties: props,
      required: Object.keys(fields),
      additionalProperties: false,
    },
  };
}

export const COMPONENTS: ComponentInfo[] = [
  engine(
    "Transform",
    "Where an entity is: its origin and orientation in the world (metres; a unit quaternion `[x, y, z, w]`). Physics writes it back after every step for the bodies it moves; a write from outside moves the body there at the next step.",
    {
      position: v3("The origin in the world, metres."),
      rotation: v4("The orientation, a unit quaternion `[x, y, z, w]`."),
    },
  ),
  engine(
    "Velocity",
    "How a body moves: the velocity of its centre of mass (m/s) and its angular velocity (rad/s), both in the world frame.",
    {
      linear: v3("Linear velocity of the centre of mass, m/s."),
      angular: v3("Angular velocity, rad/s."),
    },
  ),
  engine(
    "RigidBody",
    "A rigid body: what kind it is, its mass properties and its damping.",
    {
      kind: { $ref: "#/$defs/BodyKind" },
      mass: num("kg; 0 takes the mass from the collider's density.", { minimum: 0 }),
      linear_damping: num("Linear damping, 1/s.", { minimum: 0 }),
      angular_damping: num("Angular damping, 1/s.", { minimum: 0 }),
      gravity_scale: num("Multiplies gravity for this body."),
      ccd: bool("Continuous collision detection for fast bodies."),
    },
    {
      BodyKind: {
        description: "What kind of body.",
        oneOf: [unit("Dynamic", "Moved by forces, gravity and contacts."), unit("Fixed", "Never moves (an island, a pier).")],
      },
    },
  ),
  engine(
    "Collider",
    "The shape a body collides and floats with, its density, friction and restitution.",
    {
      shape: { $ref: "#/$defs/Shape" },
      density: { description: "kg/m^3; none when the body states its mass.", type: ["number", "null"], format: "double" },
      friction: num("Coulomb friction coefficient.", { minimum: 0 }),
      restitution: num("Bounciness, 0..1.", { minimum: 0, maximum: 1 }),
      sensor: bool("Reports overlaps without pushing."),
    },
    {
      Shape: {
        description: "A collision shape in the body's frame.",
        oneOf: [
          {
            type: "object",
            properties: { Ball: { type: "object", properties: { radius: num("Metres.") }, required: ["radius"], additionalProperties: false } },
            required: ["Ball"],
            additionalProperties: false,
            description: "A sphere.",
          },
          {
            type: "object",
            properties: {
              Cuboid: { type: "object", properties: { half_extents: v3("Half the size along x, y and z, metres.") }, required: ["half_extents"], additionalProperties: false },
            },
            required: ["Cuboid"],
            additionalProperties: false,
            description: "A box.",
          },
          {
            type: "object",
            properties: {
              Capsule: {
                type: "object",
                properties: { half_height: num("Half the straight part, metres."), radius: num("Metres.") },
                required: ["half_height", "radius"],
                additionalProperties: false,
              },
            },
            required: ["Capsule"],
            additionalProperties: false,
            description: "A capsule along y.",
          },
        ],
      },
    },
  ),
  engine(
    "Boat",
    "A boat: its controls, the actuators that follow them, and what it reads. Scripts, intents and boundary writes set the controls; the engine moves the actuators and writes the readings.",
    {
      hoist: num("Control, 0..1: how much sail to set; 0 furled.", { minimum: 0, maximum: 1 }),
      sheet: num("Control, 0..1: 0 hard in (the boom on the centreline), 1 eased right out.", { minimum: 0, maximum: 1 }),
      rudder: num("Control, -1..1: positive turns the bow to starboard.", { minimum: -1, maximum: 1 }),
      hoist_now: num("The sail as set now (follows `hoist`)."),
      sheet_now: num("The sheet as eased now (follows `sheet`)."),
      rudder_now: num("The rudder blade now (follows `rudder`)."),
      speed: num("Written by the engine: speed through the water along the heading, m/s."),
      heading_deg: num("Written by the engine: where the bow points, degrees (0 toward -z, 90 toward +x)."),
      heel_deg: num("Written by the engine: positive heeled to starboard, degrees."),
      afloat: bool("Written by the engine: the hull is in the water, not sunk beneath it, and not aground."),
      aground: bool("Written by the engine: the boat touches land (a fixed body) after the step."),
      awa_deg: num("Written by the engine: apparent wind angle off the bow, positive over the starboard side."),
      aws: num("Written by the engine: apparent wind speed, m/s."),
      boom_deg: num("Written by the engine: the boom's angle from the centreline, positive to starboard."),
      drive: num("Written by the engine: the sail's drive as a share of the best at this apparent wind."),
      trim: { $ref: "#/$defs/Trim" },
    },
    {
      Trim: {
        description: "How the sail is trimmed, from its angle of attack.",
        oneOf: [
          unit("Furled"),
          unit("Luffing", "Eased too far: the sail streams with the wind and draws little."),
          unit("Good"),
          unit("Overtrimmed", "In too far: the sail stalls."),
        ],
      },
    },
  ),
  engine(
    "Sail",
    "The boat's sail: a flat plate on a boom pivoting at the mast. Positions are in the boat's frame.",
    {
      mast: v3("The mast's foot, metres in the boat's frame."),
      boom: num("The boom's length, metres."),
      rise: num("The centre of effort's height above the mast's foot, metres."),
      area: num("Sail area, m^2."),
    },
  ),
  engine(
    "Sea",
    "A body of water to the horizon: its level at rest, its density (kg/m^3) and its waves.",
    {
      level: num("The surface at rest, metres."),
      density: num("kg/m^3."),
      waves: {
        type: "array",
        items: { $ref: "#/$defs/Wave" },
      },
    },
    {
      Wave: {
        description: "One sine wave of the sea, running at the deep-water speed of its length.",
        type: "object",
        properties: {
          length: num("Crest to crest, metres (above 0)."),
          height: num("Crest to trough, metres."),
          toward_deg: num("The bearing the crests travel toward, degrees (0 toward -z, 90 toward +x)."),
          phase: num("Phase at time 0 and the origin, radians."),
        },
        required: ["length", "height", "toward_deg", "phase"],
        additionalProperties: false,
      },
    },
  ),
  engine(
    "Wind",
    "The true wind: from a bearing at a speed, swelling and easing in gusts that sweep downwind.",
    {
      from_deg: num("Where the wind blows from, degrees (270: a westerly, blowing toward +x)."),
      speed: num("m/s.", { minimum: 0 }),
      gust: num("The gusts' share of the speed, 0 for a steady wind.", { minimum: 0 }),
      gust_period: num("Seconds between gusts at a point (above 0)."),
      gust_length: num("Metres between gusts along the wind (above 0)."),
    },
  ),
  engine(
    "Model",
    "What an entity looks like: a mesh drawn with a material, at its `Transform`, scaled.\n\n`mesh` names a built-in primitive (`cube`, `sphere`, `plane`, `cylinder`, `capsule`, `cone`,\n`torus`) or a mesh of an asset: `models/boat.glb` (every mesh of the file's default scene) or\n`models/boat.glb#Hull` (the mesh named Hull; `#2` is the third). `material` is empty for the\ninline parameters below, or names an asset's material (`models/boat.glb#Sail`), whose values\nthe inline `color` multiplies.",
    {
      cast_shadows: bool("Whether it casts shadows.", true),
      color: v4("Base color, linear RGBA; multiplies the material's. An alpha below 1 draws the entity\nblended, sorted with the other transparent ones.", [1, 1, 1, 1]),
      emissive: v3("Emitted light, linear RGB in nits-like units (1 = as bright as a lit white surface).", [0, 0, 0]),
      ior: optional("Optional index of refraction override; None preserves the material's."),
      material: text("Empty for the inline material, or an asset material path.", ""),
      mesh: text("A primitive name or an asset mesh path.", "cube"),
      metallic: num("0 dielectric to 1 metal.", { default: 0 }),
      roughness: num("0 mirror to 1 rough.", { default: 0.5 }),
      scale: v3("Scale along the entity's own axes.", [1, 1, 1]),
      transmission: optional("Optional transmission override, 0 to 1; None preserves the imported material's. Above 0 the\nentity is glass."),
      visible: bool("Whether it is drawn.", true),
    },
    undefined,
    2,
  ),
  engine(
    "Light",
    "A light at the entity's `Transform`.",
    {
      color: v3("Linear RGB.", [1, 0.96, 0.9]),
      inner_deg: num("Spot: the full-intensity cone's half angle, degrees.", { default: 25 }),
      intensity: num("Directional: illuminance in lux-like units (the sun at noon is about 10). Point and spot:\nluminous intensity, candela-like.", { default: 6 }),
      kind: { $ref: "#/$defs/LightKind", default: "directional" },
      outer_deg: num("Spot: the cone's half angle, degrees.", { default: 35 }),
      range: num("Point and spot: where the light fades to nothing, metres.", { default: 20 }),
      shadows: bool("Whether it casts shadows (directional: cascaded shadow maps).", true),
    },
    {
      LightKind: {
        description: "What kind of light.",
        oneOf: [
          unit("directional", "Parallel light from infinitely far along the entity's -z (the sun)."),
          unit("point", "From the entity's origin in every direction."),
          unit("spot", "From the entity's origin in a cone along its -z."),
        ],
      },
    },
  ),
  engine(
    "Camera",
    "A camera at the entity's `Transform`, looking along its -z with +y up.",
    {
      active: bool("The camera the game is shown through; the lowest-id active camera wins.", true),
      exposure_ev: num("Exposure compensation, stops.", { default: 0 }),
      far: num("Far plane, metres; 0 for infinitely far (reversed-Z makes that free).", { default: 0 }),
      fov_deg: num("Vertical field of view, degrees.", { default: 60 }),
      near: num("Near plane, metres.", { default: 0.1 }),
    },
  ),
  engine(
    "Splat",
    "A 3D Gaussian Splatting cloud (charter 4.4, neural rendering) at the entity's `Transform`:\n`asset` names a `.ply` (the 3DGS training output layout) or `.splat` file.",
    {
      asset: { default: "", type: "string" },
      scale: num("Uniform scale.", { default: 1 }),
      visible: { default: true, type: "boolean" },
    },
  ),
  engine(
    "Parent",
    "Places an entity under another in the hierarchy; its Transform stays in world space.",
    { parent: { $ref: "#/$defs/EntityId", description: "The parent entity." } },
    { EntityId: entityIdDef },
  ),
  project("Crew", "The boat's crew and what they are told to do.", {
    take: ["entity", "A crate to take aboard at the next tick; cleared once the crew tried."],
  }),
  project("Tally", "Crates aboard and crates there were.", {
    taken: ["u32", "Crates taken aboard."],
    worth: ["u32", "The value of the crates aboard."],
    total: ["u32", "Crates adrift when the game began."],
  }),
  project("Log", "The ship's log.", {
    distance: ["f64", "Metres sailed through the water."],
    top_speed: ["f64", "The best speed through the water so far, m/s."],
    sail_set: ["bool", "Whether the sail stood set at the last tick."],
  }),
  project("Cargo", "A crate adrift that a crew can take aboard.", {
    value: ["u32", "What it is worth."],
  }),
];

/** `Name` is a component whose value is a plain string (amoris-sim's `Name`). */
export const NAME_INFO: ComponentInfo = {
  name: "Name",
  origin: "Engine",
  version: 1,
  doc: "The engine's plain-data name: at most 64 UTF-8 bytes, not necessarily unique.",
  schema: { $schema: SCHEMA, title: "Name", type: "string", maxLength: 64 },
};

export function allComponents(): ComponentInfo[] {
  return [...COMPONENTS, NAME_INFO].sort((a, b) => a.name.localeCompare(b.name));
}

export function componentInfo(name: string): ComponentInfo | undefined {
  return name === "Name" ? NAME_INFO : COMPONENTS.find((c) => c.name === name);
}
