// Import by upload (editor.md 6.3): files picked in the browser or dropped on the Assets panel are
// sent into the project with `assets.upload` (server.md 3), in base64 parts of at most 1 MiB, each
// into the folder of its kind (`models/`, `textures/`, `splats/`, `audio/`, `scripts/`, else
// `files/`). A path the project already has is offered for a new path or a replacement.

import { api } from "../host/api";
import { HostError } from "../host/client";
import { useAssets } from "../state/assets";
import { promptText, useUi } from "../state/ui";
import { bytes as size } from "../ui/format";
import { attempt } from "./report";

/** The bytes one part holds: the host refuses more (`amoris_server::UPLOAD_PART`). */
export const UPLOAD_PART = 1 << 20;

/** The folder a file goes to, by its extension (the kinds of the host's `assets.list`). */
export function uploadDir(name: string): string {
  const ext = name.includes(".") ? name.split(".").pop()!.toLowerCase() : "";
  if (["gltf", "glb", "obj", "fbx", "bin"].includes(ext)) return "models";
  if (["png", "jpg", "jpeg", "ktx2", "hdr", "exr", "webp"].includes(ext)) return "textures";
  if (["ply", "splat", "spz"].includes(ext)) return "splats";
  if (["wav", "ogg", "mp3", "flac"].includes(ext)) return "audio";
  if (["ts", "js"].includes(ext)) return "scripts";
  return "files";
}

/** Base64 of bytes, as the host decodes it (the standard alphabet, padded). */
export function toBase64(data: Uint8Array): string {
  let s = "";
  for (let i = 0; i < data.length; i += 0x8000) s += String.fromCharCode(...data.subarray(i, i + 0x8000));
  return btoa(s);
}

/** Sends `data` to `path` in parts; the host's answer to the last one. */
export async function upload(path: string, data: Uint8Array, overwrite = false, progress?: (sent: number) => void) {
  let offset = 0;
  for (;;) {
    const part = data.subarray(offset, offset + UPLOAD_PART);
    const done = offset + part.length >= data.length;
    const r = await api.assets.upload({ path, data: toBase64(part), offset, done, ...(overwrite ? { overwrite } : {}) });
    offset += part.length;
    progress?.(offset);
    if (done) return r as { path: string; kind: string; bytes: number };
  }
}

/** Uploads each file into its kind's folder, asking for another path (or a replacement) when the
 * project has one there already; refreshes the asset list after. */
export async function importFiles(files: { name: string; arrayBuffer(): Promise<ArrayBuffer> }[]) {
  const ui = useUi.getState();
  for (const f of files) {
    const data = new Uint8Array(await f.arrayBuffer());
    let path = `${uploadDir(f.name)}/${f.name}`;
    let overwrite = false;
    for (;;) {
      try {
        const r = await upload(path, data, overwrite);
        ui.toast({ kind: "success", title: "Imported", body: `${r.path} (${r.kind}, ${size(r.bytes)})` });
        break;
      } catch (e) {
        if (!(e instanceof HostError) || e.code !== "assets.exists") {
          ui.toast({ kind: "error", title: `Import ${f.name}`, body: e instanceof Error ? e.message : String(e) });
          break;
        }
        const was = path;
        const next = await promptText({
          title: `${was} exists`,
          label: "The project has a file there. Keep the path to replace it, or type another.",
          value: was,
          confirm: "Upload",
        });
        if (!next) break;
        path = next.trim();
        overwrite = path === was;
      }
    }
  }
  const list = await attempt(api.assets.list(), "assets.list");
  if (list) useAssets.getState().set(list);
}

/** Opens the browser's file chooser and imports what is picked. */
export function chooseFiles() {
  const input = document.createElement("input");
  input.type = "file";
  input.multiple = true;
  input.style.display = "none";
  input.addEventListener("change", () => {
    const files = [...(input.files ?? [])];
    input.remove();
    if (files.length) void importFiles(files);
  });
  input.addEventListener("cancel", () => input.remove());
  document.body.append(input);
  input.click();
}
