# Debugging game scripts in VS Code

`launch.json` attaches VS Code's JavaScript debugger to a running game's script debugger
(docs/spec/debugger.md 9): breakpoints, stepping, variables and the Debug Console work in the
TypeScript under `samples/<game>/scripts/`.

1. Start a game with the debugger's endpoint, here the sailing sample:

   ```sh
   cargo run -p pocket-app --example debug_sailing            # or add --wait to hold the first tick
   ```

   It listens on `127.0.0.1:9229` (`--port` to change; update `port` below to match).
2. Copy `launch.json` to `.vscode/launch.json` at the repository root (or merge its configuration
   into yours), open `samples/sailing/scripts/rules.ts`, set a breakpoint, and run
   **Attach to Amoris (sailing)** (F5).

What the configuration does:

- `"type": "node", "request": "attach", "port": 9229`: the endpoint answers like a Node inspector
  (`/json/list`, one target, `pocket-game`).
- `"resolveSourceMapLocations": null`: the scripts are `pocket:///scripts/*.js`, not files, and
  js-debug only reads source maps of files under the workspace unless this is null.
- `"sourceMapPathOverrides": {"pocket:///*": "${workspaceFolder}/samples/sailing/*"}`: the source maps
  name `pocket:///scripts/<module>.ts`; this maps them to the project's files. For another project
  change the path, or start the game with `--file-sources` (absolute `file://` sources, no override
  needed).
- `"continueOnAttach": true`: a game started with `--wait` starts when VS Code has attached.

The same attach was checked headless with VS Code's own js-debug (crates/pocket-debug/tests/
jsdebug_dap.mjs, docs/evidence/debug/jsdebug-dap.txt).
