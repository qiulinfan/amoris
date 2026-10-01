#ifndef __EMSCRIPTEN__
// JavaScriptCore backend for ScriptHost.
#include <pocket/script/script_host.hpp>

#include <pocket/core/log.hpp>

#include <JavaScriptCore/JavaScript.h>   // the C API: Apple's framework, or WebKitGTK's JavaScriptCore on Linux

#include <cstring>
#include <limits>
#include <vector>

namespace pocket::script {
namespace {

struct JsString {
    JSStringRef ref;
    explicit JsString(std::string_view s) {
        std::string tmp(s);
        ref = JSStringCreateWithUTF8CString(tmp.c_str());
    }
    // A string already ending in a zero (std::string's c_str): no copy on the way.
    explicit JsString(const std::string& s) : ref(JSStringCreateWithUTF8CString(s.c_str())) {}
    explicit JsString(const char* s) : ref(JSStringCreateWithUTF8CString(s)) {}
    explicit JsString(JSStringRef r) : ref(r) {}
    ~JsString() { if (ref) JSStringRelease(ref); }
    JsString(const JsString&) = delete;
    JsString& operator=(const JsString&) = delete;
    std::string str() const {
        if (!ref) return {};
        std::size_t n = JSStringGetMaximumUTF8CStringSize(ref);
        std::string out(n, '\0');
        std::size_t w = JSStringGetUTF8CString(ref, out.data(), n);
        out.resize(w > 0 ? w - 1 : 0);
        return out;
    }
};

std::string value_to_string(JSContextRef ctx, JSValueRef v) {
    JSValueRef exc = nullptr;
    JSStringRef s = JSValueToStringCopy(ctx, v, &exc);
    if (!s) return "<unprintable>";
    return JsString(s).str();
}

Error exception_to_error(JSContextRef ctx, JSValueRef exc) {
    std::string message = value_to_string(ctx, exc);
    std::string stack;
    if (JSValueIsObject(ctx, exc)) {
        JSObjectRef obj = JSValueToObject(ctx, exc, nullptr);
        JsString key("stack");
        JSValueRef st = JSObjectGetProperty(ctx, obj, key.ref, nullptr);
        if (st && !JSValueIsUndefined(ctx, st)) stack = value_to_string(ctx, st);
        JsString lkey("line");
        JSValueRef line = JSObjectGetProperty(ctx, obj, lkey.ref, nullptr);
        JsString ukey("sourceURL");
        JSValueRef url = JSObjectGetProperty(ctx, obj, ukey.ref, nullptr);
        if (line && JSValueIsNumber(ctx, line)) {
            std::string where = std::format("{}:{}", url && JSValueIsString(ctx, url) ? value_to_string(ctx, url) : "<script>", static_cast<int>(JSValueToNumber(ctx, line, nullptr)));
            stack = stack.empty() ? where : where + "\n" + stack;
        }
    }
    return Error::make("script_error", message, stack);
}

JSValueRef json_to_value(JSContextRef ctx, const Json& j) {
    switch (j.type()) {
        case Json::value_t::null:
        case Json::value_t::discarded: return JSValueMakeNull(ctx);
        case Json::value_t::boolean: return JSValueMakeBoolean(ctx, j.get<bool>());
        case Json::value_t::number_integer: return JSValueMakeNumber(ctx, static_cast<double>(j.get<std::int64_t>()));
        case Json::value_t::number_unsigned: return JSValueMakeNumber(ctx, static_cast<double>(j.get<std::uint64_t>()));
        case Json::value_t::number_float: return JSValueMakeNumber(ctx, j.get<double>());
        case Json::value_t::string: {
            JsString s(j.get_ref<const std::string&>());
            return JSValueMakeString(ctx, s.ref);
        }
        case Json::value_t::array:
        case Json::value_t::object:
        case Json::value_t::binary: {
            std::string text = j.dump();
            JsString s(text);
            JSValueRef v = JSValueMakeFromJSONString(ctx, s.ref);
            return v ? v : JSValueMakeNull(ctx);
        }
    }
    return JSValueMakeNull(ctx);
}

Json value_to_json(JSContextRef ctx, JSValueRef v) {
    if (!v || JSValueIsUndefined(ctx, v) || JSValueIsNull(ctx, v)) return nullptr;
    if (JSValueIsBoolean(ctx, v)) return JSValueToBoolean(ctx, v);
    if (JSValueIsNumber(ctx, v)) {
        double d = JSValueToNumber(ctx, v, nullptr);
        if (d == static_cast<double>(static_cast<std::int64_t>(d)) && std::abs(d) < 9007199254740992.0) return static_cast<std::int64_t>(d);
        return d;
    }
    if (JSValueIsString(ctx, v)) return value_to_string(ctx, v);
    JSValueRef exc = nullptr;
    JSStringRef s = JSValueCreateJSONString(ctx, v, 0, &exc);
    if (!s) return nullptr;
    std::string text = JsString(s).str();
    return Json::parse(text, nullptr, false);
}

struct Binding {
    NativeFn fn;
    std::string name;
    std::function<std::string(std::string)> text;   // console.*: Config::console_text
};

JSValueRef native_trampoline(JSContextRef ctx, JSObjectRef function, JSObjectRef, size_t argc, const JSValueRef argv[], JSValueRef* exception) {
    auto* b = static_cast<Binding*>(JSObjectGetPrivate(function));
    if (!b) return JSValueMakeUndefined(ctx);
    Json args = Json::array();
    args.get_ref<Json::array_t&>().reserve(argc);
    for (size_t i = 0; i < argc; ++i) args.push_back(value_to_json(ctx, argv[i]));
    Result<Json> r = b->fn(args);
    if (!r) {
        JsString msg(std::format("{}: {}", r.error().code, r.error().message));
        JSValueRef m = JSValueMakeString(ctx, msg.ref);
        *exception = JSObjectMakeError(ctx, 1, &m, nullptr);
        return JSValueMakeUndefined(ctx);
    }
    return json_to_value(ctx, *r);
}

void binding_finalize(JSObjectRef obj) {
    delete static_cast<Binding*>(JSObjectGetPrivate(obj));
}

struct NumberBinding {
    NumberFn fn;
};

JSValueRef number_trampoline(JSContextRef ctx, JSObjectRef function, JSObjectRef, size_t argc, const JSValueRef argv[], JSValueRef*) {
    auto* b = static_cast<NumberBinding*>(JSObjectGetPrivate(function));
    if (!b) return JSValueMakeUndefined(ctx);
    double args[8];
    const std::size_t n = std::min<std::size_t>(argc, 8);
    for (std::size_t i = 0; i < n; ++i) args[i] = JSValueIsNumber(ctx, argv[i]) ? JSValueToNumber(ctx, argv[i], nullptr) : std::numeric_limits<double>::quiet_NaN();
    return JSValueMakeNumber(ctx, b->fn(args, n));
}

void number_finalize(JSObjectRef obj) {
    delete static_cast<NumberBinding*>(JSObjectGetPrivate(obj));
}

JSClassRef number_class() {
    static JSClassRef cls = [] {
        JSClassDefinition def = kJSClassDefinitionEmpty;
        def.className = "PocketNumbers";
        def.callAsFunction = number_trampoline;
        def.finalize = number_finalize;
        return JSClassCreate(&def);
    }();
    return cls;
}

JSClassRef binding_class() {
    static JSClassRef cls = [] {
        JSClassDefinition def = kJSClassDefinitionEmpty;
        def.className = "PocketNative";
        def.callAsFunction = native_trampoline;
        def.finalize = binding_finalize;
        return JSClassCreate(&def);
    }();
    return cls;
}

JSValueRef console_trampoline(JSContextRef ctx, JSObjectRef function, JSObjectRef, size_t argc, const JSValueRef argv[], JSValueRef*) {
    auto* b = static_cast<Binding*>(JSObjectGetPrivate(function));
    std::string line;
    for (size_t i = 0; i < argc; ++i) {
        if (i) line += ' ';
        if (JSValueIsString(ctx, argv[i])) {
            line += value_to_string(ctx, argv[i]);
        } else {
            Json j = value_to_json(ctx, argv[i]);
            line += j.is_string() ? j.get<std::string>() : j.dump();
        }
    }
    if (b && b->text) line = b->text(std::move(line));
    log::Level level = log::Level::Info;
    if (b) {
        if (b->name == "warn") level = log::Level::Warn;
        else if (b->name == "error") level = log::Level::Error;
        else if (b->name == "debug") level = log::Level::Debug;
    }
    log::global().emit(level, "script", line);
    return JSValueMakeUndefined(ctx);
}

JSClassRef console_class() {
    static JSClassRef cls = [] {
        JSClassDefinition def = kJSClassDefinitionEmpty;
        def.className = "PocketConsole";
        def.callAsFunction = console_trampoline;
        def.finalize = binding_finalize;
        return JSClassCreate(&def);
    }();
    return cls;
}

class JscHost final : public ScriptHost {
   public:
    explicit JscHost(const Config& config) {
        group_ = JSContextGroupCreate();
        ctx_ = JSGlobalContextCreateInGroup(group_, nullptr);
        JsString name("pocket");
        JSGlobalContextSetName(ctx_, name.ref);
        if (config.inspectable) JSGlobalContextSetInspectable(ctx_, true);
        JSObjectRef global = JSContextGetGlobalObject(ctx_);
        native_ = JSObjectMake(ctx_, nullptr, nullptr);
        JSValueProtect(ctx_, native_);
        set_property(global, "__pocket", native_);
        // console.*
        JSObjectRef console = JSObjectMake(ctx_, nullptr, nullptr);
        for (const char* level : {"log", "info", "warn", "error", "debug"}) {
            auto* b = new Binding{nullptr, level, config.console_text};
            JSObjectRef fn = JSObjectMake(ctx_, console_class(), b);
            set_property(console, level, fn);
        }
        set_property(global, "console", console);
        set_property(global, "globalThis", global);
    }

    ~JscHost() override {
        JSValueUnprotect(ctx_, native_);
        JSGlobalContextRelease(ctx_);
        JSContextGroupRelease(group_);
    }

    std::string engine_name() const override { return "JavaScriptCore"; }
    std::string engine_version() const override { return "system"; }

    Status evaluate(std::string_view source, std::string_view url) override {
        JsString src(source);
        JsString u(url);
        JSValueRef exc = nullptr;
        JSEvaluateScript(ctx_, src.ref, nullptr, u.ref, 1, &exc);
        if (exc) return fail(exception_to_error(ctx_, exc));
        return {};
    }

    void bind(std::string_view name, NativeFn fn) override {
        auto* b = new Binding{std::move(fn), std::string(name)};
        JSObjectRef f = JSObjectMake(ctx_, binding_class(), b);
        set_property(native_, name, f);
    }
    void bind_numbers(std::string_view name, NumberFn fn) override {
        JSObjectRef f = JSObjectMake(ctx_, number_class(), new NumberBinding{std::move(fn)});
        set_property(native_, name, f);
    }

    void share_f32(std::string_view name, float* data, std::size_t count) override {
        share(name, kJSTypedArrayTypeFloat32Array, data, count * sizeof(float));
    }
    void share_f64(std::string_view name, double* data, std::size_t count) override {
        share(name, kJSTypedArrayTypeFloat64Array, data, count * sizeof(double));
    }
    void share_u32(std::string_view name, std::uint32_t* data, std::size_t count) override {
        share(name, kJSTypedArrayTypeUint32Array, data, count * sizeof(std::uint32_t));
    }
    void share_u8(std::string_view name, std::uint8_t* data, std::size_t count) override {
        share(name, kJSTypedArrayTypeUint8Array, data, count);
    }

    Result<Json> call(std::string_view global_function, const Json& args) override {
        JSObjectRef global = JSContextGetGlobalObject(ctx_);
        JsString key(global_function);
        JSValueRef fv = JSObjectGetProperty(ctx_, global, key.ref, nullptr);
        if (!fv || !JSValueIsObject(ctx_, fv)) return fail("script_function_missing", "global function '{}' is not defined", global_function);
        JSObjectRef fn = JSValueToObject(ctx_, fv, nullptr);
        if (!JSObjectIsFunction(ctx_, fn)) return fail("script_function_missing", "'{}' is not a function", global_function);
        // The arguments live in a heap vector, where the collector's stack scan does not see them:
        // each is protected until the call returns, or making a later one (a large object parsed
        // from JSON) can collect an earlier one (the dispatch kind) out from under the call.
        std::vector<JSValueRef> argv;
        auto keep = [&](JSValueRef v) { JSValueProtect(ctx_, v); argv.push_back(v); };
        if (args.is_array()) {
            for (const auto& a : args) keep(json_to_value(ctx_, a));
        } else if (!args.is_null()) {
            keep(json_to_value(ctx_, args));
        }
        JSValueRef exc = nullptr;
        JSValueRef r = JSObjectCallAsFunction(ctx_, fn, nullptr, argv.size(), argv.empty() ? nullptr : argv.data(), &exc);
        for (JSValueRef v : argv) JSValueUnprotect(ctx_, v);
        if (exc) return fail(exception_to_error(ctx_, exc));
        return value_to_json(ctx_, r);
    }

    bool has_function(std::string_view global_function) override {
        JSObjectRef global = JSContextGetGlobalObject(ctx_);
        JsString key(global_function);
        JSValueRef fv = JSObjectGetProperty(ctx_, global, key.ref, nullptr);
        return fv && JSValueIsObject(ctx_, fv) && JSObjectIsFunction(ctx_, JSValueToObject(ctx_, fv, nullptr));
    }

    void drain_microtasks() override {
        // JavaScriptCore drains the microtask queue when the outermost API call returns; an empty
        // evaluation is enough to flush jobs queued by native callbacks.
        JsString src("void 0");
        JSEvaluateScript(ctx_, src.ref, nullptr, nullptr, 1, nullptr);
    }

    void collect_garbage() override { JSGarbageCollect(ctx_); }

    Json describe() const override {
        Json j;
        j["engine"] = engine_name();
        j["jit"] = true;
        return j;
    }

   private:
    void set_property(JSObjectRef obj, std::string_view name, JSValueRef value) {
        JsString key(name);
        JSObjectSetProperty(ctx_, obj, key.ref, value, kJSPropertyAttributeNone, nullptr);
    }
    void share(std::string_view name, JSTypedArrayType type, void* data, std::size_t bytes) {
        JSValueRef exc = nullptr;
        JSObjectRef arr = JSObjectMakeTypedArrayWithBytesNoCopy(ctx_, type, data, bytes, nullptr, nullptr, &exc);
        if (!arr) {
            log::error("script", "failed to share buffer {}", name);
            return;
        }
        set_property(native_, name, arr);
    }

    JSContextGroupRef group_ = nullptr;
    JSGlobalContextRef ctx_ = nullptr;
    JSObjectRef native_ = nullptr;
};

}  // namespace

Result<std::unique_ptr<ScriptHost>> ScriptHost::create(const Config& config) {
    return std::unique_ptr<ScriptHost>(new JscHost(config));
}

}  // namespace pocket::script
#endif  // __EMSCRIPTEN__
