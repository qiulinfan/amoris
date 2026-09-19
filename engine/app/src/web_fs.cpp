#ifdef __EMSCRIPTEN__
#include "web_fs.hpp"

#include <emscripten.h>

// clang-format off
EM_JS(void, pocket_web_mount_saves_js, (const char* dir), {
    var path = UTF8ToString(dir);
    Module.__pocket_saves = { ready: false, error: "" };
    try {
        FS.mkdirTree(path);
        FS.mount(IDBFS, {}, path);
        FS.syncfs(true, function (err) {
            if (err) Module.__pocket_saves.error = String(err);
            Module.__pocket_saves.ready = true;
        });
    } catch (e) {
        Module.__pocket_saves.error = String(e);
        Module.__pocket_saves.ready = true;
    }
});
EM_JS(int, pocket_web_saves_ready, (), {
    return Module.__pocket_saves && Module.__pocket_saves.ready ? 1 : 0;
});
EM_JS(int, pocket_web_saves_failed, (), {
    return Module.__pocket_saves && Module.__pocket_saves.error ? 1 : 0;
});
EM_JS(void, pocket_web_sync_saves_js, (), {
    FS.syncfs(false, function (err) { if (err) console.error("pocket: saves not persisted: " + err); });
});
// clang-format on

namespace pocket::app {

bool web_mount_saves(const std::string& dir) {
    pocket_web_mount_saves_js(dir.c_str());
    for (int i = 0; i < 5000 && !pocket_web_saves_ready(); ++i) emscripten_sleep(1);
    return pocket_web_saves_ready() && !pocket_web_saves_failed();
}

void web_sync_saves() { pocket_web_sync_saves_js(); }

}  // namespace pocket::app
#endif
