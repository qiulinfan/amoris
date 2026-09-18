// Error type and result alias used across the engine.
//
// Every fallible engine operation returns pocket::Result<T> (std::expected<T, Error>). Errors carry
// a stable machine-readable code and a human message so they can be surfaced unchanged through
// the CLI (--json), the script host and the agent interface.
#pragma once

#include <expected>
#include <format>
#include <string>
#include <string_view>
#include <utility>

namespace pocket {

struct Error {
    std::string code;     // stable, snake_case, e.g. "file_not_found"
    std::string message;  // human readable, one line
    std::string detail;   // optional multi-line context (stack, compiler output, ...)

    static Error make(std::string_view code, std::string_view message, std::string_view detail = {}) {
        return Error{std::string(code), std::string(message), std::string(detail)};
    }

    template <class... Args>
    static Error fmt(std::string_view code, std::format_string<Args...> fmtstr, Args&&... args) {
        return Error{std::string(code), std::format(fmtstr, std::forward<Args>(args)...), {}};
    }

    [[nodiscard]] std::string to_string() const {
        if (detail.empty()) {
            return std::format("{}: {}", code, message);
        }
        return std::format("{}: {}\n{}", code, message, detail);
    }
};

template <class T>
using Result = std::expected<T, Error>;

using Status = std::expected<void, Error>;

template <class... Args>
[[nodiscard]] inline std::unexpected<Error> fail(std::string_view code, std::format_string<Args...> fmtstr, Args&&... args) {
    return std::unexpected(Error::fmt(code, fmtstr, std::forward<Args>(args)...));
}

[[nodiscard]] inline std::unexpected<Error> fail(Error e) { return std::unexpected(std::move(e)); }

}  // namespace pocket

// Propagate an error from a Result-returning expression inside a Result-returning function.
#define POCKET_TRY(var, expr)                                     \
    auto&& var##_result = (expr);                                 \
    if (!var##_result) return ::pocket::fail(var##_result.error()); \
    auto&& var = *var##_result

#define POCKET_TRY_VOID(expr)                                      \
    do {                                                           \
        auto&& pocket_try_status = (expr);                         \
        if (!pocket_try_status) return ::pocket::fail(pocket_try_status.error()); \
    } while (0)
