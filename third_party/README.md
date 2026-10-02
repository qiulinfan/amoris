# Third-party code

Vendored sources are compiled as ordinary modules by `pocket`; fetched dependencies are declared in `pocket.toml` and installed under `.pocket/deps/` by `pocket setup`.

| Name | Version | License | How it enters |
|---|---|---|---|
| flecs | 4.1.6 | MIT | vendored amalgamation (`third_party/flecs`) |
| nlohmann/json | 3.12.0 | MIT | vendored single header |
| cpp-httplib | 0.56.0 | MIT | vendored single header |
| stb_image, stb_image_write | master (2026-09) | public domain / MIT | vendored headers; stb_image decodes project images, stb_image_write writes captures |
| stb_vorbis | 1.22 (master, 2026-09) | public domain / MIT | vendored source (`third_party/stb/include/stb_vorbis.c`, compiled in `stb_impl.c`); decodes Ogg Vorbis clips for the audio module |
| dr_mp3 | 0.7.4 (master, 2026-09) | public domain / MIT-0 | vendored header (`third_party/stb/include/dr_mp3.h`, from github.com/mackron/dr_libs, compiled in `stb_impl.c`); decodes MP3 clips for the audio module |
| dr_flac | 0.13.4 (master, 2026-10) | public domain / MIT-0 | vendored header (`third_party/stb/include/dr_flac.h`, from github.com/mackron/dr_libs, compiled in `stb_impl.c`); decodes FLAC clips for the audio module |
| Catch2 | 3.16.0 | BSL-1.0 | vendored amalgamation |
| Yoga | 3.2.1 | MIT | vendored sources |
| Box2D | 3.1.1 | MIT | vendored sources (`third_party/box2d`, the `src` and `include` directories of github.com/erincatto/box2d at tag v3.1.1); 2D rigid bodies, shapes and joints for the physics module (`docs/design/physics2d.md`) |
| nanosvg | master 239e102 (2026-07-09) | zlib | vendored headers (`third_party/stb/include/nanosvg.h`, `nanosvgrast.h`, from github.com/memononen/nanosvg, compiled in `stb_impl.c`); parses and rasterizes SVG images for the assets module |
| meshoptimizer | 1.3 | MIT | vendored sources (`third_party/meshoptimizer/src`, from github.com/zeux/meshoptimizer); simplifies meshes for levels of detail and decodes `EXT_meshopt_compression` in the assets module |
| Basis Universal | 2.50 | Apache-2.0 (zstd: BSD) | vendored transcoder (`third_party/basisu/transcoder`, with zstd's single-file decoder `zstd/zstddeclib.c`, from github.com/BinomialLLC/basis_universal at tag v2_50), compiled as C++20 behind `pocket/pocket_ktx2.h`; reads KTX2 textures |
| libwebp | 1.6.0 | BSD-3-Clause | fetched source, its decoder library built with CMake by `pocket setup`; reads WebP images |
| Draco | 1.5.7 | Apache-2.0 | fetched source, built with CMake by `pocket setup` with only the glTF bitstream; decodes `KHR_draco_mesh_compression` |
| SDL3 | 3.4.16 | Zlib | fetched source, built with CMake by `pocket setup` |
| wgpu-native | 29.0.1.1 | MIT or Apache-2.0 | fetched prebuilt archive |
| FreeType | 2.14.3 | FTL or GPLv2 | fetched source, built with CMake by `pocket setup` (no zlib/png/harfbuzz/brotli) |
| HarfBuzz | 12.3.0 | MIT (Old MIT) | fetched source, built with CMake by `pocket setup` (no FreeType/GLib/ICU; OpenType font functions) |
| Noto Sans Arabic | 2.010 | OFL-1.1 | fetched `file` dependency used by the shaping test |
| Noto Sans CJK SC | 2.004 | OFL 1.1 | fetched as a single file by `pocket setup`; the default UI font (Latin + Chinese, Japanese, Korean) |
| JavaScriptCore | system | Apple system framework | linked as a framework; see ADR 0005 |
| Emscripten | 6.0.9 | MIT / UIUC | the web toolchain (`configs.wasm`), found under `~/.pocket-tools/emsdk`; not fetched by `pocket setup` |
| SDL3 (Emscripten port) | 3.4.2 | Zlib | `--use-port=sdl3` on the web target instead of the fetched source |
| emdawnwebgpu | Emscripten port (Dawn) | BSD-3-Clause | `--use-port=emdawnwebgpu`: the WebGPU C API over the browser's WebGPU on the web target |
