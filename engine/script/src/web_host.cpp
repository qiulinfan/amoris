// The browser's own JavaScript engine as the script host (the web build). The bundle is
// evaluated on the page; natives are functions on globalThis.__pocket that marshal their
// arguments as JSON into wasm and back; shared typed arrays are views over the wasm heap that
// are recreated on access so memory growth cannot leave them detached.
#ifdef __EMSCRIPTEN__
#include <pocket/script/script_host.hpp>

#include <pocket/core/log.hpp>

#include <emscripten.h>

#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

namespace pocket::script {
namespace {

struct Binding {
    std::string name;
    NativeFn fn;
};

std::vector<Binding>& bindings() {
    static std::vector<Binding> b;
    return b;
}

char* dup(const std::string& s) {
    char* out = static_cast<char*>(std::malloc(s.size() + 1));
    std::memcpy(out, s.c_str(), s.size() + 1);
    return out;
}

}  // namespace
}  // namespace pocket::script

extern "C" {

// JS -> C++: a bound native by index with its arguments as a JSON array. Returns a malloc'd JSON
// {"ok":true,"value":...} or {"ok":false,"error":{...}} that the caller frees.
EMSCRIPTEN_KEEPALIVE char* pocket_native(int index, const char* args_json) {
    using namespace pocket;
    auto& b = script::bindings();
    Json out;
    if (index < 0 || static_cast<std::size_t>(index) >= b.size()) {
        out["ok"] = false;
        out["error"] = Json{{"code", "no_such_native"}, {"message", "unknown native"}};
        return script::dup(out.dump());
    }
    Json args = Json::parse(args_json ? args_json : "[]", nullptr, false);
    if (args.is_discarded()) args = Json::array();
    Result<Json> r = b[static_cast<std::size_t>(index)].fn(args);
    if (r) {
        out["ok"] = true;
        out["value"] = *r;
    } else {
        out["ok"] = false;
        out["error"] = Json{{"code", r.error().code}, {"message", r.error().message}};
    }
    return script::dup(out.dump());
}

}  // extern "C"

EM_JS(void, pocket_web_install, (), {
    globalThis.__pocket = globalThis.__pocket || {};
    globalThis.__pocket_web = {
        bind: function(index, name) {
            globalThis.__pocket[name] = function() {
                var args = JSON.stringify(Array.prototype.slice.call(arguments));
                var ptr = Module.ccall("pocket_native", "number", ["number", "string"], [index, args]);
                var text = UTF8ToString(ptr);
                Module._free(ptr);
                var r = JSON.parse(text);
                if (!r.ok) {
                    var e = new Error(r.error.message);
                    e.code = r.error.code;
                    e.data = r.error;
                    throw e;
                }
                return r.value;
            };
        },
        share: function(name, kind, ptr, count) {
            Object.defineProperty(globalThis.__pocket, name, {
                configurable: true,
                enumerable: true,
                get: function() {
                    var buffer = Module.HEAPU8.buffer;
                    if (kind === 0) return new Float32Array(buffer, ptr, count);
                    if (kind === 1) return new Float64Array(buffer, ptr, count);
                    if (kind === 2) return new Uint32Array(buffer, ptr, count);
                    return new Uint8Array(buffer, ptr, count);
                }
            });
        },
        evaluate: function(source, url) {
            try {
                (0, eval)(source + "\n//# sourceURL=" + url);
                return "";
            } catch (e) {
                return JSON.stringify({ message: String(e && e.message || e), stack: String(e && e.stack || "") });
            }
        },
        call: function(name, argsJson) {
            var f = globalThis[name];
            if (typeof f !== "function") return JSON.stringify({ ok: false, error: { code: "script_function_missing", message: name + " is not a function" } });
            try {
                var args = JSON.parse(argsJson);
                var r = f.apply(null, args);
                return JSON.stringify({ ok: true, value: r === undefined ? null : r });
            } catch (e) {
                return JSON.stringify({ ok: false, error: { code: "script_error", message: String(e && e.message || e), stack: String(e && e.stack || "") } });
            }
        },
        has: function(name) { return typeof globalThis[name] === "function" ? 1 : 0; }
    };
});

EM_JS(void, pocket_web_bind, (int index, const char* name), { globalThis.__pocket_web.bind(index, UTF8ToString(name)); });
EM_JS(void, pocket_web_share, (const char* name, int kind, void* ptr, int count), { globalThis.__pocket_web.share(UTF8ToString(name), kind, ptr, count); });
EM_JS(char*, pocket_web_evaluate, (const char* source, const char* url), { return stringToNewUTF8(globalThis.__pocket_web.evaluate(UTF8ToString(source), UTF8ToString(url))); });
EM_JS(char*, pocket_web_call, (const char* name, const char* args), { return stringToNewUTF8(globalThis.__pocket_web.call(UTF8ToString(name), UTF8ToString(args))); });
EM_JS(int, pocket_web_has, (const char* name), { return globalThis.__pocket_web.has(UTF8ToString(name)); });

namespace pocket::script {
namespace {

std::string take(char* p) {
    std::string s = p ? p : "";
    std::free(p);
    return s;
}

class WebHost final : public ScriptHost {
   public:
    explicit WebHost(const Config&) {
        bindings().clear();
        pocket_web_install();
    }
    std::string engine_name() const override { return "browser"; }
    std::string engine_version() const override { return "page"; }
    Status evaluate(std::string_view source, std::string_view url) override {
        std::string src(source), u(url);
        std::string err = take(pocket_web_evaluate(src.c_str(), u.c_str()));
        if (err.empty()) return {};
        Json j = Json::parse(err, nullptr, false);
        std::string message = j.is_object() ? j.value("message", err) : err;
        std::string stack = j.is_object() ? j.value("stack", "") : "";
        return fail("script_error", "{}{}{}", message, stack.empty() ? "" : "\n", stack);
    }
    void bind(std::string_view name, NativeFn fn) override {
        std::string n(name);
        bindings().push_back({n, std::move(fn)});
        pocket_web_bind(static_cast<int>(bindings().size() - 1), n.c_str());
    }
    // Through the JSON path on the web: the same answers, without the native fast path.
    void bind_numbers(std::string_view name, NumberFn fn) override {
        bind(name, [fn = std::move(fn)](const Json& args) -> Result<Json> {
            double a[8];
            const std::size_t n = std::min<std::size_t>(args.size(), 8);
            for (std::size_t i = 0; i < n; ++i) a[i] = args[i].is_number() ? args[i].get<double>() : std::numeric_limits<double>::quiet_NaN();
            return Json(fn(a, n));
        });
    }
    void share_f32(std::string_view name, float* data, std::size_t count) override { share(name, 0, data, count); }
    void share_f64(std::string_view name, double* data, std::size_t count) override { share(name, 1, data, count); }
    void share_u32(std::string_view name, std::uint32_t* data, std::size_t count) override { share(name, 2, data, count); }
    void share_u8(std::string_view name, std::uint8_t* data, std::size_t count) override { share(name, 3, data, count); }
    Result<Json> call(std::string_view global_function, const Json& args) override {
        Json argv = args.is_array() ? args : (args.is_null() ? Json::array() : Json::array({args}));
        std::string name(global_function);
        std::string dumped = argv.dump();
        std::string text = take(pocket_web_call(name.c_str(), dumped.c_str()));
        Json r = Json::parse(text, nullptr, false);
        if (!r.is_object()) return fail("script_error", "bad result from the page: {}", text);
        if (r.value("ok", false)) return r.value("value", Json());
        const Json& e = r["error"];
        std::string stack = e.value("stack", "");
        return fail(e.value("code", "script_error"), "{}{}{}", e.value("message", ""), stack.empty() ? "" : "\n", stack);
    }
    bool has_function(std::string_view global_function) override {
        std::string name(global_function);
        return pocket_web_has(name.c_str()) != 0;
    }
    void drain_microtasks() override {}   // the page drains its own queue when control returns to it
    void collect_garbage() override {}
    Json describe() const override {
        Json j;
        j["engine"] = engine_name();
        j["jit"] = true;
        return j;
    }

   private:
    void share(std::string_view name, int kind, void* data, std::size_t count) {
        std::string n(name);
        pocket_web_share(n.c_str(), kind, data, static_cast<int>(count));
    }
};

}  // namespace

Result<std::unique_ptr<ScriptHost>> ScriptHost::create(const Config& config) {
    return std::unique_ptr<ScriptHost>(new WebHost(config));
}

}  // namespace pocket::script
#endif  // __EMSCRIPTEN__
