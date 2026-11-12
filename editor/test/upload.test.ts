// Import by upload (editor.md 6.3): what the editor sends for a file (`assets.upload` parts of at
// most 1 MiB in base64, into its kind's folder) and the mock answering as the host does
// (crates/amoris-app/tests/assets_upload.rs holds the host to the same forms), over the wire.
import { expect, test } from "bun:test";
import { Session } from "../mock/session";
import { MockError } from "../mock/util";

const page = (globalThis as { window?: object }).window ?? {};
Object.defineProperty(globalThis, "window", { configurable: true, value: { ...page, location: new URL("http://127.0.0.1:7878/") } });
const { host } = await import("../src/host/api");
const { UPLOAD_PART, importFiles, toBase64, upload, uploadDir } = await import("../src/actions/assets");
const { useAssets } = await import("../src/state/assets");
const { useUi } = await import("../src/state/ui");

function wire(session = new Session(() => {})) {
  const sent: { method: string; params: Record<string, unknown> }[] = [];
  const client = host as unknown as { ws: unknown; receive(raw: string): void };
  client.ws = {
    readyState: WebSocket.OPEN,
    send(raw: string) {
      const m = JSON.parse(raw) as { id: number; method: string; params: Record<string, unknown> };
      sent.push({ method: m.method, params: m.params });
      queueMicrotask(() => {
        try {
          client.receive(JSON.stringify({ id: m.id, result: session.call(m.method, m.params) }));
        } catch (e) {
          if (!(e instanceof MockError)) throw e;
          client.receive(JSON.stringify({ id: m.id, error: e.toWire() }));
        }
      });
    },
  };
  return { session, sent };
}

const file = (name: string, data: Uint8Array) => ({ name, arrayBuffer: async () => data.slice().buffer });

test("base64 and the folders of the kinds", () => {
  expect(toBase64(new Uint8Array([]))).toBe("");
  expect(toBase64(new TextEncoder().encode("foobar"))).toBe("Zm9vYmFy");
  expect(toBase64(new Uint8Array([0, 255, 63, 251]))).toBe("AP8/+w==");
  expect(uploadDir("lighthouse.GLB")).toBe("models");
  expect(uploadDir("scene.bin")).toBe("models");
  expect(uploadDir("rock.png")).toBe("textures");
  expect(uploadDir("garden.splat")).toBe("splats");
  expect(uploadDir("wave.ogg")).toBe("audio");
  expect(uploadDir("README")).toBe("files");
});

test("a file of 2.5 MiB goes in three parts and lands in models/", async () => {
  const { session, sent } = wire();
  const data = Uint8Array.from({ length: UPLOAD_PART * 2.5 }, (_, i) => (i * 7) & 255);
  await importFiles([file("lighthouse.glb", data)]);
  const parts = sent.filter((m) => m.method === "assets.upload");
  expect(parts.map((m) => [m.params.path, m.params.offset, m.params.done, (m.params.data as string).length])).toEqual([
    ["models/lighthouse.glb", 0, false, Math.ceil(UPLOAD_PART / 3) * 4],
    ["models/lighthouse.glb", UPLOAD_PART, false, Math.ceil(UPLOAD_PART / 3) * 4],
    ["models/lighthouse.glb", UPLOAD_PART * 2, true, Math.ceil(UPLOAD_PART / 2 / 3) * 4],
  ]);
  expect(session.assets.find((a) => a.path === "models/lighthouse.glb")).toEqual({ path: "models/lighthouse.glb", kind: "model", bytes: data.length });
  // The list was asked again and holds it.
  expect(sent.at(-1)!.method).toBe("assets.list");
  expect(useAssets.getState().assets.some((a) => a.path === "models/lighthouse.glb")).toBe(true);
});

test("a path the project has asks for another, or replaces it when kept", async () => {
  const { session, sent } = wire();
  await upload("textures/dot.png", new Uint8Array([1, 2, 3]));
  // Kept: replaced with overwrite.
  const kept = importFiles([file("dot.png", new Uint8Array([9, 9]))]);
  await waitFor(() => useUi.getState().prompt !== null);
  expect(useUi.getState().prompt!.value).toBe("textures/dot.png");
  useUi.getState().prompt!.resolve("textures/dot.png");
  useUi.getState().set({ prompt: null });
  await kept;
  expect(sent.filter((m) => m.method === "assets.upload").at(-1)!.params.overwrite).toBe(true);
  expect(session.assets.find((a) => a.path === "textures/dot.png")!.bytes).toBe(2);
  // Another path: uploaded there, the first left as it was.
  const moved = importFiles([file("dot.png", new Uint8Array([7, 7, 7, 7]))]);
  await waitFor(() => useUi.getState().prompt !== null);
  useUi.getState().prompt!.resolve("textures/dot 2.png");
  useUi.getState().set({ prompt: null });
  await moved;
  expect(session.assets.find((a) => a.path === "textures/dot.png")!.bytes).toBe(2);
  expect(session.assets.find((a) => a.path === "textures/dot 2.png")!.bytes).toBe(4);
});

test("the mock refuses as the host does", () => {
  const s = new Session(() => {});
  const refuse = (p: Record<string, unknown>) => {
    try {
      s.call("assets.upload", p);
    } catch (e) {
      return e as MockError;
    }
    throw new Error("not refused");
  };
  for (const path of ["../escape.bin", "/tmp/escape.bin", ".amoris/x.bin", "models/", ""]) {
    const e = refuse({ path, data: "eA==" });
    expect([e.code, e.detail.path]).toEqual(["request.invalid_value", "/path"]);
  }
  expect(refuse({ path: "files/b.bin", data: "not base64!" }).detail.path).toBe("/data");
  expect(s.call("assets.upload", { path: "files/a.bin", data: "YWJj", done: false })).toEqual({ path: "files/a.bin", received: 3 });
  const e = refuse({ path: "files/a.bin", data: "ZGVm", offset: 5 });
  expect([e.code, e.detail.expected, e.detail.got]).toEqual(["assets.upload_offset", 3, 5]);
  expect(s.call("assets.upload", { path: "files/a.bin", data: "ZGVm", offset: 3 })).toEqual({ path: "files/a.bin", kind: "file", bytes: 6 });
  expect(refuse({ path: "files/a.bin", data: "eA==" }).code).toBe("assets.exists");
});

async function waitFor(test: () => boolean) {
  for (let i = 0; i < 200 && !test(); i++) await Bun.sleep(5);
  expect(test()).toBe(true);
}
