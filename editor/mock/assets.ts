// The mock's project assets (`assets.list`): kinds the charter names (glTF models, textures, splats,
// neural textures, audio) with small generated SVG thumbnails for some, so both thumbnail and
// kind-icon cards show in the editor. And `assets.upload`, which adds to them as the host does.

import { MockError } from "./util";

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

/** A file's kind by its extension, as the host's `assets.list` names it (amoris-server local.rs
 * `kind_of`). */
export function assetKind(path: string): string {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  if (["gltf", "glb", "obj", "fbx"].includes(ext)) return "model";
  if (["png", "jpg", "jpeg", "ktx2", "hdr", "exr", "webp"].includes(ext)) return "texture";
  if (["ply", "splat", "spz"].includes(ext)) return "splat";
  if (["ts", "js"].includes(ext)) return "script";
  if (["wav", "ogg", "mp3", "flac"].includes(ext)) return "audio";
  if (["json", "toml", "jsonl"].includes(ext)) return "data";
  if (ext === "wgsl") return "shader";
  return "file";
}

/** The most bytes one `assets.upload` part holds (amoris-server's `UPLOAD_PART`). */
export const UPLOAD_PART = 1 << 20;

const invalid = (field: string, message: string) => new MockError("request.invalid_value", message, { path: `/${field}` });

/** A relative path inside the project: no `..`, no root, no dot files (local.rs `inside`). */
function inside(path: string): boolean {
  if (!path || path.startsWith("/")) return false;
  return path.split("/").filter((s) => s !== "").every((s) => s !== ".." && !s.startsWith("."));
}

function decode(data: string): Uint8Array {
  if (data.length % 4 !== 0) throw invalid("data", `The data is not base64: its length ${data.length} is not a multiple of 4.`);
  if (!/^[A-Za-z0-9+/]*={0,2}$/.test(data)) throw invalid("data", "The data is not base64: it holds a character outside the alphabet.");
  return Uint8Array.from(atob(data), (c) => c.charCodeAt(0));
}

/** `assets.upload` as the host answers it: parts appended from offset 0, the file listed at the
 * last part, the same refusals with the same codes, messages and details. */
export function uploadPart(s: { assets: AssetInfo[]; uploads: Map<string, number> }, p: Record<string, unknown>) {
  const path = String(p.path);
  const offset = typeof p.offset === "number" ? p.offset : 0;
  const done = p.done !== false;
  const overwrite = p.overwrite === true;
  if (!inside(path) || path.endsWith("/")) throw invalid("path", `'${path}' is not a file path inside the project.`);
  const bytes = decode(String(p.data));
  if (bytes.length > UPLOAD_PART) throw invalid("data", `A part holds at most ${UPLOAD_PART} bytes; this one holds ${bytes.length}.`);
  const exists = () => new MockError("assets.exists", `The project already has '${path}'; upload with overwrite to replace it.`, { path });
  if (!overwrite && s.assets.some((a) => a.path === path)) throw exists();
  const had = offset === 0 ? 0 : (s.uploads.get(path) ?? 0);
  if (had !== offset) {
    throw new MockError("assets.upload_offset", `This part of '${path}' starts at ${offset}, but the parts received end at ${had}: upload from offset ${had}, or from 0 again.`, {
      path,
      expected: had,
      got: offset,
    });
  }
  const received = offset + bytes.length;
  if (!done) {
    s.uploads.set(path, received);
    return { path, received };
  }
  s.uploads.delete(path);
  const asset = { path, kind: assetKind(path), bytes: received };
  const i = s.assets.findIndex((a) => a.path === path);
  if (i >= 0) s.assets[i] = asset;
  else s.assets.push(asset);
  return asset;
}
