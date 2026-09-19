#pragma once
// The browser's persistent file system for save slots (docs/web.md). Only built with Emscripten.
#include <string>

namespace pocket::app {
// Mounts an IndexedDB-backed directory at `dir` and loads what it holds; false when the browser
// refused (saves then live for the page's lifetime only).
bool web_mount_saves(const std::string& dir);
// Writes the mounted directory back to IndexedDB (asynchronous, fire and forget).
void web_sync_saves();
}  // namespace pocket::app
