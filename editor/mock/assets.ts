// The mock's project assets (`assets.list`): kinds the charter names (glTF models, textures, splats,
// neural textures, audio) with small generated SVG thumbnails for some, so both thumbnail and
// kind-icon cards show in the editor.

export interface AssetInfo {
  path: string;
  kind: string;
  bytes: number;
  thumbnail?: string;
}

function svg(body: string): string {
  return `data:image/svg+xml;utf8,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">${body}</svg>`)}`;
}

const planks = svg(
  `<rect width="64" height="64" fill="#7a5230"/>${[0, 1, 2, 3]
    .map((i) => `<rect y="${i * 16}" width="64" height="15" fill="${["#8a5d36", "#6f4a2a", "#93653b", "#7d5531"][i]}"/><line x1="${(i * 23) % 64}" y1="${i * 16}" x2="${(i * 23) % 64}" y2="${i * 16 + 15}" stroke="#4b3019" stroke-width="1"/>`)
    .join("")}`,
);
const seaNormal = svg(
  `<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#8080ff"/><stop offset="1" stop-color="#6f8fe8"/></linearGradient></defs><rect width="64" height="64" fill="url(#g)"/>${Array.from({ length: 7 }, (_, i) => `<path d="M0 ${6 + i * 9} Q16 ${2 + i * 9} 32 ${6 + i * 9} T64 ${6 + i * 9}" stroke="#a7b4ff" stroke-width="2" fill="none" opacity="0.7"/>`).join("")}`,
);
const canvas = svg(
  `<rect width="64" height="64" fill="#e9e4d6"/>${Array.from({ length: 16 }, (_, i) => `<line x1="${i * 4}" y1="0" x2="${i * 4}" y2="64" stroke="#d8d1bd" stroke-width="1"/><line y1="${i * 4}" x1="0" y2="${i * 4}" x2="64" stroke="#d8d1bd" stroke-width="1"/>`).join("")}`,
);
const sky = svg(
  `<defs><linearGradient id="s" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#3b6fb6"/><stop offset="0.6" stop-color="#a9c8ea"/><stop offset="1" stop-color="#f3d9b1"/></linearGradient></defs><rect width="64" height="64" fill="url(#s)"/><circle cx="46" cy="40" r="6" fill="#fff6d8"/>`,
);
const crate = svg(
  `<rect width="64" height="64" fill="#20242c"/><g transform="translate(32 34)"><path d="M0 -18 L18 -8 L0 2 L-18 -8 Z" fill="#a8743f"/><path d="M-18 -8 L0 2 L0 22 L-18 12 Z" fill="#7a5230"/><path d="M18 -8 L0 2 L0 22 L18 12 Z" fill="#8f6236"/></g>`,
);
const sloop = svg(
  `<rect width="64" height="64" fill="#20242c"/><path d="M12 42 L52 42 L46 50 L18 50 Z" fill="#dfe3e8"/><line x1="32" y1="10" x2="32" y2="42" stroke="#c9b18a" stroke-width="2"/><path d="M33 12 L33 40 L50 40 Z" fill="#f4f1e8"/><path d="M31 16 L31 40 L18 40 Z" fill="#e7e2d4"/>`,
);

export const ASSETS: AssetInfo[] = [
  { path: "models/sloop.glb", kind: "model", bytes: 1_284_310, thumbnail: sloop },
  { path: "models/crate.glb", kind: "model", bytes: 214_880, thumbnail: crate },
  { path: "models/buoy.glb", kind: "model", bytes: 96_412 },
  { path: "models/island.glb", kind: "model", bytes: 6_912_004 },
  { path: "models/pier.glb", kind: "model", bytes: 842_118 },
  { path: "textures/planks_albedo.png", kind: "texture", bytes: 2_097_152, thumbnail: planks },
  { path: "textures/sea_normal.png", kind: "texture", bytes: 4_194_304, thumbnail: seaNormal },
  { path: "textures/sail_canvas.png", kind: "texture", bytes: 1_048_576, thumbnail: canvas },
  { path: "textures/sky_dusk.ktx2", kind: "texture", bytes: 8_388_608, thumbnail: sky },
  { path: "splats/rock_garden.splat", kind: "splat", bytes: 31_457_280 },
  { path: "splats/harbour.ply", kind: "splat", bytes: 88_080_384 },
  { path: "neural/planks.ntc", kind: "neural", bytes: 393_216 },
  { path: "neural/sail_canvas.ntc", kind: "neural", bytes: 262_144 },
  { path: "audio/waves.ogg", kind: "audio", bytes: 1_572_864 },
  { path: "audio/creak.ogg", kind: "audio", bytes: 98_304 },
  { path: "scenes/scene.json", kind: "scene", bytes: 1_342 },
];
