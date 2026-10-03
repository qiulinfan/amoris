# Packaging

`pocket pack <project>` turns a project into a folder that runs on another machine of the same
system (macOS, Windows, Linux) without the repository, the toolchain or the TypeScript sources:

```
dist/<name>/
  <name>                launcher: exec bin/pocket_runtime with the paths below; extra arguments pass through
  bin/pocket_runtime    the engine (release configuration by default; SDL3, wgpu-native and FreeType are linked statically)
  project.js            the game's scripts, bundled by `pocket ts`
  project.json          settings from project.toml plus the font path
  project/              project.toml, the scene, assets/ and .imported/ (scripts/ and other dot-folders are left out)
  fonts/                the UI font
  README.txt
```

```bash
./.pocket/pocket pack assets                  # dist/assets/
./.pocket/pocket pack assets --zip            # plus dist/assets.zip
./.pocket/pocket pack assets --config debug   # sanitized runtime, for checking a pack in CI
./dist/assets/assets                          # opens the window
./dist/assets/assets --headless --frames 600 --json
./dist/assets/assets --serve 4711 --paused    # agents talk to a packed game too
```

The packed runtime is the same executable the repository runs, so every command, report and capture
works unchanged; only the sources are absent. Nothing is signed or notarized: distributing outside a
machine you control needs Apple's tooling on top of this. On Linux the same command packs the Linux
runtime (checked in the container of [the build system](build-system.md), Linux: the packed hello
runs headless on Vulkan); the machine it goes to needs WebKitGTK's JavaScriptCore
(`libjavascriptcoregtk-4.1`) and a Vulkan driver. On Windows it packs `bin/pocket_runtime.exe` with
the DLLs built beside it (`pocket_jsc.dll`, JavaScriptCore; `dxcompiler.dll`, the Windows SDK's
shader compiler) and a `<name>.cmd` launcher; the machine it goes to needs the Visual C++ runtime,
and which DXC a pack for players carries is the owner's call (`docs/development.md`, Current state).
`project/.imported/` is the store's Blender conversions and parsed OBJ meshes
(`docs/design/assets.md`, Formats): a packed game reads those, so a model brought in through Blender
loads without Blender; load each such file once (a run or `assets.reload`) before packing.

`pocket pack <project> --ios` makes an app for the iOS Simulator, `dist/ios/<name>.app`: the runtime
built for the simulator as its executable, the same files as above under `game/`, an `Info.plist`
and an ad hoc signature. `pocket run <project> --ios` packs it, installs it on a simulated iPhone
and starts it, passing what follows `--` (`-- --serve 4711` opens its control server to the Mac);
see [the build system](build-system.md), iOS.

`pocket pack <project> --web` makes the browser version instead: `dist/web/<name>/` with
`index.html`, the wasm runtime and the project packaged into a virtual file system, to host on any
static server. The page exposes the same commands through `window.pocket`; see [the web
build](web.md).
