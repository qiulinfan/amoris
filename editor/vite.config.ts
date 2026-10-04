// The editor's dev server and build. In development every host endpoint (docs/spec/host-protocol.md
// section 1) is proxied to the host named by POCKET_HOST: the real `pocket editor` at 127.0.0.1:7878
// by default, or the mock (`bun run dev:mock` sets it to the mock's port).
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const host = process.env.POCKET_HOST ?? "http://127.0.0.1:7878";
const port = Number(process.env.EDITOR_PORT ?? 5173);

const http = { target: host, changeOrigin: true };
const ws = { target: host, changeOrigin: true, ws: true };

export default defineConfig({
  plugins: [react()],
  server: {
    host: "127.0.0.1",
    port,
    proxy: {
      "/api": http,
      "/ws": ws,
      "/render": ws,
      "/assets": http,
      "/wasm": http,
      "/mcp": http,
      // No /devtools or /json: the host's CDP endpoint is pocket-debug's own port (9229), and the
      // editor debugs through `debug.*` over /ws (docs/spec/editor.md 8.1).
    },
    // The SDK's declarations (../crates/pocket-script/src/prelude/pocket.d.ts) are read at build
    // time for Monaco, the fallback until the host sends the project's.
    fs: { allow: [".."] },
  },
  build: {
    outDir: "dist",
    // `/assets/<path>` belongs to the host (project assets), so the bundle lives under `/_app/`.
    assetsDir: "_app",
    target: "es2023",
    chunkSizeWarningLimit: 8000,
  },
  worker: { format: "es" },
});
