// How every command is called, for agents: its parameters and what it does. `help` answers from
// it, `commands {usage: true}` lists it, and the dispatcher refuses a parameter a command does not
// take (docs/mcp.md, Asking the engine).
#pragma once

#include <pocket/core/json.hpp>

#include <span>
#include <string>
#include <string_view>
#include <vector>

namespace pocket::app {

struct CommandHelp {
    std::string_view name;
    std::string_view params;    // "entity, component, value, cause?": ? optional, a | b alternatives, ... more keys accepted
    std::string_view summary;
};

std::span<const CommandHelp> command_helps();

// The SDK's exports, one line each (generated from sdk/runtime by `pocket gen`, tools/pocket/src/sdkdoc.rs):
// what `help {sdk}` answers.
struct SdkHelp {
    std::string_view name;        // "timer.after", "onTick", "RayHit"
    std::string_view signature;   // as written in the source
    std::string_view doc;         // the first sentence of its doc comment
    std::string_view file;
};
std::span<const SdkHelp> sdk_helps();
const CommandHelp* command_help(std::string_view name);
// The parameter names a signature mentions (alternatives and optional ones included).
std::vector<std::string> command_param_names(std::string_view params);
const std::vector<std::string>& command_param_names(const CommandHelp& help);   // the same, worked out once a command
// Whether a signature accepts keys beyond the ones it names ("...").
bool command_params_open(std::string_view params);
Json command_help_json(const CommandHelp& h);
// Command names close to a mistyped one, nearest first.
std::vector<std::string> command_suggestions(std::string_view name, std::size_t max = 3);

}  // namespace pocket::app
