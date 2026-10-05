// `bun run dev:mock`: the mock host and the Vite dev server together, Vite proxying to the mock.
// Ctrl+C stops both.

const MOCK_PORT = process.env.MOCK_PORT ?? "7879";
const root = new URL("..", import.meta.url).pathname;

const mock = Bun.spawn(["bun", "mock/host.ts", "--port", MOCK_PORT, ...process.argv.slice(2)], {
  cwd: root,
  stdout: "inherit",
  stderr: "inherit",
});
const vite = Bun.spawn(["bun", "x", "vite"], {
  cwd: root,
  env: { ...process.env, POCKET_HOST: `http://127.0.0.1:${MOCK_PORT}` },
  stdout: "inherit",
  stderr: "inherit",
});

const stop = () => {
  mock.kill();
  vite.kill();
  process.exit(0);
};
process.on("SIGINT", stop);
process.on("SIGTERM", stop);
await Promise.race([mock.exited, vite.exited]);
stop();
