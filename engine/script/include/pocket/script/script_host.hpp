// Script host.
//
// The engine talks to gameplay code through this interface only. The bundle produced by
// `pocket ts` is plain JavaScript; the host evaluates it, exposes a `__pocket` native namespace
// and calls back into global dispatch functions. Data crosses the boundary either as JSON
// (control path, per tick, small) or as shared typed arrays (data path, zero copy).
#pragma once

#include <pocket/core/json.hpp>
#include <pocket/core/result.hpp>

#include <functional>
#include <memory>
#include <string>
#include <string_view>

namespace pocket::script {

struct Config {
    bool inspectable = false;  // allow Safari Web Inspector to attach (JavaScriptCore)
};

// A native function callable from script as __pocket.<name>(...args). `args` is a JSON array.
using NativeFn = std::function<Result<Json>(const Json& args)>;

class ScriptHost {
   public:
    virtual ~ScriptHost() = default;
    static Result<std::unique_ptr<ScriptHost>> create(const Config& config);

    [[nodiscard]] virtual std::string engine_name() const = 0;
    [[nodiscard]] virtual std::string engine_version() const = 0;

    // Evaluate a script. `url` is used in stack traces.
    virtual Status evaluate(std::string_view source, std::string_view url) = 0;
    // Install __pocket.<name>.
    virtual void bind(std::string_view name, NativeFn fn) = 0;
    // Expose engine memory as __pocket.<name> (Float32Array / Uint32Array / Uint8Array). No copy;
    // the memory must outlive the host.
    virtual void share_f32(std::string_view name, float* data, std::size_t count) = 0;
    virtual void share_u32(std::string_view name, std::uint32_t* data, std::size_t count) = 0;
    virtual void share_u8(std::string_view name, std::uint8_t* data, std::size_t count) = 0;
    // Call a global function with JSON arguments; returns its JSON result (undefined -> null).
    virtual Result<Json> call(std::string_view global_function, const Json& args) = 0;
    [[nodiscard]] virtual bool has_function(std::string_view global_function) = 0;
    // Run pending promise jobs.
    virtual void drain_microtasks() = 0;
    virtual void collect_garbage() = 0;
    // Bytes reported by the engine heap, if available.
    [[nodiscard]] virtual Json describe() const = 0;
};

}  // namespace pocket::script
