# npm packages in a game

`scripts/main.ts` imports three packages by name (`docs/sdk.md`, Packages):

- `simplex-noise` 4.0.3, an ES module chosen through `exports` (`import`), fed the seeded `Math.random`;
- `inkjs` 2.4.0 through the `./full` subpath (`exports["./full"].import`, an `.mjs` file), whose compiler turns an ink story into a playable one at run time;
- `seedrandom` 3.0.5, CommonJS, which tries `require('crypto')` and falls back (its `"browser": {"crypto": false}` makes that an empty object).

`package.json` pins the versions measured. The directory is not a sample (its `node_modules` is not in the repository); to repeat, copy it somewhere, `npm install`, then:

```bash
./.pocket/pocket ts <dir> --out <dir>.js        # bundled 41 modules, 444 KB
./.pocket/pocket check <dir>                    # no type errors (inkjs and simplex-noise ship types; @types/seedrandom)
./build/debug/bin/pocket_runtime --project <dir> --bundle <dir>.js --project-config <dir>.js.project.json --headless --frames 3 --json
./.pocket/pocket pack <dir> --web --config wasm-small --out dist/web/pkgdemo   # then window.pocket.command("state") on the page
```

The state, natively (JavaScriptCore, seed 1) and in Chromium (its own JavaScript engine, the web pack, seed 1), was the same:

| key | value |
|---|---|
| `noise` | 0.793415 |
| `ink.first` | A fork in the road. |
| `ink.picked` | Go left\|Go right -> You went right.\nThe end. |
| `seedrandom` | 0.5463663768140734 (Node gives the same for `seedrandom("hello")()`) |

With `--seed 2`, `noise` is 0.657174; seed 1 again gives 0.793415. `runtime_tests [random]` pins the first three `Math.random` values for seed 7 to the ones Node computes with the same generator.
