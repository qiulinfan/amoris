# Third-party code

Vendored sources are compiled as ordinary modules by `pocket`; fetched dependencies are declared in `pocket.toml` and installed under `.pocket/deps/` by `pocket setup`.

| Name | Version | License | How it enters |
|---|---|---|---|
| flecs | 4.1.6 | MIT | vendored amalgamation (`third_party/flecs`) |
| nlohmann/json | 3.12.0 | MIT | vendored single header |
| cpp-httplib | 0.56.0 | MIT | vendored single header |
| stb_image, stb_image_write | master (2026-09) | public domain / MIT | vendored headers |
| Catch2 | 3.16.0 | BSL-1.0 | vendored amalgamation |
| Yoga | 3.2.1 | MIT | vendored sources |
| SDL3 | 3.4.16 | Zlib | fetched source, built with CMake by `pocket setup` |
| wgpu-native | 29.0.1.1 | MIT or Apache-2.0 | fetched prebuilt archive |
| FreeType | 2.14.3 | FTL or GPLv2 | fetched source, built with CMake by `pocket setup` (no zlib/png/harfbuzz/brotli) |
| HarfBuzz | 14.4.0 | MIT (Old MIT) | pinned, not yet fetched: text uses FreeType advances until shaping lands |
| Noto Sans CJK SC | 2.004 | OFL 1.1 | fetched as a single file by `pocket setup`; the default UI font (Latin + Chinese, Japanese, Korean) |
| JavaScriptCore | system | Apple system framework | linked as a framework; see ADR 0005 |
