// JSON alias. nlohmann::json with ordered objects so that serialized documents are stable and
// diffable (agents compare text; key order must not wobble).
#pragma once

#include <nlohmann/json.hpp>

namespace pocket {
using Json = nlohmann::ordered_json;
}
