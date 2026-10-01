// What a command is given for a component, checked against the metadata before it is applied:
// every key a field, every value of the field's type, an i32 field's value names known. The
// generated from_json reads the keys it knows and skips the rest, so without this a misspelt
// field or a name where a number goes would be dropped and the call would look like it worked.
#include <pocket/world/world.hpp>

#include <algorithm>
#include <cctype>
#include <format>
#include <vector>

namespace pocket::world {

namespace {

std::size_t edit_distance(std::string_view a, std::string_view b) {
    std::vector<std::size_t> row(b.size() + 1);
    for (std::size_t j = 0; j <= b.size(); ++j) row[j] = j;
    for (std::size_t i = 1; i <= a.size(); ++i) {
        std::size_t diag = row[0];
        row[0] = i;
        for (std::size_t j = 1; j <= b.size(); ++j) {
            const std::size_t up = row[j];
            const char x = static_cast<char>(std::tolower(static_cast<unsigned char>(a[i - 1])));
            const char y = static_cast<char>(std::tolower(static_cast<unsigned char>(b[j - 1])));
            row[j] = std::min({row[j] + 1, row[j - 1] + 1, diag + (x == y ? 0 : 1)});
            diag = up;
        }
    }
    return row[b.size()];
}

// The closest of `options` to `name`, when close enough to be what was meant.
template <class Range, class Name>
std::string nearest(std::string_view name, const Range& options, Name name_of) {
    std::string best;
    std::size_t best_d = std::max<std::size_t>(2, name.size() / 3) + 1;
    for (const auto& o : options) {
        const std::string_view n = name_of(o);
        const std::size_t d = edit_distance(name, n);
        if (d < best_d) {
            best_d = d;
            best = std::string(n);
        }
    }
    return best;
}

std::string did_you_mean(const std::string& s) { return s.empty() ? std::string() : std::format("; did you mean '{}'?", s); }

std::string_view vector_keys(std::string_view type) {
    if (type == "vec2") return "xy";
    if (type == "vec3") return "xyz";
    if (type == "vec4" || type == "quat") return "xyzw";
    if (type == "color") return "rgba";
    return {};
}

Status check_fields(std::span<const FieldInfo> fields, const Json& patch, const std::string& where);

Status check_value(const FieldInfo& f, const Json& v, const std::string& where) {
    const std::string_view t = f.type;
    if (t == "f32" || t == "f64" || t == "i32" || t == "u32" || t == "i64") {
        if (v.is_number()) return {};
        if (!f.names.empty()) {
            if (v.is_string()) {
                const std::string s = v.get<std::string>();
                if (std::find(f.names.begin(), f.names.end(), s) != f.names.end()) return {};
                std::string list;
                for (std::string_view n : f.names) list += (list.empty() ? "" : ", ") + std::string(n);
                return fail("bad_args", "{} is one of {} (or its number, 0 to {}), not '{}'{}", where, list, f.names.size() - 1, s, did_you_mean(nearest(s, f.names, [](std::string_view n) { return n; })));
            }
            return fail("bad_args", "{} is a number or one of its names, not {}", where, v.dump());
        }
        return fail("bad_args", "{} is a number ({}), not {}", where, t, v.dump());
    }
    if (t == "bool") {
        if (v.is_boolean() || (v.is_number() && (v.get<double>() == 0 || v.get<double>() == 1))) return {};
        return fail("bad_args", "{} is true or false, not {}", where, v.dump());
    }
    if (t == "string") {
        if (v.is_string()) return {};
        return fail("bad_args", "{} is a string, not {}", where, v.dump());
    }
    if (t == "entity") {
        if (v.is_number_unsigned() || (v.is_number() && v.get<double>() >= 0)) return {};
        return fail("bad_args", "{} is an entity id (a number), not {}", where, v.dump());
    }
    if (const std::string_view keys = vector_keys(t); !keys.empty()) {
        if (v.is_array()) {
            if (v.size() > keys.size()) return fail("bad_args", "{} has {} parts ({}), not {}", where, keys.size(), keys, v.size());
            for (const Json& x : v) if (!x.is_number()) return fail("bad_args", "{} is {} numbers, not {}", where, keys.size(), v.dump());
            return {};
        }
        if (v.is_object()) {
            for (const auto& [k, x] : v.items()) {
                if (k.size() != 1 || keys.find(k[0]) == std::string_view::npos) return fail("bad_args", "{} is a {} with the parts {}, not '{}'", where, t, keys, k);
                if (!x.is_number()) return fail("bad_args", "{}.{} is a number, not {}", where, k, x.dump());
            }
            return {};
        }
        if (t == "color" && v.is_string()) {
            Color4 c;
            if (parse_hex_color(v.get<std::string>(), c)) return {};
            return fail("bad_args", "{} is a color: \"#rrggbb\" (or #rgb, #rrggbbaa), {{r, g, b, a}} or an array of numbers, not {}", where, v.dump());
        }
        return fail("bad_args", "{} is a {}: {{{}}}{} or an array of numbers, not {}", where, t, std::string(keys.begin(), keys.end()), t == "color" ? ", \"#rrggbb\"" : "", v.dump());
    }
    if (t.starts_with("list:")) {
        const std::string_view record = t.substr(5);
        const auto records = record_infos();
        const auto it = std::find_if(records.begin(), records.end(), [&](const RecordInfo& r) { return r.name == record; });
        if (!v.is_array()) return fail("bad_args", "{} is a list of {} (an array of objects), not {}", where, record, v.dump());
        if (it == records.end()) return {};
        for (std::size_t i = 0; i < v.size(); ++i) POCKET_TRY_VOID(check_fields(it->fields, v[i], std::format("{}[{}]", where, i)));
        return {};
    }
    return {};
}

Status check_fields(std::span<const FieldInfo> fields, const Json& patch, const std::string& where) {
    if (!patch.is_object()) return fail("bad_args", "{} is an object of fields, not {}", where, patch.dump());
    for (const auto& [k, v] : patch.items()) {
        const auto it = std::find_if(fields.begin(), fields.end(), [&](const FieldInfo& f) { return f.name == k; });
        if (it == fields.end()) {
            return fail("bad_args", "{} has no field '{}'{}", where, k, did_you_mean(nearest(k, fields, [](const FieldInfo& f) { return f.name; })));
        }
        POCKET_TRY_VOID(check_value(*it, v, where + "." + k));
    }
    return {};
}

}  // namespace

Status check_component_patch(std::span<const ComponentInfo> comps, std::string_view component, const Json& patch) {
    const auto it = std::find_if(comps.begin(), comps.end(), [&](const ComponentInfo& c) { return c.name == component; });
    if (it == comps.end()) {
        return fail("unknown_component", "unknown component '{}'{} (world.schema lists them)", component, did_you_mean(nearest(component, comps, [](const ComponentInfo& c) { return c.name; })));
    }
    return check_fields(it->fields, patch, std::string(component));
}

Status check_components(std::span<const ComponentInfo> known, const Json& components) {
    if (components.is_null()) return {};
    if (!components.is_object()) return fail("bad_args", "components is an object of component name -> fields, not {}", components.dump());
    for (const auto& [name, patch] : components.items()) POCKET_TRY_VOID(check_component_patch(known, name, patch));
    return {};
}

Status World::check_patch(std::string_view component, const Json& patch) const { return check_component_patch(component_infos_all(), component, patch); }
Status World::check_components(const Json& components) const { return pocket::world::check_components(component_infos_all(), components); }

bool parse_hex_color(std::string_view text, Color4& out) {
    if (!text.empty() && text.front() == '#') text.remove_prefix(1);
    if (text.size() != 3 && text.size() != 4 && text.size() != 6 && text.size() != 8) return false;
    auto digit = [](char c) -> int {
        if (c >= '0' && c <= '9') return c - '0';
        if (c >= 'a' && c <= 'f') return c - 'a' + 10;
        if (c >= 'A' && c <= 'F') return c - 'A' + 10;
        return -1;
    };
    const bool shorthand = text.size() <= 4;
    const std::size_t channels = shorthand ? text.size() : text.size() / 2;
    float v[4] = {1, 1, 1, 1};
    for (std::size_t i = 0; i < channels; ++i) {
        const int hi = digit(text[shorthand ? i : 2 * i]), lo = digit(text[shorthand ? i : 2 * i + 1]);
        if (hi < 0 || lo < 0) return false;
        v[i] = static_cast<float>(hi * 16 + lo) / 255.0f;
    }
    out = {v[0], v[1], v[2], v[3]};
    return true;
}

}  // namespace pocket::world
