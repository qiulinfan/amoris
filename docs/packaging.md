# Packaging

`pocket pack <project>` turns a project into a folder that runs on another Mac without the repository, the toolchain or the TypeScript sources:

```
dist/<name>/
  <name>                launcher: exec bin/pocket_runtime with the paths below; extra arguments pass through
  bin/pocket_runtime    the engine (release configuration by default; SDL3, wgpu-native and FreeType are linked statically)
  project.js            the game's scripts, bundled by `pocket ts`
  project.json          settings from project.toml plus the font path
  project/              project.toml, the scene and assets/ (scripts/ is left out)
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

The packed runtime is the same executable the repository runs, so every command, report and capture works unchanged; only the sources are absent. Nothing is signed or notarized: distributing outside a machine you control needs Apple's tooling on top of this. Windows and Linux packs arrive with those platforms (V8, ADR 0005).
