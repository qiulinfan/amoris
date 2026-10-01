#include <pocket/assets/assets.hpp>

#include <pocket/core/fs.hpp>
#include <pocket/core/log.hpp>
#include <pocket/core/tangents.hpp>

#include <stb_image.h>

#include <algorithm>
#include <cmath>
#include <cstring>
#include <functional>
#include <limits>
#include <optional>
#include <set>

namespace pocket::assets {

Json Mesh::describe() const {
    Json j;
    j["path"] = path;
    j["vertices"] = vertices.size();
    j["triangles"] = indices.size() / 3;
    j["submeshes"] = submeshes.size();
    j["nodes"] = node_count;
    j["importer"] = importer;
    if (!converted.empty()) j["converted"] = converted;
    if (!lights.empty()) {
        Json lj = Json::array();
        for (const LightDef& l : lights) lj.push_back(Json{{"name", l.name}, {"type", l.type == 0 ? "directional" : l.type == 2 ? "spot" : "point"}, {"intensity", l.intensity}});
        j["lights"] = lj;
    }
    if (!cameras.empty()) {
        Json cj = Json::array();
        for (const CameraDef& c : cameras) cj.push_back(Json{{"name", c.name}, {"orthographic", c.orthographic}});
        j["cameras"] = cj;
    }
    Json mats = Json::array();
    for (const auto& m : materials) {
        Json mj;
        mj["name"] = m.name;
        mj["base_color"] = Json::array({m.base_color.x, m.base_color.y, m.base_color.z, m.base_color.w});
        if (!m.texture.empty()) mj["texture"] = m.texture;
        mj["metallic"] = m.metallic;
        mj["roughness"] = m.roughness;
        if (!m.metallic_roughness_texture.empty()) mj["metallic_roughness_texture"] = m.metallic_roughness_texture;
        if (!m.normal_texture.empty()) {
            mj["normal_texture"] = m.normal_texture;
            mj["normal_scale"] = m.normal_scale;
        }
        if (!m.emissive_texture.empty()) mj["emissive_texture"] = m.emissive_texture;
        if (m.emissive.x > 0 || m.emissive.y > 0 || m.emissive.z > 0) mj["emissive"] = Json{{"r", m.emissive.x}, {"g", m.emissive.y}, {"b", m.emissive.z}};
        if (m.double_sided) mj["double_sided"] = true;
        if (m.blend) mj["blend"] = true;
        if (m.alpha_cutoff > 0) mj["cutoff"] = m.alpha_cutoff;
        if (m.uv_transformed) mj["uv_transform"] = Json{{"offset", Json::array({m.uv_offset.x, m.uv_offset.y})}, {"scale", Json::array({m.uv_scale.x, m.uv_scale.y})}};
        if (m.transmission > 0) {
            mj["transmission"] = m.transmission;
            mj["ior"] = m.ior;
            if (m.thickness > 0) mj["thickness"] = m.thickness;
            if (m.attenuation_distance > 0) mj["attenuation"] = Json{{"color", Json::array({m.attenuation_color.x, m.attenuation_color.y, m.attenuation_color.z})}, {"distance", m.attenuation_distance}};
        }
        if (m.clearcoat > 0) {
            mj["clearcoat"] = m.clearcoat;
            mj["clearcoat_roughness"] = m.clearcoat_roughness;
        }
        if (m.sheen_color.x > 0 || m.sheen_color.y > 0 || m.sheen_color.z > 0) mj["sheen"] = Json{{"color", Json::array({m.sheen_color.x, m.sheen_color.y, m.sheen_color.z})}, {"roughness", m.sheen_roughness}};
        if (m.specular != 1 || m.specular_color.x != 1 || m.specular_color.y != 1 || m.specular_color.z != 1) mj["specular"] = Json{{"factor", m.specular}, {"color", Json::array({m.specular_color.x, m.specular_color.y, m.specular_color.z})}};
        if (m.anisotropy > 0) mj["anisotropy"] = Json{{"strength", m.anisotropy}, {"rotation", m.anisotropy_rotation}};
        if (m.unlit) mj["unlit"] = true;
        mats.push_back(mj);
    }
    j["materials"] = mats;
    j["aabb"] = Json{{"min", Json::array({aabb_min.x, aabb_min.y, aabb_min.z})}, {"max", Json::array({aabb_max.x, aabb_max.y, aabb_max.z})}};
    // The nodes that carry geometry, by name (or index when unnamed), in node order: what MeshRenderer.node takes.
    Json parts = Json::array();
    std::vector<bool> seen(nodes.size(), false);
    for (const Submesh& sm : submeshes) {
        if (sm.origin < 0 || static_cast<std::size_t>(sm.origin) >= nodes.size() || seen[static_cast<std::size_t>(sm.origin)]) continue;
        seen[static_cast<std::size_t>(sm.origin)] = true;
        const std::string& name = nodes[static_cast<std::size_t>(sm.origin)].name;
        parts.push_back(name.empty() ? std::to_string(sm.origin) : name);
    }
    j["parts"] = parts;
    // Nodes' custom properties (glTF extras, as Blender writes an object's), under the names
    // world.instantiate gives the nodes' entities: the node's name when no other node has it, else
    // the name and the node's index (node<index> for an unnamed one).
    std::map<std::string, int> name_count;
    for (const Node& n : nodes) name_count[n.name]++;
    Json properties = Json::object();
    for (std::size_t i = 0; i < nodes.size(); ++i) {
        const Node& n = nodes[i];
        if (!n.extras.is_object()) continue;
        const std::string key = n.name.empty() ? "node" + std::to_string(i) : name_count[n.name] == 1 ? n.name : n.name + "_" + std::to_string(i);
        properties[key] = n.extras;
    }
    if (!properties.empty()) j["properties"] = properties;
    j["moving_parts"] = moving_parts();
    j["skinned"] = skinned();
    j["vertex_colors"] = vertex_colors;
    j["tangents"] = file_tangents ? "file" : "uv";
    Json sk = Json::array();
    for (const auto& s : skins) sk.push_back(Json{{"name", s.name}, {"joints", s.joints.size()}});
    j["skins"] = sk;
    Json an = Json::array();
    for (const auto& a : animations) an.push_back(Json{{"name", a.name}, {"duration", a.duration}, {"channels", a.channels.size()}});
    j["animations"] = an;
    Json targets = Json::array();
    for (const auto& t : morph_targets) targets.push_back(t.name);
    j["targets"] = targets;
    if (!default_weights.empty()) j["default_weights"] = default_weights;
    return j;
}

int Mesh::morph_target(std::string_view name) const {
    for (std::size_t i = 0; i < morph_targets.size(); ++i) if (morph_targets[i].name == name) return static_cast<int>(i);
    if (!name.empty() && std::all_of(name.begin(), name.end(), [](char c) { return c >= '0' && c <= '9'; })) {
        const int i = std::atoi(std::string(name).c_str());
        if (i >= 0 && static_cast<std::size_t>(i) < morph_targets.size()) return i;
    }
    return -1;
}

int Mesh::node_index(std::string_view name) const {
    for (std::size_t i = 0; i < nodes.size(); ++i) if (!nodes[i].name.empty() && nodes[i].name == name) return static_cast<int>(i);
    if (!name.empty() && std::all_of(name.begin(), name.end(), [](char c) { return c >= '0' && c <= '9'; })) {
        const int i = std::atoi(std::string(name).c_str());
        if (i >= 0 && static_cast<std::size_t>(i) < nodes.size()) return i;
    }
    return -1;
}

Mat4 Mesh::rest_global(int node) const {
    Mat4 g = Mat4::identity();
    if (node < 0 || static_cast<std::size_t>(node) >= nodes.size()) return g;
    std::vector<std::size_t> lineage;
    for (int n = node, guard = 0; n >= 0 && static_cast<std::size_t>(n) < nodes.size() && guard < 256; n = nodes[static_cast<std::size_t>(n)].parent, ++guard) lineage.push_back(static_cast<std::size_t>(n));
    for (auto it = lineage.rbegin(); it != lineage.rend(); ++it) g = g * nodes[*it].rest;
    return g;
}

std::size_t Mesh::moving_parts() const {
    std::size_t n = 0;
    for (const Submesh& sm : submeshes) n += sm.node >= 0;
    return n;
}

const AnimationClip* Mesh::clip(std::string_view name) const {
    for (const auto& a : animations) {
        if (a.name == name) return &a;
    }
    return nullptr;
}

namespace {

Json properties_of(const Json& j) {
    // Tiled writes properties as [{name, type, value}]; keep them as {name: value}.
    Json out = Json::object();
    if (j.contains("properties") && j["properties"].is_array()) {
        for (const Json& p : j["properties"]) {
            if (p.is_object() && p.contains("name")) out[p["name"].get<std::string>()] = p.value("value", Json(nullptr));
        }
    }
    return out;
}

bool truthy(const Json& props, const char* key) {
    if (!props.is_object() || !props.contains(key)) return false;
    const Json& v = props[key];
    return (v.is_boolean() && v.get<bool>()) || (v.is_number() && v.get<double>() != 0) || (v.is_string() && (v == "true" || v == "1"));
}

}  // namespace

int TileSet::frame_at(int local_id, std::uint64_t time_ms) const {
    const auto it = animations.find(local_id);
    if (it == animations.end()) return local_id;
    auto t = static_cast<int>(time_ms % static_cast<std::uint64_t>(it->second.total_ms));
    for (const auto& [id, ms] : it->second.frames) {
        if (t < ms) return id;
        t -= ms;
    }
    return it->second.frames.back().first;
}

bool TileSet::solid(int local_id) const {
    auto it = tile_properties.find(local_id);
    if (it != tile_properties.end() && truthy(it->second, "solid")) return true;
    return shapes.contains(local_id) && !one_way(local_id);   // drawn collision shapes make a tile solid inside them
}

const std::vector<TileSet::Shape>* TileSet::shapes_of(int local_id) const {
    auto it = shapes.find(local_id);
    return it == shapes.end() ? nullptr : &it->second;
}

bool TileSet::one_way(int local_id) const {
    auto it = tile_properties.find(local_id);
    return it != tile_properties.end() && truthy(it->second, "one_way");
}

int TileSet::slope(int local_id) const {
    auto it = tile_properties.find(local_id);
    if (it == tile_properties.end() || !it->second.is_object() || !it->second.contains("slope")) return 0;
    const Json& v = it->second["slope"];
    if (v.is_number()) return v.get<double>() > 0 ? 1 : v.get<double>() < 0 ? -1 : 0;
    if (v.is_string()) return v.get<std::string>() == "right" ? 1 : v.get<std::string>() == "left" ? -1 : 0;
    return 0;
}

bool TileLayer::solid_layer() const { return truthy(properties, "solid"); }

const TileSet* TileMap::tileset_for(std::uint32_t gid) const {
    const std::uint32_t id = gid & kIdMask;
    const TileSet* best = nullptr;
    for (const TileSet& ts : tilesets) {
        if (id >= ts.first_gid && (!best || ts.first_gid > best->first_gid)) best = &ts;
    }
    return best;
}

const TileLayer* TileMap::layer(std::string_view name) const {
    for (const TileLayer& l : layers) {
        if (l.name == name) return &l;
    }
    return nullptr;
}

TileLayer* TileMap::layer_mut(std::string_view name) {
    for (TileLayer& l : layers) if (l.name == name) return &l;
    return nullptr;
}

Result<std::uint32_t> TileMap::set(std::string_view layer_name, int x, int y, std::uint32_t gid) {
    TileLayer* l = layer_mut(layer_name);
    if (!l) {
        std::string names;
        for (const TileLayer& other : layers) names += (names.empty() ? "" : ", ") + other.name;
        return fail("unknown_layer", "{} has no tile layer '{}' (layers: {})", path, layer_name, names);
    }
    if (x < 0 || y < 0 || x >= l->width || y >= l->height) return fail("out_of_map", "tile ({}, {}) is outside layer '{}' ({}x{})", x, y, l->name, l->width, l->height);
    if (gid != 0) {
        const TileSet* ts = tileset_for(gid);
        const std::uint32_t local = ts ? (gid & kIdMask) - ts->first_gid : 0;
        if (!ts || (ts->tile_count > 0 && local >= static_cast<std::uint32_t>(ts->tile_count))) return fail("bad_gid", "gid {} is not a tile of any tileset of {}", gid & kIdMask, path);
    }
    std::uint32_t& cell = l->gids[static_cast<std::size_t>(y) * static_cast<std::size_t>(l->width) + static_cast<std::size_t>(x)];
    const std::uint32_t was = cell;
    if (was != gid) {
        cell = gid;
        ++revision;
        ++l->revision;
    }
    return was;
}

namespace {

// {name: value} back to Tiled's property list.
Json tiled_properties(const Json& props) {
    Json out = Json::array();
    if (!props.is_object()) return out;
    for (auto it = props.begin(); it != props.end(); ++it) {
        const Json& v = it.value();
        const char* type = v.is_boolean() ? "bool" : v.is_number_integer() ? "int" : v.is_number() ? "float" : "string";
        out.push_back(Json{{"name", it.key()}, {"type", type}, {"value", v.is_string() || v.is_number() || v.is_boolean() ? v : Json(v.dump())}});
    }
    return out;
}

}  // namespace

Result<TileLayer*> TileMap::add_layer(const std::string& name, bool visible, float opacity, Json properties) {
    if (name.empty()) return fail("bad_args", "a layer needs a name");
    if (layer(name)) return fail("duplicate_layer", "{} already has a tile layer '{}'", path, name);
    TileLayer l;
    l.name = name;
    int next_id = 1;
    for (const TileLayer& other : layers) next_id = std::max(next_id, other.id + 1);
    if (source.is_object() && source.contains("nextlayerid") && source["nextlayerid"].is_number()) next_id = std::max(next_id, source["nextlayerid"].get<int>());
    l.id = next_id;
    l.width = width;
    l.height = height;
    l.gids.assign(static_cast<std::size_t>(width) * static_cast<std::size_t>(height), 0u);
    l.visible = visible;
    l.opacity = std::clamp(opacity, 0.0f, 1.0f);
    l.properties = properties.is_object() ? std::move(properties) : Json::object();
    l.revision = 1;
    ++revision;
    if (source.is_object()) {
        Json lj{{"id", l.id}, {"type", "tilelayer"}, {"name", l.name}, {"width", l.width}, {"height", l.height}, {"x", 0}, {"y", 0}, {"opacity", l.opacity}, {"visible", l.visible}, {"data", Json::array()}};
        if (!l.properties.empty()) lj["properties"] = tiled_properties(l.properties);
        if (!source.contains("layers") || !source["layers"].is_array()) source["layers"] = Json::array();
        source["layers"].push_back(std::move(lj));
        source["nextlayerid"] = l.id + 1;
    }
    layers.push_back(std::move(l));
    return &layers.back();
}

Status TileMap::remove_layer(std::string_view name) {
    std::size_t k = 0;
    for (; k < layers.size(); ++k) if (layers[k].name == name) break;
    if (k == layers.size()) return fail("unknown_layer", "{} has no tile layer '{}'", path, name);
    layers.erase(layers.begin() + static_cast<std::ptrdiff_t>(k));
    for (ImageLayer& il : image_layers) if (il.before > static_cast<int>(k)) --il.before;   // the pictures keep their place among the rest
    ++revision;
    if (source.is_object() && source.contains("layers") && source["layers"].is_array()) {
        // The k-th tile layer of the document, in document order with groups flattened.
        std::size_t seen = 0;
        std::function<bool(Json&)> remove_kth = [&](Json& list) -> bool {
            for (auto it = list.begin(); it != list.end(); ++it) {
                if (!it->is_object()) continue;
                const std::string type = it->value("type", "tilelayer");
                if (type == "group") {
                    if (it->contains("layers") && (*it)["layers"].is_array() && remove_kth((*it)["layers"])) return true;
                    continue;
                }
                if (type != "tilelayer") continue;
                if (seen++ == k) {
                    list.erase(it);
                    return true;
                }
            }
            return false;
        };
        remove_kth(source["layers"]);
    }
    return {};
}

Status TileMap::move_layer(std::string_view name, std::size_t index) {
    std::size_t k = 0;
    for (; k < layers.size(); ++k) if (layers[k].name == name) break;
    if (k == layers.size()) return fail("unknown_layer", "{} has no tile layer '{}'", path, name);
    if (index >= layers.size()) return fail("bad_args", "index {} is outside the {} tile layers of {}", index, layers.size(), path);
    if (index == k) return {};
    TileLayer moving = std::move(layers[k]);
    layers.erase(layers.begin() + static_cast<std::ptrdiff_t>(k));
    layers.insert(layers.begin() + static_cast<std::ptrdiff_t>(index), std::move(moving));
    for (ImageLayer& il : image_layers) {
        // A picture keeps its place among the tile layers around it: one leaving the set before it
        // or joining it moves the count by one.
        if (static_cast<int>(k) < il.before && static_cast<int>(index) >= il.before) --il.before;
        else if (static_cast<int>(k) >= il.before && static_cast<int>(index) < il.before) ++il.before;
    }
    ++revision;
    for (TileLayer& l : layers) ++l.revision;   // every layer's draw order moved: the meshes are keyed by it
    if (source.is_object() && source.contains("layers") && source["layers"].is_array()) {
        // The tile layer nodes in document order, groups flattened: each as the list holding it
        // and its position there. The moved node leaves its list and goes before the node now at
        // the target position, in that node's list (so a layer moved to where a group's layer is
        // joins the group), or after the last tile layer when the target is the end.
        using Slot = std::pair<Json*, std::size_t>;
        std::vector<Slot> nodes;
        std::function<void(Json&)> collect = [&](Json& list) {
            for (std::size_t i = 0; i < list.size(); ++i) {
                if (!list[i].is_object()) continue;
                const std::string type = list[i].value("type", "tilelayer");
                if (type == "group") {
                    if (list[i].contains("layers") && list[i]["layers"].is_array()) collect(list[i]["layers"]);
                } else if (type == "tilelayer") {
                    nodes.emplace_back(&list, i);
                }
            }
        };
        collect(source["layers"]);
        if (k < nodes.size()) {
            Json node = (*nodes[k].first)[nodes[k].second];
            nodes[k].first->erase(nodes[k].first->begin() + static_cast<std::ptrdiff_t>(nodes[k].second));
            nodes.clear();
            collect(source["layers"]);
            if (index < nodes.size()) nodes[index].first->insert(nodes[index].first->begin() + static_cast<std::ptrdiff_t>(nodes[index].second), std::move(node));
            else if (!nodes.empty()) nodes.back().first->insert(nodes.back().first->begin() + static_cast<std::ptrdiff_t>(nodes.back().second + 1), std::move(node));
            else source["layers"].push_back(std::move(node));
        }
    }
    return {};
}

Result<TileSet*> TileMap::add_tileset(TileSet set) {
    if (set.name.empty()) return fail("bad_args", "a tileset needs a name");
    for (const TileSet& t : tilesets) if (t.name == set.name) return fail("duplicate_tileset", "{} already has a tileset '{}'", path, set.name);
    if (set.tile_width <= 0 || set.tile_height <= 0 || set.columns <= 0 || set.tile_count <= 0) return fail("bad_args", "a tileset needs positive tile_width, tile_height, columns and tile_count");
    std::uint32_t first = 1;
    for (const TileSet& t : tilesets) first = std::max(first, t.first_gid + static_cast<std::uint32_t>(std::max(t.tile_count, 1)));
    set.first_gid = first;
    ++revision;
    if (source.is_object()) {
        const std::filesystem::path base = std::filesystem::path(path).parent_path();
        std::string image = std::filesystem::path(set.image).lexically_relative(base).generic_string();
        if (image.empty()) image = std::filesystem::path(set.image).filename().generic_string();
        Json tj{{"name", set.name}, {"firstgid", set.first_gid}, {"image", image}, {"tilewidth", set.tile_width}, {"tileheight", set.tile_height}, {"columns", set.columns}, {"tilecount", set.tile_count}, {"imagewidth", set.image_width}, {"imageheight", set.image_height}, {"spacing", set.spacing}, {"margin", set.margin}};
        Json tiles = Json::array();
        for (const auto& [tid, props] : set.tile_properties) tiles.push_back(Json{{"id", tid}, {"properties", tiled_properties(props)}});
        if (!tiles.empty()) tj["tiles"] = tiles;
        if (!source.contains("tilesets") || !source["tilesets"].is_array()) source["tilesets"] = Json::array();
        source["tilesets"].push_back(std::move(tj));
    }
    tilesets.push_back(std::move(set));
    return &tilesets.back();
}

Status TileMap::remove_tileset(std::string_view name) {
    std::size_t k = 0;
    for (; k < tilesets.size(); ++k) if (tilesets[k].name == name) break;
    if (k == tilesets.size()) return fail("unknown_tileset", "{} has no tileset '{}'", path, name);
    const TileSet& set = tilesets[k];
    const std::uint32_t first = set.first_gid, last = set.first_gid + static_cast<std::uint32_t>(std::max(set.tile_count, 1)) - 1;
    for (const TileLayer& l : layers) {
        std::size_t used = 0;
        for (std::uint32_t g : l.gids) {
            const std::uint32_t id = g & kIdMask;
            used += id >= first && id <= last;
        }
        if (used > 0) return fail("tileset_in_use", "{} tiles of tileset '{}' are on layer '{}' of {}; clear them first", used, name, l.name, path);
    }
    tilesets.erase(tilesets.begin() + static_cast<std::ptrdiff_t>(k));
    ++revision;
    if (source.is_object() && source.contains("tilesets") && source["tilesets"].is_array()) {
        Json& list = source["tilesets"];
        for (auto it = list.begin(); it != list.end(); ++it) {
            if (it->is_object() && it->value("firstgid", 0u) == first && it->value("name", "") == name) {
                list.erase(it);
                break;
            }
        }
    }
    return {};
}

Json TileMap::to_json() const {
    Json doc = source.is_object() ? source : Json::object();
    if (!source.is_object()) {
        doc["type"] = "map";
        doc["version"] = "1.10";
        doc["orientation"] = orientation;
        if (orientation == "hexagonal") doc["hexsidelength"] = hex_side;
        if (orientation == "hexagonal" || orientation == "staggered") {
            doc["staggeraxis"] = stagger_y ? "y" : "x";
            doc["staggerindex"] = stagger_odd ? "odd" : "even";
        }
        doc["renderorder"] = "right-down";
        doc["infinite"] = false;
        doc["width"] = width;
        doc["height"] = height;
        doc["tilewidth"] = tile_width;
        doc["tileheight"] = tile_height;
        if (properties.is_object() && !properties.empty()) doc["properties"] = tiled_properties(properties);
        Json sets = Json::array();
        for (const TileSet& t : tilesets) {
            Json tj{{"name", t.name}, {"firstgid", t.first_gid}, {"image", std::filesystem::path(t.image).filename().generic_string()}, {"tilewidth", t.tile_width}, {"tileheight", t.tile_height}, {"columns", t.columns}, {"tilecount", t.tile_count}, {"imagewidth", t.image_width}, {"imageheight", t.image_height}, {"spacing", t.spacing}, {"margin", t.margin}};
            Json tiles = Json::array();
            std::set<int> tile_ids;
            for (const auto& [id, props] : t.tile_properties) tile_ids.insert(id);
            for (const auto& [id, anim] : t.animations) tile_ids.insert(id);
            for (const auto& [id, sh] : t.shapes) tile_ids.insert(id);
            for (int id : tile_ids) {
                Json tile{{"id", id}};
                if (auto pit = t.tile_properties.find(id); pit != t.tile_properties.end()) tile["properties"] = tiled_properties(pit->second);
                if (auto a = t.animations.find(id); a != t.animations.end()) {
                    Json frames = Json::array();
                    for (const auto& [fid, ms] : a->second.frames) frames.push_back(Json{{"tileid", fid}, {"duration", ms}});
                    tile["animation"] = frames;
                }
                if (auto sh = t.shapes.find(id); sh != t.shapes.end()) {
                    // Back into the collision editor's pixels.
                    Json objects = Json::array();
                    int oid = 1;
                    for (const TileSet::Shape& s : sh->second) objects.push_back(Json{{"id", oid++}, {"name", ""}, {"type", ""}, {"x", s.x0 * static_cast<float>(t.tile_width)}, {"y", s.y0 * static_cast<float>(t.tile_height)}, {"width", (s.x1 - s.x0) * static_cast<float>(t.tile_width)}, {"height", (s.y1 - s.y0) * static_cast<float>(t.tile_height)}, {"rotation", 0}, {"visible", true}});
                    tile["objectgroup"] = Json{{"type", "objectgroup"}, {"draworder", "index"}, {"name", ""}, {"objects", objects}, {"opacity", 1}, {"visible", true}, {"x", 0}, {"y", 0}};
                }
                tiles.push_back(tile);
            }
            if (!tiles.empty()) tj["tiles"] = tiles;
            sets.push_back(tj);
        }
        doc["tilesets"] = sets;
        Json ls = Json::array();
        int next_id = 1;
        for (const TileLayer& l : layers) {
            Json lj{{"id", l.id > 0 ? l.id : next_id}, {"type", "tilelayer"}, {"name", l.name}, {"width", l.width}, {"height", l.height}, {"x", 0}, {"y", 0}, {"opacity", l.opacity}, {"visible", l.visible}, {"data", Json::array()}};
            if (l.properties.is_object() && !l.properties.empty()) lj["properties"] = tiled_properties(l.properties);
            ls.push_back(lj);
            ++next_id;
        }
        doc["layers"] = ls;
        doc["nextlayerid"] = next_id;
        doc["nextobjectid"] = 1;
    }
    // Tile layers were read in document order (groups flattened), so the k-th tile layer of the
    // document is layers[k].
    std::size_t k = 0;
    std::function<void(Json&)> patch_layer = [&](Json& l) {
        if (!l.is_object()) return;
        std::string type = l.value("type", "tilelayer");
        if (type == "group") {
            if (l.contains("layers") && l["layers"].is_array()) for (Json& c : l["layers"]) patch_layer(c);
            return;
        }
        if (type != "tilelayer") return;
        if (k < layers.size()) {
            Json data = Json::array();
            for (std::uint32_t g : layers[k].gids) data.push_back(g);
            l["data"] = std::move(data);
            l.erase("encoding");
            l.erase("compression");
        }
        ++k;
    };
    if (doc.contains("layers") && doc["layers"].is_array()) for (Json& l : doc["layers"]) patch_layer(l);
    return doc;
}

bool TileMap::solid_at(int x, int y) const { return solidity_at(x, y) == 1; }

int TileMap::solidity_at(int x, int y) const {
    int best = 0;
    for (const TileLayer& l : layers) {
        if (!l.visible || x < 0 || y < 0 || x >= l.width || y >= l.height) continue;
        const std::uint32_t gid = l.gids[static_cast<std::size_t>(y) * static_cast<std::size_t>(l.width) + static_cast<std::size_t>(x)];
        if (gid == 0) continue;
        if (l.solid_layer()) return 1;
        if (const TileSet* ts = tileset_for(gid)) {
            int local = static_cast<int>((gid & kIdMask) - ts->first_gid);
            if (ts->slope(local) != 0) return 3;
            if (ts->solid(local)) return 1;
            if (ts->one_way(local)) best = 2;
        }
    }
    return best;
}

void TileMap::solid_boxes(int x, int y, std::vector<TileSet::Shape>& out) const {
    out.clear();
    const int kind = solidity_at(x, y);
    if (kind != 1 && kind != 2) return;
    for (const TileLayer& l : layers) {
        if (!l.visible || x < 0 || y < 0 || x >= l.width || y >= l.height) continue;
        const std::uint32_t gid = l.gids[static_cast<std::size_t>(y) * static_cast<std::size_t>(l.width) + static_cast<std::size_t>(x)];
        if (gid == 0) continue;
        if (l.solid_layer()) { out.push_back({}); return; }
        const TileSet* ts = tileset_for(gid);
        if (!ts) continue;
        const int local = static_cast<int>((gid & kIdMask) - ts->first_gid);
        if (ts->slope(local) != 0) continue;
        const bool wanted = kind == 1 ? ts->solid(local) : ts->one_way(local);
        if (!wanted) continue;
        if (const std::vector<TileSet::Shape>* sh = ts->shapes_of(local)) {
            const bool fh = (gid & kFlipH) != 0, fv = (gid & kFlipV) != 0;
            for (TileSet::Shape s : *sh) {
                if (fh) s = {1 - s.x1, s.y0, 1 - s.x0, s.y1};
                if (fv) s = {s.x0, 1 - s.y1, s.x1, 1 - s.y0};
                out.push_back(s);
            }
        } else {
            out.push_back({});
        }
        return;
    }
}

bool TileMap::solid_at_point(int x, int y, float fx, float fy) const {
    if (solidity_at(x, y) != 1) return false;
    std::vector<TileSet::Shape> boxes;
    solid_boxes(x, y, boxes);
    for (const TileSet::Shape& s : boxes) if (fx >= s.x0 && fx < s.x1 && fy >= s.y0 && fy < s.y1) return true;
    return false;
}

int TileMap::slope_at(int x, int y) const {
    for (const TileLayer& l : layers) {
        if (!l.visible || x < 0 || y < 0 || x >= l.width || y >= l.height) continue;
        const std::uint32_t gid = l.gids[static_cast<std::size_t>(y) * static_cast<std::size_t>(l.width) + static_cast<std::size_t>(x)];
        if (gid == 0) continue;
        if (const TileSet* ts = tileset_for(gid)) {
            int s = ts->slope(static_cast<int>((gid & kIdMask) - ts->first_gid));
            if (s != 0) return (gid & kFlipH) ? -s : s;
        }
    }
    return 0;
}

// Whether the cell at index `i` along the stagger axis is the shifted one.
bool staggered_index(const TileMap& m, int i) { return (i % 2 != 0) == m.stagger_odd; }

Vec2 TileMap::tile_pixel(int x, int y) const {
    const auto tw = static_cast<float>(tile_width), th = static_cast<float>(tile_height);
    if (orientation == "isometric") return {(static_cast<float>(x - y) + static_cast<float>(height - 1)) * tw * 0.5f, static_cast<float>(x + y) * th * 0.5f};
    if (orientation == "staggered" || orientation == "hexagonal") {
        const auto side = static_cast<float>(hex_side);
        if (stagger_y) return {static_cast<float>(x) * tw + (staggered_index(*this, y) ? tw * 0.5f : 0.0f), static_cast<float>(y) * (th + side) * 0.5f};
        return {static_cast<float>(x) * (tw + side) * 0.5f, static_cast<float>(y) * th + (staggered_index(*this, x) ? th * 0.5f : 0.0f)};
    }
    return {static_cast<float>(x) * tw, static_cast<float>(y) * th};
}

Vec2 TileMap::pixel_size() const {
    const auto tw = static_cast<float>(tile_width), th = static_cast<float>(tile_height);
    const auto w = static_cast<float>(width), h = static_cast<float>(height);
    if (orientation == "isometric") return {(w + h) * tw * 0.5f, (w + h) * th * 0.5f};
    if (orientation == "staggered" || orientation == "hexagonal") {
        const auto side = static_cast<float>(hex_side);
        if (stagger_y) return {w * tw + (height > 1 ? tw * 0.5f : 0.0f), th + (h - 1.0f) * (th + side) * 0.5f};
        return {tw + (w - 1.0f) * (tw + side) * 0.5f, h * th + (width > 1 ? th * 0.5f : 0.0f)};
    }
    return {w * tw, h * th};
}

bool TileMap::cell_at_pixel(float px, float py, int& x, int& y) const {
    const auto tw = static_cast<float>(tile_width), th = static_cast<float>(tile_height);
    if (orientation == "isometric") {
        // Back through the projection: the unprojected tile space has square cells of tile_height.
        const float ux = (px - static_cast<float>(height) * tw * 0.5f) / tw, uy = py / th;
        x = static_cast<int>(std::floor(uy + ux));
        y = static_cast<int>(std::floor(uy - ux));
    } else if (orientation == "staggered" || orientation == "hexagonal") {
        // The cell whose center is nearest, among the guess from the box grid and its neighbours
        // (a hexagon or a diamond is the set of points nearest its center); a diamond's distances
        // are measured with the axes scaled to the tile, so a wide tile is not read as a tall one.
        const auto side = static_cast<float>(hex_side);
        const float sx = orientation == "hexagonal" ? 1.0f : 1.0f / tw, sy = orientation == "hexagonal" ? 1.0f : 1.0f / th;
        int gx, gy;
        if (stagger_y) { gy = static_cast<int>(std::floor(py / ((th + side) * 0.5f))); gx = static_cast<int>(std::floor((px - (staggered_index(*this, gy) ? tw * 0.5f : 0.0f)) / tw)); }
        else { gx = static_cast<int>(std::floor(px / ((tw + side) * 0.5f))); gy = static_cast<int>(std::floor((py - (staggered_index(*this, gx) ? th * 0.5f : 0.0f)) / th)); }
        float best = std::numeric_limits<float>::max();
        x = gx;
        y = gy;
        for (int cy = gy - 1; cy <= gy + 1; ++cy) {
            for (int cx = gx - 1; cx <= gx + 1; ++cx) {
                const Vec2 c = tile_pixel(cx, cy);
                const float dx = (px - (c.x + tw * 0.5f)) * sx, dy = (py - (c.y + th * 0.5f)) * sy;
                const float d = dx * dx + dy * dy;
                if (d < best) { best = d; x = cx; y = cy; }
            }
        }
    } else {
        x = static_cast<int>(std::floor(px / tw));
        y = static_cast<int>(std::floor(py / th));
    }
    return x >= 0 && y >= 0 && x < width && y < height;
}

Vec2 TileMap::object_pixel(float ox, float oy) const {
    if (orientation != "isometric") return {ox, oy};
    const auto tw = static_cast<float>(tile_width), th = static_cast<float>(tile_height);
    const float ux = ox / th, uy = oy / th;   // Tiled keeps isometric objects in the unprojected tile space
    return {(ux - uy) * tw * 0.5f + static_cast<float>(height) * tw * 0.5f, (ux + uy) * th * 0.5f};
}

Json TileMap::describe() const {
    Json j;
    j["path"] = path;
    j["kind"] = "tilemap";
    if (infinite) {
        j["infinite"] = true;
        j["origin"] = Json{{"x", chunk_x}, {"y", chunk_y}};   // where the box starts, in Tiled's tile coordinates
    }
    j["orientation"] = orientation;
    if (orientation == "hexagonal") j["hex_side"] = hex_side;
    if (orientation == "hexagonal" || orientation == "staggered") {
        j["stagger_axis"] = stagger_y ? "y" : "x";
        j["stagger_index"] = stagger_odd ? "odd" : "even";
    }
    j["width"] = width;
    j["height"] = height;
    j["tile_width"] = tile_width;
    j["tile_height"] = tile_height;
    const Vec2 px = pixel_size();
    j["pixel_width"] = px.x;
    j["pixel_height"] = px.y;
    Json ls = Json::array();
    for (const TileLayer& l : layers) {
        std::size_t filled = 0;
        for (std::uint32_t g : l.gids) filled += g != 0;
        ls.push_back(Json{{"name", l.name}, {"id", l.id}, {"width", l.width}, {"height", l.height}, {"tiles", filled}, {"visible", l.visible}, {"solid", l.solid_layer()}, {"properties", l.properties}});
    }
    j["layers"] = ls;
    j["revision"] = revision;
    Json ts = Json::array();
    for (const TileSet& t : tilesets) {
        std::size_t solid = 0;
        for (const auto& [id, p] : t.tile_properties) solid += t.solid(id);
        ts.push_back(Json{{"name", t.name}, {"first_gid", t.first_gid}, {"image", t.image}, {"tile_width", t.tile_width}, {"tile_height", t.tile_height}, {"columns", t.columns}, {"tile_count", t.tile_count}, {"solid_tiles", solid}});
    }
    j["tilesets"] = ts;
    Json os = Json::array();
    for (const ObjectLayer& o : object_layers) os.push_back(Json{{"name", o.name}, {"objects", o.objects.size()}});
    j["object_layers"] = os;
    Json is = Json::array();
    for (const ImageLayer& il : image_layers) is.push_back(Json{{"name", il.name}, {"id", il.id}, {"image", il.image}, {"offset_x", il.offset_x}, {"offset_y", il.offset_y}, {"repeat_x", il.repeat_x}, {"repeat_y", il.repeat_y}, {"parallax_x", il.parallax_x}, {"parallax_y", il.parallax_y}, {"opacity", il.opacity}, {"visible", il.visible}, {"before", il.before}, {"properties", il.properties}});
    j["image_layers"] = is;
    j["properties"] = properties;
    return j;
}

Result<TileMap> parse_tilemap(const std::string& text, const std::string& display_path) {
    Json doc = Json::parse(text, nullptr, false);
    if (doc.is_discarded() || !doc.is_object()) return fail("bad_tilemap", "{}: not a JSON object", display_path);
    TileMap map;
    map.path = display_path;
    map.orientation = doc.value("orientation", "orthogonal");
    if (map.orientation != "orthogonal" && map.orientation != "isometric" && map.orientation != "staggered" && map.orientation != "hexagonal") return fail("bad_tilemap", "{}: orientation '{}' is not one of orthogonal, isometric, staggered, hexagonal", display_path, map.orientation);
    map.width = doc.value("width", 0);
    map.height = doc.value("height", 0);
    map.tile_width = doc.value("tilewidth", 0);
    map.tile_height = doc.value("tileheight", 0);
    map.hex_side = map.orientation == "hexagonal" ? doc.value("hexsidelength", 0) : 0;
    map.stagger_y = doc.value("staggeraxis", "y") != "x";
    map.stagger_odd = doc.value("staggerindex", "odd") != "even";
    map.properties = properties_of(doc);
    map.infinite = doc.value("infinite", false);
    if (map.infinite) {
        if (map.orientation != "orthogonal") return fail("bad_tilemap", "{}: infinite maps are read for orthogonal maps only", display_path);
        // The box around every chunk of every tile layer: the map's size, with cells and objects shifted so it starts at (0, 0).
        int minx = std::numeric_limits<int>::max(), miny = std::numeric_limits<int>::max(), maxx = std::numeric_limits<int>::min(), maxy = std::numeric_limits<int>::min();
        std::function<void(const Json&)> scan = [&](const Json& l) {
            if (!l.is_object()) return;
            if (l.value("type", "tilelayer") == "group") {
                for (const Json& c : l.value("layers", Json::array())) scan(c);
                return;
            }
            for (const Json& c : l.value("chunks", Json::array())) {
                if (!c.is_object()) continue;
                const int x = c.value("x", 0), y = c.value("y", 0), w = c.value("width", 0), h = c.value("height", 0);
                if (w <= 0 || h <= 0) continue;
                minx = std::min(minx, x);
                miny = std::min(miny, y);
                maxx = std::max(maxx, x + w);
                maxy = std::max(maxy, y + h);
            }
        };
        for (const Json& l : doc.value("layers", Json::array())) scan(l);
        if (maxx > minx && maxy > miny) {
            map.chunk_x = minx;
            map.chunk_y = miny;
            map.width = maxx - minx;
            map.height = maxy - miny;
        }
    }
    if (map.width <= 0 || map.height <= 0 || map.tile_width <= 0 || map.tile_height <= 0) return fail("bad_tilemap", "{}: width, height, tilewidth and tileheight must be positive", display_path);
    if (map.hex_side < 0 || (map.orientation == "hexagonal" && map.hex_side > (map.stagger_y ? map.tile_height : map.tile_width))) return fail("bad_tilemap", "{}: hexsidelength must lie between 0 and the tile's extent along the stagger axis", display_path);
    std::filesystem::path base = std::filesystem::path(display_path).parent_path();
    for (const Json& t : doc.value("tilesets", Json::array())) {
        if (!t.is_object()) continue;
        if (t.contains("source")) return fail("bad_tilemap", "{}: external tilesets (.tsx / .tsj) are not supported; embed the tileset in the map", display_path);
        TileSet ts;
        ts.name = t.value("name", "");
        ts.first_gid = t.value("firstgid", 1u);
        ts.image = (base / t.value("image", "")).lexically_normal().generic_string();
        ts.tile_width = t.value("tilewidth", map.tile_width);
        ts.tile_height = t.value("tileheight", map.tile_height);
        ts.columns = t.value("columns", 0);
        ts.tile_count = t.value("tilecount", 0);
        ts.image_width = t.value("imagewidth", 0);
        ts.image_height = t.value("imageheight", 0);
        ts.spacing = t.value("spacing", 0);
        ts.margin = t.value("margin", 0);
        if (ts.columns <= 0 && ts.image_width > 0 && ts.tile_width > 0) ts.columns = (ts.image_width - 2 * ts.margin + ts.spacing) / (ts.tile_width + ts.spacing);
        if (ts.columns <= 0) return fail("bad_tilemap", "{}: tileset '{}' has no columns", display_path, ts.name);
        if (ts.tile_count <= 0 && ts.image_height > 0) ts.tile_count = ts.columns * ((ts.image_height - 2 * ts.margin + ts.spacing) / (ts.tile_height + ts.spacing));
        for (const Json& tile : t.value("tiles", Json::array())) {
            if (!tile.is_object() || !tile.contains("id")) continue;
            const int id = tile["id"].get<int>();
            ts.tile_properties[id] = properties_of(tile);
            if (tile.contains("animation") && tile["animation"].is_array()) {
                TileSet::Animation anim;
                for (const Json& f : tile["animation"]) {
                    if (!f.is_object()) continue;
                    const int ms = std::max(f.value("duration", 0), 0);
                    anim.frames.emplace_back(f.value("tileid", id), ms);
                    anim.total_ms += ms;
                }
                if (!anim.frames.empty() && anim.total_ms > 0) ts.animations[id] = std::move(anim);
            }
            if (tile.contains("objectgroup") && tile["objectgroup"].is_object() && ts.tile_width > 0 && ts.tile_height > 0) {
                // The collision editor's rectangles, as fractions of the tile.
                std::vector<TileSet::Shape> shapes;
                for (const Json& o : tile["objectgroup"].value("objects", Json::array())) {
                    if (!o.is_object() || o.value("point", false) || o.value("ellipse", false) || o.contains("polygon") || o.contains("polyline")) continue;
                    const float w = o.value("width", 0.0f), h = o.value("height", 0.0f), ox = o.value("x", 0.0f), oy = o.value("y", 0.0f);
                    if (w <= 0 || h <= 0) continue;
                    TileSet::Shape s;
                    s.x0 = std::clamp(ox / static_cast<float>(ts.tile_width), 0.0f, 1.0f);
                    s.x1 = std::clamp((ox + w) / static_cast<float>(ts.tile_width), 0.0f, 1.0f);
                    s.y0 = std::clamp(oy / static_cast<float>(ts.tile_height), 0.0f, 1.0f);
                    s.y1 = std::clamp((oy + h) / static_cast<float>(ts.tile_height), 0.0f, 1.0f);
                    if (s.x1 > s.x0 && s.y1 > s.y0) shapes.push_back(s);
                }
                if (!shapes.empty()) ts.shapes[id] = std::move(shapes);
            }
        }
        map.tilesets.push_back(std::move(ts));
    }
    std::function<Status(const Json&)> read_layer = [&](const Json& l) -> Status {
        std::string type = l.value("type", "tilelayer");
        if (type == "group") {
            for (const Json& c : l.value("layers", Json::array())) POCKET_TRY_VOID(read_layer(c));
            return {};
        }
        if (type == "tilelayer") {
            TileLayer layer;
            layer.name = l.value("name", "");
            layer.id = l.value("id", 0);
            layer.width = l.value("width", map.width);
            layer.height = l.value("height", map.height);
            layer.visible = l.value("visible", true);
            layer.opacity = l.value("opacity", 1.0f);
            layer.offset_x = l.value("offsetx", 0.0f);
            layer.offset_y = l.value("offsety", 0.0f);
            layer.properties = properties_of(l);
            if (l.contains("chunks") && l["chunks"].is_array()) {
                // An infinite map's layer: its chunks laid into the box around them all.
                layer.width = map.width;
                layer.height = map.height;
                layer.gids.assign(static_cast<std::size_t>(map.width) * static_cast<std::size_t>(map.height), 0u);
                for (const Json& c : l["chunks"]) {
                    if (!c.is_object()) continue;
                    if (c.value("encoding", "csv") != "csv" || c.contains("compression")) return fail("bad_tilemap", "{}: layer '{}' must use csv encoding (base64 and compression are not supported)", display_path, layer.name);
                    const int cx = c.value("x", 0) - map.chunk_x, cy = c.value("y", 0) - map.chunk_y, cw = c.value("width", 0), ch = c.value("height", 0);
                    const Json& data = c.value("data", Json::array());
                    if (cw <= 0 || ch <= 0 || !data.is_array() || data.size() != static_cast<std::size_t>(cw) * static_cast<std::size_t>(ch)) return fail("bad_tilemap", "{}: layer '{}' has a chunk whose data does not match its size", display_path, layer.name);
                    for (int y = 0; y < ch; ++y) {
                        for (int x = 0; x < cw; ++x) {
                            const Json& g = data[static_cast<std::size_t>(y) * static_cast<std::size_t>(cw) + static_cast<std::size_t>(x)];
                            if (!g.is_number()) return fail("bad_tilemap", "{}: layer '{}' has a non-numeric tile", display_path, layer.name);
                            const int mx = cx + x, my = cy + y;
                            if (mx < 0 || my < 0 || mx >= map.width || my >= map.height) continue;
                            layer.gids[static_cast<std::size_t>(my) * static_cast<std::size_t>(map.width) + static_cast<std::size_t>(mx)] = static_cast<std::uint32_t>(g.get<double>());
                        }
                    }
                }
                for (std::uint32_t g : layer.gids) {
                    if (g != 0 && !map.tileset_for(g)) return fail("bad_tilemap", "{}: layer '{}' uses gid {} that no tileset covers", display_path, layer.name, g & TileMap::kIdMask);
                }
                map.layers.push_back(std::move(layer));
                return {};
            }
            if (l.value("encoding", "csv") != "csv" || l.contains("compression")) return fail("bad_tilemap", "{}: layer '{}' must use csv encoding (base64 and compression are not supported)", display_path, layer.name);
            const Json& data = l.value("data", Json::array());
            if (!data.is_array() || data.size() != static_cast<std::size_t>(layer.width) * static_cast<std::size_t>(layer.height)) return fail("bad_tilemap", "{}: layer '{}' data does not match {}x{}", display_path, layer.name, layer.width, layer.height);
            layer.gids.reserve(data.size());
            for (const Json& g : data) {
                if (!g.is_number()) return fail("bad_tilemap", "{}: layer '{}' has a non-numeric tile", display_path, layer.name);
                layer.gids.push_back(static_cast<std::uint32_t>(g.get<double>()));
            }
            for (std::uint32_t g : layer.gids) {
                if (g != 0 && !map.tileset_for(g)) return fail("bad_tilemap", "{}: layer '{}' uses gid {} that no tileset covers", display_path, layer.name, g & TileMap::kIdMask);
            }
            map.layers.push_back(std::move(layer));
            return {};
        }
        if (type == "objectgroup") {
            ObjectLayer ol;
            ol.name = l.value("name", "");
            ol.properties = properties_of(l);
            for (const Json& o : l.value("objects", Json::array())) {
                if (!o.is_object()) continue;
                MapObject mo;
                mo.name = o.value("name", "");
                mo.type = o.contains("type") ? o.value("type", "") : o.value("class", "");
                mo.x = o.value("x", 0.0f);
                mo.y = o.value("y", 0.0f);
                if (map.infinite) {   // into the box's own pixel space
                    mo.x -= static_cast<float>(map.chunk_x * map.tile_width);
                    mo.y -= static_cast<float>(map.chunk_y * map.tile_height);
                }
                mo.width = o.value("width", 0.0f);
                mo.height = o.value("height", 0.0f);
                mo.gid = o.value("gid", 0u);
                mo.point = o.value("point", false);
                for (const char* key : {"polyline", "polygon"}) {
                    if (!o.contains(key) || !o[key].is_array()) continue;
                    for (const Json& p : o[key]) if (p.is_object()) mo.points.push_back(Vec2{p.value("x", 0.0f), p.value("y", 0.0f)});
                    mo.closed = std::string_view(key) == "polygon";
                }
                mo.properties = properties_of(o);
                ol.objects.push_back(std::move(mo));
            }
            map.object_layers.push_back(std::move(ol));
            return {};
        }
        if (type == "imagelayer") {
            ImageLayer il;
            il.name = l.value("name", "");
            il.id = l.value("id", 0);
            const std::string image = l.value("image", "");
            if (!image.empty()) il.image = (base / image).lexically_normal().generic_string();
            il.offset_x = l.value("offsetx", 0.0f) + l.value("x", 0.0f);
            il.offset_y = l.value("offsety", 0.0f) + l.value("y", 0.0f);
            if (map.infinite) {
                il.offset_x -= static_cast<float>(map.chunk_x * map.tile_width);
                il.offset_y -= static_cast<float>(map.chunk_y * map.tile_height);
            }
            il.opacity = l.value("opacity", 1.0f);
            il.visible = l.value("visible", true);
            il.repeat_x = l.value("repeatx", false);
            il.repeat_y = l.value("repeaty", false);
            il.parallax_x = l.value("parallaxx", 1.0f);
            il.parallax_y = l.value("parallaxy", 1.0f);
            if (const std::string tint = l.value("tintcolor", ""); tint.size() == 7 || tint.size() == 9) {
                // "#rrggbb" or "#aarrggbb"
                auto hex = [&](std::size_t at) { return static_cast<float>(std::stoi(tint.substr(at, 2), nullptr, 16)) / 255.0f; };
                const bool alpha = tint.size() == 9;
                il.tint = {hex(alpha ? 3 : 1), hex(alpha ? 5 : 3), hex(alpha ? 7 : 5), alpha ? hex(1) : 1.0f};
            }
            il.properties = properties_of(l);
            il.before = static_cast<int>(map.layers.size());
            map.image_layers.push_back(std::move(il));
            return {};
        }
        return {};  // unknown kinds are ignored
    };
    for (const Json& l : doc.value("layers", Json::array())) POCKET_TRY_VOID(read_layer(l));
    map.source = std::move(doc);
    return map;
}

Json Image::describe() const {
    Json j;
    j["path"] = path;
    j["width"] = width;
    j["height"] = height;
    j["bytes"] = rgba.size() + hdr.size() * sizeof(float);
    if (!hdr.empty()) j["hdr"] = true;
    return j;
}

namespace {

struct Accessor {
    const std::uint8_t* data = nullptr;
    std::size_t count = 0;
    std::size_t stride = 0;      // bytes between elements
    int component_type = 0;      // 5120..5126
    int components = 1;          // 1 (SCALAR), 2, 3, 4, 16
    bool normalized = false;
};

int components_of(const std::string& type) {
    if (type == "SCALAR") return 1;
    if (type == "VEC2") return 2;
    if (type == "VEC3") return 3;
    if (type == "VEC4") return 4;
    if (type == "MAT4") return 16;
    if (type == "MAT3") return 9;
    if (type == "MAT2") return 4;
    return 0;
}

std::size_t component_size(int ct) {
    switch (ct) {
        case 5120: case 5121: return 1;
        case 5122: case 5123: return 2;
        case 5125: case 5126: return 4;
        default: return 0;
    }
}

float read_float(const std::uint8_t* p, int ct, bool normalized) {
    switch (ct) {
        case 5126: { float f; std::memcpy(&f, p, 4); return f; }
        case 5120: { std::int8_t v; std::memcpy(&v, p, 1); return normalized ? std::max(static_cast<float>(v) / 127.0f, -1.0f) : static_cast<float>(v); }
        case 5121: { std::uint8_t v = *p; return normalized ? static_cast<float>(v) / 255.0f : static_cast<float>(v); }
        case 5122: { std::int16_t v; std::memcpy(&v, p, 2); return normalized ? std::max(static_cast<float>(v) / 32767.0f, -1.0f) : static_cast<float>(v); }
        case 5123: { std::uint16_t v; std::memcpy(&v, p, 2); return normalized ? static_cast<float>(v) / 65535.0f : static_cast<float>(v); }
        case 5125: { std::uint32_t v; std::memcpy(&v, p, 4); return static_cast<float>(v); }
        default: return 0;
    }
}

std::uint32_t read_index(const std::uint8_t* p, int ct) {
    switch (ct) {
        case 5121: return *p;
        case 5123: { std::uint16_t v; std::memcpy(&v, p, 2); return v; }
        case 5125: { std::uint32_t v; std::memcpy(&v, p, 4); return v; }
        default: return 0;
    }
}

struct Gltf {
    Json doc;
    std::vector<std::string> buffers;
    std::filesystem::path base_dir;
    std::string display;

    Result<Accessor> accessor(int index) const {
        if (!doc.contains("accessors") || index < 0 || index >= static_cast<int>(doc["accessors"].size())) return fail("bad_gltf", "{}: accessor {} out of range", display, index);
        const Json& a = doc["accessors"][static_cast<std::size_t>(index)];
        Accessor acc;
        acc.count = a.value("count", 0);
        acc.component_type = a.value("componentType", 0);
        acc.components = components_of(a.value("type", ""));
        acc.normalized = a.value("normalized", false);
        std::size_t csize = component_size(acc.component_type);
        if (acc.components == 0 || csize == 0) return fail("bad_gltf", "{}: accessor {} has an unsupported type", display, index);
        if (!a.contains("bufferView")) return fail("bad_gltf", "{}: accessor {} without bufferView (sparse accessors are not supported)", display, index);
        int bv_index = a["bufferView"].get<int>();
        if (bv_index < 0 || bv_index >= static_cast<int>(doc.value("bufferViews", Json::array()).size())) return fail("bad_gltf", "{}: bufferView {} out of range", display, bv_index);
        const Json& bv = doc["bufferViews"][static_cast<std::size_t>(bv_index)];
        int buffer = bv.value("buffer", -1);
        if (buffer < 0 || buffer >= static_cast<int>(buffers.size())) return fail("bad_gltf", "{}: buffer {} out of range", display, buffer);
        std::size_t offset = static_cast<std::size_t>(bv.value("byteOffset", 0)) + static_cast<std::size_t>(a.value("byteOffset", 0));
        acc.stride = bv.value("byteStride", 0);
        if (acc.stride == 0) acc.stride = csize * static_cast<std::size_t>(acc.components);
        const std::string& bytes = buffers[static_cast<std::size_t>(buffer)];
        std::size_t needed = acc.count == 0 ? 0 : offset + (acc.count - 1) * acc.stride + csize * static_cast<std::size_t>(acc.components);
        if (needed > bytes.size()) return fail("bad_gltf", "{}: accessor {} reads past buffer {} ({} > {})", display, index, buffer, needed, bytes.size());
        acc.data = reinterpret_cast<const std::uint8_t*>(bytes.data()) + offset;
        return acc;
    }
};

Mat4 node_matrix(const Json& node) {
    if (node.contains("matrix") && node["matrix"].is_array() && node["matrix"].size() == 16) {
        Mat4 m;
        for (int i = 0; i < 16; ++i) m.m[i] = node["matrix"][static_cast<std::size_t>(i)].get<float>();
        return m;
    }
    Vec3 t{0, 0, 0}, s{1, 1, 1};
    Quat r{0, 0, 0, 1};
    if (node.contains("translation") && node["translation"].size() == 3) t = {node["translation"][0].get<float>(), node["translation"][1].get<float>(), node["translation"][2].get<float>()};
    if (node.contains("scale") && node["scale"].size() == 3) s = {node["scale"][0].get<float>(), node["scale"][1].get<float>(), node["scale"][2].get<float>()};
    if (node.contains("rotation") && node["rotation"].size() == 4) r = {node["rotation"][0].get<float>(), node["rotation"][1].get<float>(), node["rotation"][2].get<float>(), node["rotation"][3].get<float>()};
    return Mat4::trs(t, r, s);
}

Mat4 transpose_of(const Mat4& m) {
    Mat4 r;
    for (int c = 0; c < 4; ++c)
        for (int row = 0; row < 4; ++row) r.at(c, row) = m.at(row, c);
    return r;
}

}  // namespace

void fill_tangents(Mesh& mesh) {
    bool missing = false;
    for (const MeshVertex& v : mesh.vertices) if (v.tangent.w == 0) { missing = true; break; }
    if (!missing) return;
    const std::vector<Vec4> made = uv_tangents(mesh.vertices, mesh.indices);
    for (std::size_t i = 0; i < mesh.vertices.size(); ++i) if (mesh.vertices[i].tangent.w == 0) mesh.vertices[i].tangent = made[i];
}

Result<Mesh> parse_gltf(const std::string& bytes, const std::filesystem::path& base_dir, const std::string& display_path) {
    Gltf g;
    g.base_dir = base_dir;
    g.display = display_path;
    std::string json_text;
    std::string bin_chunk;
    if (bytes.size() >= 12 && bytes.compare(0, 4, "glTF") == 0) {
        std::uint32_t version, length;
        std::memcpy(&version, bytes.data() + 4, 4);
        std::memcpy(&length, bytes.data() + 8, 4);
        if (version != 2) return fail("bad_gltf", "{}: GLB version {} (only 2 is supported)", display_path, version);
        std::size_t pos = 12;
        while (pos + 8 <= bytes.size() && pos + 8 <= length) {
            std::uint32_t chunk_len, chunk_type;
            std::memcpy(&chunk_len, bytes.data() + pos, 4);
            std::memcpy(&chunk_type, bytes.data() + pos + 4, 4);
            pos += 8;
            if (pos + chunk_len > bytes.size()) return fail("bad_gltf", "{}: truncated GLB chunk", display_path);
            if (chunk_type == 0x4E4F534Au) json_text = bytes.substr(pos, chunk_len);       // JSON
            else if (chunk_type == 0x004E4942u) bin_chunk = bytes.substr(pos, chunk_len);  // BIN
            pos += chunk_len;
        }
        if (json_text.empty()) return fail("bad_gltf", "{}: GLB without a JSON chunk", display_path);
    } else {
        json_text = bytes;
    }
    g.doc = Json::parse(json_text, nullptr, false);
    if (g.doc.is_discarded() || !g.doc.is_object()) return fail("bad_gltf", "{}: not valid glTF JSON", display_path);
    // Buffers: the GLB chunk, data URIs, or files next to the .gltf.
    for (const Json& b : g.doc.value("buffers", Json::array())) {
        if (!b.contains("uri")) {
            g.buffers.push_back(bin_chunk);
            continue;
        }
        std::string uri = b["uri"].get<std::string>();
        if (uri.starts_with("data:")) {
            std::size_t comma = uri.find(',');
            if (comma == std::string::npos || uri.find(";base64") == std::string::npos) return fail("bad_gltf", "{}: unsupported data URI", display_path);
            std::string_view enc(uri.data() + comma + 1, uri.size() - comma - 1);
            std::string out;
            out.reserve(enc.size() * 3 / 4);
            auto val = [](char c) -> int {
                if (c >= 'A' && c <= 'Z') return c - 'A';
                if (c >= 'a' && c <= 'z') return c - 'a' + 26;
                if (c >= '0' && c <= '9') return c - '0' + 52;
                if (c == '+') return 62;
                if (c == '/') return 63;
                return -1;
            };
            int buf = 0, bits = 0;
            for (char c : enc) {
                int v = val(c);
                if (v < 0) continue;
                buf = (buf << 6) | v;
                bits += 6;
                if (bits >= 8) { bits -= 8; out.push_back(static_cast<char>((buf >> bits) & 0xFF)); }
            }
            g.buffers.push_back(std::move(out));
        } else {
            POCKET_TRY(file, fs::read_bytes(base_dir / uri));
            g.buffers.push_back(std::string(file.begin(), file.end()));
        }
    }
    Mesh mesh;
    mesh.path = display_path;
    // Materials.
    // A textureInfo object -> project-relative image path ("" when it cannot be resolved).
    auto texture_path = [&](const Json& info) -> std::string {
        if (!info.is_object() || !info.contains("index")) return "";
        int ti = info["index"].get<int>();
        const Json& textures = g.doc.value("textures", Json::array());
        if (ti < 0 || ti >= static_cast<int>(textures.size()) || !textures[static_cast<std::size_t>(ti)].contains("source")) return "";
        int si = textures[static_cast<std::size_t>(ti)]["source"].get<int>();
        const Json& images = g.doc.value("images", Json::array());
        if (si < 0 || si >= static_cast<int>(images.size())) return "";
        const Json& img = images[static_cast<std::size_t>(si)];
        if (img.contains("uri") && !img["uri"].get<std::string>().starts_with("data:")) {
            // Relative to the glTF file; expressed relative to the project like every asset path.
            std::filesystem::path rel = std::filesystem::path(display_path).parent_path() / img["uri"].get<std::string>();
            return rel.lexically_normal().generic_string();
        }
        if (img.contains("bufferView")) return display_path + "#image" + std::to_string(si);  // embedded: decoded on request
        return "";
    };
    for (const Json& m : g.doc.value("materials", Json::array())) {
        Material mat;
        mat.name = m.value("name", "");
        mat.double_sided = m.value("doubleSided", false);
        mat.blend = m.value("alphaMode", "OPAQUE") == "BLEND";
        if (m.value("alphaMode", "OPAQUE") == "MASK") mat.alpha_cutoff = m.value("alphaCutoff", 0.5f);
        if (m.contains("pbrMetallicRoughness")) {
            const Json& pbr = m["pbrMetallicRoughness"];
            if (pbr.contains("baseColorFactor") && pbr["baseColorFactor"].size() == 4) {
                mat.base_color = {pbr["baseColorFactor"][0].get<float>(), pbr["baseColorFactor"][1].get<float>(), pbr["baseColorFactor"][2].get<float>(), pbr["baseColorFactor"][3].get<float>()};
            }
            mat.metallic = pbr.value("metallicFactor", 1.0f);
            mat.roughness = pbr.value("roughnessFactor", 1.0f);
            if (pbr.contains("baseColorTexture")) {
                mat.texture = texture_path(pbr["baseColorTexture"]);
                const Json& info = pbr["baseColorTexture"];
                if (info.is_object() && info.contains("extensions") && info["extensions"].is_object() && info["extensions"].contains("KHR_texture_transform")) {
                    const Json& tt = info["extensions"]["KHR_texture_transform"];
                    if (tt.contains("offset") && tt["offset"].is_array() && tt["offset"].size() == 2) mat.uv_offset = {tt["offset"][0].get<float>(), tt["offset"][1].get<float>()};
                    if (tt.contains("scale") && tt["scale"].is_array() && tt["scale"].size() == 2) mat.uv_scale = {tt["scale"][0].get<float>(), tt["scale"][1].get<float>()};
                    mat.uv_transformed = mat.uv_offset.x != 0 || mat.uv_offset.y != 0 || mat.uv_scale.x != 1 || mat.uv_scale.y != 1;
                }
            }
            if (pbr.contains("metallicRoughnessTexture")) mat.metallic_roughness_texture = texture_path(pbr["metallicRoughnessTexture"]);
        }
        if (m.contains("normalTexture")) {
            mat.normal_texture = texture_path(m["normalTexture"]);
            mat.normal_scale = m["normalTexture"].value("scale", 1.0f);
        }
        if (m.contains("emissiveTexture")) mat.emissive_texture = texture_path(m["emissiveTexture"]);
        if (m.contains("emissiveFactor") && m["emissiveFactor"].size() == 3) {
            mat.emissive = {m["emissiveFactor"][0].get<float>(), m["emissiveFactor"][1].get<float>(), m["emissiveFactor"][2].get<float>()};
        }
        // KHR_materials_emissive_strength: emission brighter than the factor's 0..1 (Blender's strength).
        if (m.contains("extensions") && m["extensions"].is_object() && m["extensions"].contains("KHR_materials_emissive_strength")) {
            const float k = m["extensions"]["KHR_materials_emissive_strength"].value("emissiveStrength", 1.0f);
            mat.emissive = mat.emissive * k;
        }
        // Glass and lacquer: transmission, its index of refraction and volume, and a clear coat
        // (what Blender's Principled BSDF writes for Transmission, IOR and Coat).
        if (m.contains("extensions") && m["extensions"].is_object()) {
            const Json& ext = m["extensions"];
            auto obj = [&](const char* name) { return ext.contains(name) && ext[name].is_object() ? &ext[name] : nullptr; };
            if (const Json* t = obj("KHR_materials_transmission")) mat.transmission = std::clamp(t->value("transmissionFactor", 0.0f), 0.0f, 1.0f);
            if (const Json* t = obj("KHR_materials_ior")) mat.ior = std::max(t->value("ior", 1.5f), 1.0f);
            if (const Json* t = obj("KHR_materials_volume")) {
                mat.thickness = std::max(t->value("thicknessFactor", 0.0f), 0.0f);
                mat.attenuation_distance = std::max(t->value("attenuationDistance", 0.0f), 0.0f);
                if (t->contains("attenuationColor") && (*t)["attenuationColor"].is_array() && (*t)["attenuationColor"].size() == 3)
                    mat.attenuation_color = {(*t)["attenuationColor"][0].get<float>(), (*t)["attenuationColor"][1].get<float>(), (*t)["attenuationColor"][2].get<float>()};
            }
            if (const Json* t = obj("KHR_materials_clearcoat")) {
                mat.clearcoat = std::clamp(t->value("clearcoatFactor", 0.0f), 0.0f, 1.0f);
                mat.clearcoat_roughness = std::clamp(t->value("clearcoatRoughnessFactor", 0.0f), 0.0f, 1.0f);
            }
            // Cloth, the specular layer, brushed metal and shadeless surfaces (Blender's Sheen,
            // Specular IOR Level and Tint, Anisotropic, and an emission-only or unlit material).
            auto vec3_at = [](const Json& o, const char* key, Vec3 fallback) {
                if (!o.contains(key) || !o[key].is_array() || o[key].size() != 3) return fallback;
                return Vec3{o[key][0].get<float>(), o[key][1].get<float>(), o[key][2].get<float>()};
            };
            if (const Json* t = obj("KHR_materials_sheen")) {
                mat.sheen_color = vec3_at(*t, "sheenColorFactor", Vec3{0, 0, 0});
                mat.sheen_roughness = std::clamp(t->value("sheenRoughnessFactor", 0.0f), 0.0f, 1.0f);
            }
            if (const Json* t = obj("KHR_materials_specular")) {
                mat.specular = std::clamp(t->value("specularFactor", 1.0f), 0.0f, 1.0f);
                mat.specular_color = vec3_at(*t, "specularColorFactor", Vec3{1, 1, 1});
            }
            if (const Json* t = obj("KHR_materials_anisotropy")) {
                mat.anisotropy = std::clamp(t->value("anisotropyStrength", 0.0f), 0.0f, 1.0f);
                mat.anisotropy_rotation = t->value("anisotropyRotation", 0.0f);
            }
            if (obj("KHR_materials_unlit")) mat.unlit = true;
        }
        mesh.materials.push_back(mat);
    }
    if (mesh.materials.empty()) mesh.materials.push_back(Material{});
    const std::size_t default_material = mesh.materials.size() - 1;
    bool has_default = g.doc.value("materials", Json::array()).empty();
    // Nodes: walk the scene graph (default scene, or every root) and bake mesh instances.
    const Json& nodes = g.doc.value("nodes", Json::array());
    const Json& meshes = g.doc.value("meshes", Json::array());
    std::vector<int> roots;
    if (g.doc.contains("scenes") && !g.doc["scenes"].empty()) {
        int scene_index = g.doc.value("scene", 0);
        const Json& scene = g.doc["scenes"][static_cast<std::size_t>(std::clamp(scene_index, 0, static_cast<int>(g.doc["scenes"].size()) - 1))];
        for (const Json& n : scene.value("nodes", Json::array())) roots.push_back(n.get<int>());
    } else {
        std::vector<bool> is_child(nodes.size(), false);
        for (const Json& n : nodes) for (const Json& c : n.value("children", Json::array())) if (c.get<std::size_t>() < is_child.size()) is_child[c.get<std::size_t>()] = true;
        for (std::size_t i = 0; i < nodes.size(); ++i) if (!is_child[i]) roots.push_back(static_cast<int>(i));
    }
    // The node hierarchy as authored (skeletons live here), before any baking.
    for (std::size_t i = 0; i < nodes.size(); ++i) {
        const Json& n = nodes[i];
        Node out;
        out.name = n.value("name", "node" + std::to_string(i));
        out.rest = node_matrix(n);
        if (n.contains("translation") && n["translation"].size() == 3) out.translation = {n["translation"][0].get<float>(), n["translation"][1].get<float>(), n["translation"][2].get<float>()};
        if (n.contains("scale") && n["scale"].size() == 3) out.scale = {n["scale"][0].get<float>(), n["scale"][1].get<float>(), n["scale"][2].get<float>()};
        if (n.contains("rotation") && n["rotation"].size() == 4) out.rotation = {n["rotation"][0].get<float>(), n["rotation"][1].get<float>(), n["rotation"][2].get<float>(), n["rotation"][3].get<float>()};
        for (const Json& c : n.value("children", Json::array())) {
            int ci = c.get<int>();
            if (ci >= 0 && ci < static_cast<int>(nodes.size())) out.children.push_back(ci);
        }
        if (n.contains("extensions") && n["extensions"].is_object() && n["extensions"].contains("KHR_lights_punctual")) out.light = n["extensions"]["KHR_lights_punctual"].value("light", -1);
        if (n.contains("extras") && n["extras"].is_object() && !n["extras"].empty()) out.extras = n["extras"];
        if (n.contains("camera") && n["camera"].is_number_integer()) out.camera = n["camera"].get<int>();
        mesh.nodes.push_back(std::move(out));
    }
    for (std::size_t i = 0; i < mesh.nodes.size(); ++i) {
        for (int c : mesh.nodes[i].children) mesh.nodes[static_cast<std::size_t>(c)].parent = static_cast<int>(i);
    }
    // Skins: joints and their inverse bind matrices.
    for (const Json& sj : g.doc.value("skins", Json::array())) {
        Skin skin;
        skin.name = sj.value("name", "skin" + std::to_string(mesh.skins.size()));
        for (const Json& j : sj.value("joints", Json::array())) {
            int ji = j.get<int>();
            if (ji < 0 || ji >= static_cast<int>(mesh.nodes.size())) return fail("bad_gltf", "{}: skin joint {} out of range", display_path, ji);
            skin.joints.push_back(ji);
        }
        if (skin.joints.empty()) return fail("bad_gltf", "{}: a skin without joints", display_path);
        if (sj.contains("inverseBindMatrices")) {
            POCKET_TRY(ibm, g.accessor(sj["inverseBindMatrices"].get<int>()));
            if (ibm.components != 16 || ibm.count < skin.joints.size()) return fail("bad_gltf", "{}: inverseBindMatrices must hold one MAT4 per joint", display_path);
            std::size_t cs = component_size(ibm.component_type);
            for (std::size_t j = 0; j < skin.joints.size(); ++j) {
                Mat4 m;
                const std::uint8_t* mp = ibm.data + j * ibm.stride;
                for (int k = 0; k < 16; ++k) m.m[k] = read_float(mp + static_cast<std::size_t>(k) * cs, ibm.component_type, ibm.normalized);
                skin.inverse_bind.push_back(m);
            }
        } else {
            skin.inverse_bind.assign(skin.joints.size(), Mat4::identity());
        }
        mesh.skins.push_back(std::move(skin));
    }
    // Animations: keyframe channels on node translation, rotation and scale.
    for (const Json& aj : g.doc.value("animations", Json::array())) {
        AnimationClip clip;
        clip.name = aj.value("name", "animation" + std::to_string(mesh.animations.size()));
        const Json& samplers = aj.value("samplers", Json::array());
        for (const Json& ch : aj.value("channels", Json::array())) {
            const Json& target = ch.value("target", Json::object());
            std::string path = target.value("path", "");
            int node = target.value("node", -1);
            int si = ch.value("sampler", -1);
            if (node < 0 || node >= static_cast<int>(mesh.nodes.size()) || si < 0 || si >= static_cast<int>(samplers.size())) continue;
            AnimationChannel c;
            c.node = node;
            int width = 3;
            if (path == "translation") c.path = 0;
            else if (path == "rotation") { c.path = 1; width = 4; }
            else if (path == "scale") c.path = 2;
            else if (path == "weights") {
                // Morph target weights: one scalar per target per key, the targets being those of
                // the node's mesh (every primitive of a mesh has the same number).
                c.path = 3;
                const Json& node_j = nodes[static_cast<std::size_t>(node)];
                const Json& meshes_j = g.doc.value("meshes", Json::array());
                const int mi = node_j.value("mesh", -1);
                width = 0;
                if (mi >= 0 && mi < static_cast<int>(meshes_j.size())) {
                    const Json& prims = meshes_j[static_cast<std::size_t>(mi)].value("primitives", Json::array());
                    if (!prims.empty()) width = static_cast<int>(prims[0].value("targets", Json::array()).size());
                }
                if (width <= 0) continue;
            } else {
                continue;
            }
            c.width = width;
            const Json& sampler = samplers[static_cast<std::size_t>(si)];
            std::string interp = sampler.value("interpolation", "LINEAR");
            c.step = interp == "STEP";
            const bool cubic = interp == "CUBICSPLINE";
            POCKET_TRY(in, g.accessor(sampler.value("input", -1)));
            POCKET_TRY(out, g.accessor(sampler.value("output", -1)));
            const std::size_t per_key = cubic ? 3 : 1;
            if (c.path == 3) {
                if (out.components != 1) return fail("bad_gltf", "{}: animation '{}' weights output must be SCALAR", display_path, clip.name);
                if (out.count < in.count * per_key * static_cast<std::size_t>(width)) return fail("bad_gltf", "{}: animation '{}' has fewer weight outputs than keys", display_path, clip.name);
            } else {
                if (out.components != width) return fail("bad_gltf", "{}: animation '{}' output has {} components for {}", display_path, clip.name, out.components, path);
                if (out.count < in.count * per_key) return fail("bad_gltf", "{}: animation '{}' has fewer outputs than keys", display_path, clip.name);
            }
            std::size_t ics = component_size(in.component_type), ocs = component_size(out.component_type);
            for (std::size_t k = 0; k < in.count; ++k) {
                float t = read_float(in.data + k * in.stride, in.component_type, in.normalized);
                c.times.push_back(t);
                clip.duration = std::max(clip.duration, t);
                if (c.path == 3) {
                    // Scalars: the key's block of `width` values (a cubic key holds in-tangents, values, out-tangents).
                    const std::size_t first = (k * per_key + (cubic ? 1 : 0)) * static_cast<std::size_t>(width);
                    for (int w = 0; w < width; ++w) c.values.push_back(read_float(out.data + (first + static_cast<std::size_t>(w)) * out.stride, out.component_type, out.normalized));
                } else {
                    const std::uint8_t* vp = out.data + (k * per_key + (cubic ? 1 : 0)) * out.stride;
                    for (int w = 0; w < width; ++w) c.values.push_back(read_float(vp + static_cast<std::size_t>(w) * ocs, out.component_type, out.normalized));
                }
            }
            (void)ics;
            if (!c.times.empty()) clip.channels.push_back(std::move(c));
        }
        mesh.animations.push_back(std::move(clip));
    }
    // Nodes a clip moves, and everything below them: their unskinned geometry is kept in the
    // node's own space as a moving part the pose places, instead of baked into the file's space.
    std::vector<bool> moved(nodes.size(), false);
    for (const AnimationClip& clip : mesh.animations) {
        for (const AnimationChannel& c : clip.channels) {
            if (c.path == 3 || c.node < 0 || c.node >= static_cast<int>(nodes.size())) continue;   // weights move no node
            std::vector<int> stack{c.node};
            while (!stack.empty()) {
                const int n = stack.back();
                stack.pop_back();
                if (n < 0 || n >= static_cast<int>(nodes.size()) || moved[static_cast<std::size_t>(n)]) continue;
                moved[static_cast<std::size_t>(n)] = true;
                if (static_cast<std::size_t>(n) < mesh.nodes.size()) for (int ch : mesh.nodes[static_cast<std::size_t>(n)].children) stack.push_back(ch);
            }
        }
    }
    bool any_geometry = false;
    std::function<Status(int, const Mat4&, int)> visit = [&](int ni, const Mat4& parent, int depth) -> Status {
        if (depth > 64 || ni < 0 || ni >= static_cast<int>(nodes.size())) return {};
        const Json& node = nodes[static_cast<std::size_t>(ni)];
        Mat4 world = parent * node_matrix(node);
        if (node.contains("mesh")) {
            int mi = node["mesh"].get<int>();
            if (mi < 0 || mi >= static_cast<int>(meshes.size())) return fail("bad_gltf", "{}: mesh {} out of range", display_path, mi);
            mesh.node_count++;
            // A skinned mesh stays in its bind space: the joints place it, not the node (glTF 2.0).
            const int skin_index = node.value("skin", -1);
            if (skin_index >= static_cast<int>(mesh.skins.size())) return fail("bad_gltf", "{}: node skin {} out of range", display_path, skin_index);
            const bool part = skin_index < 0 && moved[static_cast<std::size_t>(ni)];
            const Mat4 bake = skin_index >= 0 || part ? Mat4::identity() : world;
            Mat4 normal_m = transpose_of(bake.inverse_affine());
            const Vec3 bx{bake.at(0, 0), bake.at(0, 1), bake.at(0, 2)}, by{bake.at(1, 0), bake.at(1, 1), bake.at(1, 2)}, bz{bake.at(2, 0), bake.at(2, 1), bake.at(2, 2)};
            const bool bake_mirrors = dot(cross(bx, by), bz) < 0;
            for (const Json& prim : meshes[static_cast<std::size_t>(mi)].value("primitives", Json::array())) {
                int mode = prim.value("mode", 4);
                if (mode != 4) continue;  // triangles only
                const Json& attrs = prim.value("attributes", Json::object());
                if (!attrs.contains("POSITION")) continue;
                POCKET_TRY(pos, g.accessor(attrs["POSITION"].get<int>()));
                std::optional<Accessor> nrm, uv, jnt, wgt, col, tan;
                if (attrs.contains("COLOR_0")) { POCKET_TRY(a, g.accessor(attrs["COLOR_0"].get<int>())); col = a; mesh.vertex_colors = true; }
                if (attrs.contains("NORMAL")) { POCKET_TRY(a, g.accessor(attrs["NORMAL"].get<int>())); nrm = a; }
                if (attrs.contains("TEXCOORD_0")) { POCKET_TRY(a, g.accessor(attrs["TEXCOORD_0"].get<int>())); uv = a; }
                if (attrs.contains("TANGENT")) { POCKET_TRY(a, g.accessor(attrs["TANGENT"].get<int>())); if (a.components == 4) { tan = a; mesh.file_tangents = true; } }
                if (skin_index >= 0 && attrs.contains("JOINTS_0") && attrs.contains("WEIGHTS_0")) {
                    POCKET_TRY(a, g.accessor(attrs["JOINTS_0"].get<int>())); jnt = a;
                    POCKET_TRY(b, g.accessor(attrs["WEIGHTS_0"].get<int>())); wgt = b;
                    if (jnt->components != 4 || wgt->components != 4) return fail("bad_gltf", "{}: JOINTS_0 and WEIGHTS_0 must be VEC4", display_path);
                }
                std::uint32_t base = static_cast<std::uint32_t>(mesh.vertices.size());
                if (jnt) mesh.skin_vertices.resize(base);  // zeros for any unskinned geometry before this
                // Morph targets: the primitive's targets by index become the mesh's (their deltas
                // baked like the geometry), named by the mesh's extras.targetNames when it has them.
                struct TargetAccessors { std::optional<Accessor> pos, nrm; };
                std::vector<TargetAccessors> targets;
                {
                    const Json& mesh_j = meshes[static_cast<std::size_t>(mi)];
                    const Json& names = mesh_j.contains("extras") && mesh_j["extras"].is_object() ? mesh_j["extras"].value("targetNames", Json::array()) : Json::array();
                    const Json& tj = prim.value("targets", Json::array());
                    for (std::size_t t = 0; t < tj.size(); ++t) {
                        TargetAccessors ta;
                        if (tj[t].contains("POSITION")) { POCKET_TRY(a, g.accessor(tj[t]["POSITION"].get<int>())); ta.pos = a; }
                        if (tj[t].contains("NORMAL")) { POCKET_TRY(a, g.accessor(tj[t]["NORMAL"].get<int>())); ta.nrm = a; }
                        targets.push_back(ta);
                        if (mesh.morph_targets.size() <= t) {
                            MorphTarget mt;
                            mt.name = t < names.size() && names[t].is_string() ? names[t].get<std::string>() : "target" + std::to_string(t);
                            mesh.morph_targets.push_back(std::move(mt));
                        }
                    }
                    if (!tj.empty() && mesh.default_weights.empty() && mesh_j.contains("weights") && mesh_j["weights"].is_array()) {
                        for (const Json& wv : mesh_j["weights"]) mesh.default_weights.push_back(wv.is_number() ? wv.get<float>() : 0.0f);
                    }
                }
                for (auto& mt : mesh.morph_targets) { mt.positions.resize(base); mt.normals.resize(base); }
                for (std::size_t v = 0; v < pos.count; ++v) {
                    for (std::size_t t = 0; t < mesh.morph_targets.size(); ++t) {
                        Vec3 dp{0, 0, 0}, dn{0, 0, 0};
                        if (t < targets.size()) {
                            if (targets[t].pos && v < targets[t].pos->count) {
                                const Accessor& a = *targets[t].pos;
                                const std::uint8_t* ap = a.data + v * a.stride;
                                const std::size_t acs = component_size(a.component_type);
                                dp = bake.transform_dir(Vec3{read_float(ap, a.component_type, a.normalized), read_float(ap + acs, a.component_type, a.normalized), read_float(ap + 2 * acs, a.component_type, a.normalized)});
                            }
                            if (targets[t].nrm && v < targets[t].nrm->count) {
                                const Accessor& a = *targets[t].nrm;
                                const std::uint8_t* ap = a.data + v * a.stride;
                                const std::size_t acs = component_size(a.component_type);
                                Vec4 n4 = normal_m * Vec4{read_float(ap, a.component_type, a.normalized), read_float(ap + acs, a.component_type, a.normalized), read_float(ap + 2 * acs, a.component_type, a.normalized), 0};
                                dn = {n4.x, n4.y, n4.z};
                            }
                        }
                        mesh.morph_targets[t].positions.push_back(dp);
                        mesh.morph_targets[t].normals.push_back(dn);
                    }
                    MeshVertex mv;
                    const std::uint8_t* pp = pos.data + v * pos.stride;
                    std::size_t cs = component_size(pos.component_type);
                    Vec3 p{read_float(pp, pos.component_type, pos.normalized), read_float(pp + cs, pos.component_type, pos.normalized), read_float(pp + 2 * cs, pos.component_type, pos.normalized)};
                    mv.position = bake.transform_point(p);
                    if (jnt && v < jnt->count && v < wgt->count) {
                        SkinVertex sv;
                        std::size_t jcs = component_size(jnt->component_type), wcs = component_size(wgt->component_type);
                        const std::uint8_t* jp = jnt->data + v * jnt->stride;
                        const std::uint8_t* wp = wgt->data + v * wgt->stride;
                        float sum = 0;
                        float w[4];
                        for (int k = 0; k < 4; ++k) {
                            sv.joints[k] = static_cast<std::uint16_t>(read_index(jp + static_cast<std::size_t>(k) * jcs, jnt->component_type));
                            w[k] = read_float(wp + static_cast<std::size_t>(k) * wcs, wgt->component_type, wgt->normalized);
                            sum += w[k];
                        }
                        if (sum <= 0) { w[0] = 1; w[1] = w[2] = w[3] = 0; sum = 1; }
                        sv.weights = {w[0] / sum, w[1] / sum, w[2] / sum, w[3] / sum};
                        mesh.skin_vertices.push_back(sv);
                    } else if (!mesh.skin_vertices.empty()) {
                        mesh.skin_vertices.push_back(SkinVertex{});
                    }
                    if (nrm && v < nrm->count) {
                        const std::uint8_t* np = nrm->data + v * nrm->stride;
                        std::size_t ncs = component_size(nrm->component_type);
                        Vec3 n{read_float(np, nrm->component_type, nrm->normalized), read_float(np + ncs, nrm->component_type, nrm->normalized), read_float(np + 2 * ncs, nrm->component_type, nrm->normalized)};
                        Vec4 n4 = normal_m * Vec4{n.x, n.y, n.z, 0};
                        mv.normal = normalize(Vec3{n4.x, n4.y, n4.z});
                    } else {
                        mv.normal = {0, 1, 0};
                    }
                    if (tan && v < tan->count) {
                        // A direction: the bake's turn and scale, not its normal matrix; a mirroring
                        // bake turns the bitangent's side over.
                        const std::uint8_t* tp = tan->data + v * tan->stride;
                        const std::size_t tcs = component_size(tan->component_type);
                        Vec4 t4 = bake * Vec4{read_float(tp, tan->component_type, tan->normalized), read_float(tp + tcs, tan->component_type, tan->normalized), read_float(tp + 2 * tcs, tan->component_type, tan->normalized), 0};
                        const Vec3 t3 = Vec3{t4.x, t4.y, t4.z};
                        if (length(t3) > 1e-8f) {
                            const Vec3 tn = normalize(t3);
                            const float side = read_float(tp + 3 * tcs, tan->component_type, tan->normalized) < 0 ? -1.0f : 1.0f;
                            mv.tangent = {tn.x, tn.y, tn.z, side * (bake_mirrors ? -1.0f : 1.0f)};
                        }
                    }
                    if (uv && v < uv->count) {
                        const std::uint8_t* up = uv->data + v * uv->stride;
                        std::size_t ucs = component_size(uv->component_type);
                        mv.uv = {read_float(up, uv->component_type, uv->normalized), read_float(up + ucs, uv->component_type, uv->normalized)};
                    }
                    if (col && v < col->count) {
                        // Linear, as glTF has it: floats, or normalized bytes or shorts; RGB or RGBA.
                        const std::uint8_t* cp = col->data + v * col->stride;
                        const std::size_t ccs = component_size(col->component_type);
                        const bool norm = col->normalized || col->component_type != 5126;
                        mv.color = {read_float(cp, col->component_type, norm), read_float(cp + ccs, col->component_type, norm), read_float(cp + 2 * ccs, col->component_type, norm),
                                    col->components >= 4 ? read_float(cp + 3 * ccs, col->component_type, norm) : 1.0f};
                    }
                    mesh.vertices.push_back(mv);
                }
                Submesh sm;
                sm.skin = jnt ? skin_index : -1;
                sm.node = part ? ni : -1;
                sm.origin = ni;
                sm.first_index = static_cast<std::uint32_t>(mesh.indices.size());
                if (prim.contains("indices")) {
                    POCKET_TRY(idx, g.accessor(prim["indices"].get<int>()));
                    for (std::size_t i = 0; i < idx.count; ++i) {
                        std::uint32_t value = read_index(idx.data + i * idx.stride, idx.component_type);
                        if (value >= pos.count) return fail("bad_gltf", "{}: index {} out of range", display_path, value);
                        mesh.indices.push_back(base + value);
                    }
                } else {
                    for (std::size_t i = 0; i < pos.count; ++i) mesh.indices.push_back(base + static_cast<std::uint32_t>(i));
                }
                sm.index_count = static_cast<std::uint32_t>(mesh.indices.size()) - sm.first_index;
                int mat = prim.value("material", -1);
                sm.material = mat >= 0 && mat < static_cast<int>(mesh.materials.size()) - (has_default ? 1 : 0) ? static_cast<std::uint32_t>(mat) : static_cast<std::uint32_t>(default_material);
                if (!nrm) {
                    // Flat normals from the triangles when the file has none.
                    for (std::uint32_t i = sm.first_index; i + 2 < sm.first_index + sm.index_count; i += 3) {
                        MeshVertex& a = mesh.vertices[mesh.indices[i]];
                        MeshVertex& b = mesh.vertices[mesh.indices[i + 1]];
                        MeshVertex& c = mesh.vertices[mesh.indices[i + 2]];
                        Vec3 n = normalize(cross(b.position - a.position, c.position - a.position));
                        a.normal = b.normal = c.normal = n;
                    }
                }
                mesh.submeshes.push_back(sm);
                any_geometry = true;
            }
        }
        for (const Json& c : node.value("children", Json::array())) POCKET_TRY_VOID(visit(c.get<int>(), world, depth + 1));
        return {};
    };
    Mat4 identity;
    for (int r : roots) POCKET_TRY_VOID(visit(r, identity, 0));
    for (auto& mt : mesh.morph_targets) { mt.positions.resize(mesh.vertices.size()); mt.normals.resize(mesh.vertices.size()); }
    if (!mesh.morph_targets.empty()) mesh.default_weights.resize(mesh.morph_targets.size(), 0.0f);
    // A file of clips alone (an animation library's) is read for them; it is not a model to draw.
    if (!any_geometry && mesh.animations.empty()) return fail("bad_gltf", "{}: no triangle geometry", display_path);
    if (!mesh.skin_vertices.empty()) mesh.skin_vertices.resize(mesh.vertices.size());
    for (const Skin& s : mesh.skins) {
        for (const SkinVertex& sv : mesh.skin_vertices) {
            for (std::uint16_t j : sv.joints) {
                if (j >= s.joints.size() && mesh.skins.size() == 1) return fail("bad_gltf", "{}: a vertex refers to joint {} of a {}-joint skin", display_path, j, s.joints.size());
            }
        }
    }
    // The bounds, with the moving parts where their nodes rest.
    bool first = true;
    for (const Submesh& sm : mesh.submeshes) {
        const Mat4 place = mesh.rest_global(sm.node);
        for (std::uint32_t i = sm.first_index; i < sm.first_index + sm.index_count && i < mesh.indices.size(); ++i) {
            const Vec3 pos = sm.node >= 0 ? place.transform_point(mesh.vertices[mesh.indices[i]].position) : mesh.vertices[mesh.indices[i]].position;
            if (first) { mesh.aabb_min = mesh.aabb_max = pos; first = false; continue; }
            mesh.aabb_min = {std::min(mesh.aabb_min.x, pos.x), std::min(mesh.aabb_min.y, pos.y), std::min(mesh.aabb_min.z, pos.z)};
            mesh.aabb_max = {std::max(mesh.aabb_max.x, pos.x), std::max(mesh.aabb_max.y, pos.y), std::max(mesh.aabb_max.z, pos.z)};
        }
    }
    // Lights (KHR_lights_punctual) and cameras the nodes carry.
    if (g.doc.contains("extensions") && g.doc["extensions"].is_object() && g.doc["extensions"].contains("KHR_lights_punctual")) {
        for (const Json& lj : g.doc["extensions"]["KHR_lights_punctual"].value("lights", Json::array())) {
            LightDef l;
            l.name = lj.value("name", "light" + std::to_string(mesh.lights.size()));
            const std::string type = lj.value("type", "point");
            l.type = type == "directional" ? 0 : type == "spot" ? 2 : 1;
            if (lj.contains("color") && lj["color"].size() == 3) l.color = {lj["color"][0].get<float>(), lj["color"][1].get<float>(), lj["color"][2].get<float>()};
            l.intensity = lj.value("intensity", 1.0f);
            l.range = lj.value("range", 0.0f);
            if (lj.contains("spot")) {
                l.inner_cone = lj["spot"].value("innerConeAngle", 0.0f);
                l.outer_cone = lj["spot"].value("outerConeAngle", 0.785398f);
            }
            mesh.lights.push_back(l);
        }
    }
    for (const Json& cj : g.doc.value("cameras", Json::array())) {
        CameraDef c;
        c.name = cj.value("name", "camera" + std::to_string(mesh.cameras.size()));
        c.orthographic = cj.value("type", "perspective") == "orthographic";
        const Json& d = cj.value(c.orthographic ? "orthographic" : "perspective", Json::object());
        c.yfov = d.value("yfov", 0.8f);
        c.ymag = d.value("ymag", 5.0f);
        c.znear = d.value("znear", 0.1f);
        c.zfar = d.value("zfar", 1000.0f);
        mesh.cameras.push_back(c);
    }
    return mesh;
}

Result<Image> decode_image(const std::string& bytes, const std::string& display_path) {
    int w = 0, h = 0, channels = 0;
    const auto* data = reinterpret_cast<const stbi_uc*>(bytes.data());
    if (stbi_is_hdr_from_memory(data, static_cast<int>(bytes.size()))) {
        // Light levels beyond white (a sky, a panorama): kept as floats, and an 8-bit view beside them.
        float* f = stbi_loadf_from_memory(data, static_cast<int>(bytes.size()), &w, &h, &channels, 4);
        if (!f) return fail("bad_image", "{}: {}", display_path, stbi_failure_reason() ? stbi_failure_reason() : "cannot decode");
        Image img;
        img.path = display_path;
        img.width = static_cast<std::uint32_t>(w);
        img.height = static_cast<std::uint32_t>(h);
        const std::size_t n = static_cast<std::size_t>(w) * static_cast<std::size_t>(h) * 4;
        img.hdr.assign(f, f + n);
        stbi_image_free(f);
        img.rgba.resize(n);
        for (std::size_t i = 0; i < n; ++i) {
            if (i % 4 == 3) { img.rgba[i] = 255; continue; }
            const float c = std::clamp(img.hdr[i], 0.0f, 1.0f);
            const float e = c <= 0.0031308f ? c * 12.92f : 1.055f * std::pow(c, 1.0f / 2.4f) - 0.055f;
            img.rgba[i] = static_cast<std::uint8_t>(std::lround(e * 255.0f));
        }
        return img;
    }
    stbi_uc* pixels = stbi_load_from_memory(reinterpret_cast<const stbi_uc*>(bytes.data()), static_cast<int>(bytes.size()), &w, &h, &channels, 4);
    if (!pixels) return fail("bad_image", "{}: {}", display_path, stbi_failure_reason() ? stbi_failure_reason() : "cannot decode");
    Image img;
    img.path = display_path;
    img.width = static_cast<std::uint32_t>(w);
    img.height = static_cast<std::uint32_t>(h);
    img.rgba.assign(pixels, pixels + static_cast<std::size_t>(w) * static_cast<std::size_t>(h) * 4);
    stbi_image_free(pixels);
    return img;
}

AssetStore::AssetStore(std::filesystem::path project_dir) : project_dir_(std::move(project_dir)) {}

Result<std::filesystem::path> AssetStore::resolve(const std::string& path) const {
    if (path.empty()) return fail("bad_args", "asset path is empty");
    std::filesystem::path base = std::filesystem::weakly_canonical(project_dir_);
    std::filesystem::path full = std::filesystem::weakly_canonical(base / path);
    auto [bi, fi] = std::mismatch(base.begin(), base.end(), full.begin(), full.end());
    if (bi != base.end()) return fail("forbidden", "{} is outside the project directory", path);
    if (!std::filesystem::exists(full)) return fail("no_such_asset", "{} does not exist (looked in {})", path, full.string());
    return full;
}

bool AssetStore::has_mesh(const std::string& path) const { return meshes_.contains(path); }

void AssetStore::put_mesh(const std::string& path, Mesh mesh) {
    mesh.path = path;
    meshes_[path] = std::make_unique<Mesh>(std::move(mesh));
    failures_.erase(path);
}

void AssetStore::forget_mesh(const std::string& path) {
    meshes_.erase(path);
}

Result<const Mesh*> AssetStore::mesh(const std::string& path) {
    if (auto it = meshes_.find(path); it != meshes_.end()) return it->second.get();
    if (auto f = failures_.find("mesh:" + path); f != failures_.end()) return fail("bad_asset", "{}", f->second);
    POCKET_TRY(full, resolve(path));
    std::string ext = full.extension().string();
    std::transform(ext.begin(), ext.end(), ext.begin(), [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
    Result<Mesh> parsed = fail("bad_asset", "unread");
    if (ext == ".obj") {
        POCKET_TRY(text, fs::read_text(full));
        parsed = parse_obj(text, path, [this](const std::string& p) -> Result<std::string> {
            POCKET_TRY(f, resolve(p));
            return fs::read_text(f);
        });
    } else if (ext == ".stl") {
        POCKET_TRY(bytes, fs::read_bytes(full));
        parsed = parse_stl(std::string(bytes.begin(), bytes.end()), path);
    } else if (ext == ".ply") {
        POCKET_TRY(bytes, fs::read_bytes(full));
        parsed = parse_ply(std::string(bytes.begin(), bytes.end()), path);
    } else if (blender_format(ext)) {
        // Through Blender into glTF (cached by content), then read like any glTF.
        auto glb = converted_glb(path, full, false);
        if (!glb) {
            failures_["mesh:" + path] = glb.error().message;
            return fail(glb.error());
        }
        POCKET_TRY(bytes, fs::read_bytes(*glb));
        parsed = parse_gltf(std::string(bytes.begin(), bytes.end()), glb->parent_path(), path);
        if (parsed) {
            parsed->importer = "blender";
            parsed->converted = std::filesystem::relative(*glb, project_dir_).generic_string();
        }
    } else {
        POCKET_TRY(bytes, fs::read_bytes(full));
        parsed = parse_gltf(std::string(bytes.begin(), bytes.end()), full.parent_path(), path);
    }
    if (!parsed) {
        failures_["mesh:" + path] = parsed.error().message;
        return fail(parsed.error());
    }
    if (parsed->vertices.empty()) {
        // Clips and a skeleton, nothing to draw: kept for animation.library, refused as a model.
        animation_files_[path] = std::make_unique<Mesh>(std::move(*parsed));
        failures_["mesh:" + path] = path + ": no triangle geometry (a file of clips: animation.library puts them on a model)";
        return fail("bad_gltf", "{}", failures_["mesh:" + path]);
    }
    fill_tangents(*parsed);
    auto owned = std::make_unique<Mesh>(std::move(*parsed));
    const Mesh* raw = owned.get();
    meshes_[path] = std::move(owned);
    version_++;
    log::info("assets", "loaded mesh {} ({} vertices, {} triangles, {} materials)", path, raw->vertices.size(), raw->indices.size() / 3, raw->materials.size());
    return raw;
}

Result<const TileMap*> AssetStore::tilemap(const std::string& path) {
    if (auto it = tilemaps_.find(path); it != tilemaps_.end()) return it->second.get();
    if (auto f = failures_.find("tilemap:" + path); f != failures_.end()) return fail("bad_asset", "{}", f->second);
    POCKET_TRY(full, resolve(path));
    POCKET_TRY(text, fs::read_text(full));
    auto parsed = parse_tilemap(text, path);
    if (!parsed) {
        failures_["tilemap:" + path] = parsed.error().message;
        return fail(parsed.error());
    }
    auto owned = std::make_unique<TileMap>(std::move(*parsed));
    const TileMap* raw = owned.get();
    tilemaps_[path] = std::move(owned);
    version_++;
    log::info("assets", "loaded tile map {} ({}x{} tiles, {} layers, {} tilesets)", path, raw->width, raw->height, raw->layers.size(), raw->tilesets.size());
    return raw;
}

Result<TileMap*> AssetStore::tilemap_mut(const std::string& path) {
    POCKET_TRY(loaded, tilemap(path));
    (void)loaded;
    return tilemaps_.at(path).get();
}

Result<TileMap*> AssetStore::copy_tilemap(const std::string& path, const std::string& name) {
    if (name.empty()) return fail("bad_args", "a map copy needs a name");
    if (tilemaps_.contains(name)) return fail("duplicate_map", "a map named {} is already loaded", name);
    POCKET_TRY(src, tilemap(path));
    auto owned = std::make_unique<TileMap>(*src);
    owned->path = name;
    owned->file = false;
    TileMap* raw = owned.get();
    tilemaps_[name] = std::move(owned);
    version_++;
    return raw;
}

TileMap* AssetStore::put_tilemap(TileMap map) {
    if (auto it = tilemaps_.find(map.path); it != tilemaps_.end()) {
        // Replacing a map something may have drawn: its layers count on from the old ones'
        // revisions, so what was built from those is built again.
        std::uint64_t top = it->second->revision;
        for (const TileLayer& l : it->second->layers) top = std::max(top, l.revision);
        for (TileLayer& l : map.layers) l.revision = std::max(l.revision, top + 1);
        map.revision = std::max(map.revision, top + 1);
    }
    auto owned = std::make_unique<TileMap>(std::move(map));
    TileMap* raw = owned.get();
    tilemaps_[raw->path] = std::move(owned);
    version_++;
    return raw;
}

std::vector<const TileMap*> AssetStore::tilemaps() const {
    std::vector<const TileMap*> all;
    for (const auto& [path, m] : tilemaps_) all.push_back(m.get());
    return all;
}

Result<const Image*> AssetStore::image(const std::string& path) {
    if (auto it = images_.find(path); it != images_.end()) return it->second.get();
    if (auto f = failures_.find("image:" + path); f != failures_.end()) return fail("bad_asset", "{}", f->second);
    Result<Image> decoded = fail("bad_asset", "unresolved");
    if (std::size_t hash = path.find("#image"); hash != std::string::npos) {
        // An image embedded in a glTF buffer view.
        std::string gltf_path = path.substr(0, hash);
        int index = std::atoi(path.c_str() + hash + 6);
        POCKET_TRY(resolved, resolve(gltf_path));
        // A Blender-read file's images live in the glTF it became.
        std::filesystem::path full = resolved;
        if (auto c = converted_.find(gltf_path); c != converted_.end()) full = c->second;
        POCKET_TRY(bytes, fs::read_bytes(full));
        std::string text(bytes.begin(), bytes.end());
        // Re-read the container to find the buffer view (cheap; embedded images are rare).
        std::string json_text = text, bin;
        if (text.size() >= 12 && text.compare(0, 4, "glTF") == 0) {
            std::size_t pos = 12;
            json_text.clear();
            while (pos + 8 <= text.size()) {
                std::uint32_t len, type;
                std::memcpy(&len, text.data() + pos, 4);
                std::memcpy(&type, text.data() + pos + 4, 4);
                pos += 8;
                if (pos + len > text.size()) break;
                if (type == 0x4E4F534Au) json_text = text.substr(pos, len);
                else if (type == 0x004E4942u) bin = text.substr(pos, len);
                pos += len;
            }
        }
        Json doc = Json::parse(json_text, nullptr, false);
        if (doc.is_discarded() || !doc.contains("images") || index < 0 || index >= static_cast<int>(doc["images"].size())) decoded = fail("bad_asset", "{}: no embedded image {}", gltf_path, index);
        else {
            const Json& img = doc["images"][static_cast<std::size_t>(index)];
            int bv = img.value("bufferView", -1);
            if (bv < 0 || bv >= static_cast<int>(doc.value("bufferViews", Json::array()).size())) decoded = fail("bad_asset", "{}: image {} has no bufferView", gltf_path, index);
            else {
                const Json& view = doc["bufferViews"][static_cast<std::size_t>(bv)];
                std::size_t off = view.value("byteOffset", 0), len = view.value("byteLength", 0);
                if (off + len > bin.size()) decoded = fail("bad_asset", "{}: image {} view out of range", gltf_path, index);
                else decoded = decode_image(bin.substr(off, len), path);
            }
        }
    } else {
        POCKET_TRY(full, resolve(path));
        POCKET_TRY(bytes, fs::read_bytes(full));
        decoded = decode_image(std::string(bytes.begin(), bytes.end()), path);
    }
    if (!decoded) {
        failures_["image:" + path] = decoded.error().message;
        return fail(decoded.error());
    }
    auto owned = std::make_unique<Image>(std::move(*decoded));
    const Image* raw = owned.get();
    images_[path] = std::move(owned);
    version_++;
    log::info("assets", "loaded image {} ({}x{})", path, raw->width, raw->height);
    return raw;
}

Result<std::filesystem::path> AssetStore::converted_glb(const std::string& path, const std::filesystem::path& full, bool force, Conversion* report) {
    const std::filesystem::path out = project_dir_ / ".imported" / (path + ".glb");
    const std::string blender = find_blender(blender_config_);
    POCKET_TRY(conv, convert_with_blender(full, out, blender, force));
    if (!conv.cached) log::info("assets", "imported {} through Blender in {:.1f} s", path, conv.seconds);
    converted_[path] = out;
    if (report) *report = conv;
    return out;
}

Result<Json> AssetStore::add_clips(const std::string& model, const std::string& source, bool root_translation_only) {
    const Mesh* src = nullptr;
    if (auto m = mesh(source)) src = *m;
    else if (auto it = animation_files_.find(source); it != animation_files_.end()) src = it->second.get();
    else return fail(m.error());
    POCKET_TRY(target_const, mesh(model));
    (void)target_const;
    Mesh* dst = meshes_[model].get();
    if (!dst) return fail("bad_asset", "{} is not loaded", model);
    auto bare = [](std::string_view n) {
        const std::size_t c = n.rfind(':');
        return std::string(c == std::string_view::npos ? n : n.substr(c + 1));
    };
    std::map<std::string, int> by_name;
    for (std::size_t i = 0; i < dst->nodes.size(); ++i) by_name.try_emplace(dst->nodes[i].name, static_cast<int>(i));
    for (std::size_t i = 0; i < dst->nodes.size(); ++i) by_name.try_emplace(bare(dst->nodes[i].name), static_cast<int>(i));
    // The skeleton's root: the first skin's first joint, else the first node without a parent.
    int root = -1;
    if (!dst->skins.empty() && !dst->skins[0].joints.empty()) root = dst->skins[0].joints[0];
    for (std::size_t i = 0; root < 0 && i < dst->nodes.size(); ++i) if (dst->nodes[i].parent < 0) root = static_cast<int>(i);
    Json added = Json::array();
    std::size_t dropped = 0;
    for (const AnimationClip& c : src->animations) {
        AnimationClip out;
        out.name = src->animations.size() == 1 ? std::filesystem::path(source).stem().string() : c.name;
        out.duration = c.duration;
        for (const AnimationChannel& ch : c.channels) {
            if (ch.node < 0 || ch.node >= static_cast<int>(src->nodes.size()) || ch.path == 3) { ++dropped; continue; }
            const std::string& name = src->nodes[static_cast<std::size_t>(ch.node)].name;
            auto it = by_name.find(name);
            if (it == by_name.end()) it = by_name.find(bare(name));
            if (it == by_name.end()) { ++dropped; continue; }
            if (root_translation_only && ch.path == 0 && it->second != root) { ++dropped; continue; }
            AnimationChannel mapped = ch;
            mapped.node = it->second;
            out.channels.push_back(std::move(mapped));
        }
        if (out.channels.empty()) continue;
        auto same = std::find_if(dst->animations.begin(), dst->animations.end(), [&](const AnimationClip& a) { return a.name == out.name; });
        added.push_back(Json{{"name", out.name}, {"duration", out.duration}, {"channels", out.channels.size()}});
        if (same != dst->animations.end()) *same = std::move(out);
        else dst->animations.push_back(std::move(out));
    }
    version_++;
    return Json{{"model", model}, {"source", source}, {"clips", added}, {"channels_left_out", dropped}};
}

Result<Json> AssetStore::import(const std::string& path, bool force) {
    POCKET_TRY(full, resolve(path));
    std::string ext = full.extension().string();
    std::transform(ext.begin(), ext.end(), ext.begin(), [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
    Json j;
    j["path"] = path;
    if (blender_format(ext)) {
        Conversion conv;
        POCKET_TRY(glb, converted_glb(path, full, force, &conv));
        j["importer"] = "blender";
        j["converted"] = std::filesystem::relative(glb, project_dir_).generic_string();
        j["cached"] = conv.cached;
        j["seconds"] = conv.seconds;
        j["blender"] = conv.blender;
    } else {
        j["importer"] = ext == ".obj" ? "obj" : ext == ".stl" ? "stl" : ext == ".ply" ? "ply" : "gltf";
    }
    invalidate(path);
    POCKET_TRY(mesh, this->mesh(path));
    j["mesh"] = mesh->describe();
    return j;
}

void AssetStore::invalidate(const std::string& path) {
    meshes_.erase(path);
    animation_files_.erase(path);
    images_.erase(path);
    tilemaps_.erase(path);
    failures_.erase("mesh:" + path);
    failures_.erase("image:" + path);
    failures_.erase("tilemap:" + path);
    version_++;
}

void AssetStore::invalidate_all() {
    meshes_.clear();
    animation_files_.clear();
    images_.clear();
    for (auto it = tilemaps_.begin(); it != tilemaps_.end();) it = it->second->file ? tilemaps_.erase(it) : std::next(it);   // copies have no file to reload from
    failures_.clear();
    version_++;
}

Json AssetStore::list() const {
    Json files = Json::array();
    std::filesystem::path dir = project_dir_ / "assets";
    if (std::filesystem::is_directory(dir)) {
        std::vector<std::filesystem::path> paths;
        for (auto& entry : std::filesystem::recursive_directory_iterator(dir)) if (entry.is_regular_file()) paths.push_back(entry.path());
        std::sort(paths.begin(), paths.end());
        for (const auto& p : paths) {
            std::string ext = p.extension().string();
            std::transform(ext.begin(), ext.end(), ext.begin(), [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
            const bool model = ext == ".glb" || ext == ".gltf" || ext == ".obj" || ext == ".stl" || ext == ".ply" || blender_format(ext);
            std::string kind = model ? "mesh" : ext == ".png" || ext == ".jpg" || ext == ".jpeg" || ext == ".bmp" || ext == ".tga" || ext == ".hdr" ? "image" : ext == ".tmj" ? "tilemap" : ext == ".wav" || ext == ".ogg" || ext == ".mp3" ? "audio" : ext == ".mtl" ? "material" : "other";

            Json f;
            f["path"] = std::filesystem::relative(p, project_dir_).generic_string();
            f["kind"] = kind;
            if (model) f["importer"] = blender_format(ext) ? "blender" : ext == ".obj" ? "obj" : ext == ".stl" ? "stl" : ext == ".ply" ? "ply" : "gltf";
            f["bytes"] = std::filesystem::file_size(p);
            f["loaded"] = kind == "mesh" ? meshes_.contains(f["path"].get<std::string>()) : kind == "image" ? images_.contains(f["path"].get<std::string>()) : kind == "tilemap" ? tilemaps_.contains(f["path"].get<std::string>()) : false;
            files.push_back(f);
        }
    }
    // The project's scripts too (scripts/ and scenarios/), so an editor can open them.
    for (const char* sub : {"scripts", "scenarios"}) {
        std::filesystem::path sdir = project_dir_ / sub;
        if (!std::filesystem::is_directory(sdir)) continue;
        std::vector<std::filesystem::path> paths;
        for (auto& entry : std::filesystem::recursive_directory_iterator(sdir)) if (entry.is_regular_file()) paths.push_back(entry.path());
        std::sort(paths.begin(), paths.end());
        for (const auto& p : paths) {
            std::string ext = p.extension().string();
            std::transform(ext.begin(), ext.end(), ext.begin(), [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
            if (ext != ".ts" && ext != ".tsx" && ext != ".js") continue;
            Json f;
            f["path"] = std::filesystem::relative(p, project_dir_).generic_string();
            f["kind"] = "script";
            f["bytes"] = std::filesystem::file_size(p);
            f["loaded"] = false;
            files.push_back(f);
        }
    }
    return files;
}

Json AssetStore::describe(const std::string& path) {
    std::string ext = std::filesystem::path(path).extension().string();
    std::transform(ext.begin(), ext.end(), ext.begin(), [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
    Json j;
    j["path"] = path;
    if (ext == ".glb" || ext == ".gltf" || ext == ".obj" || ext == ".stl" || ext == ".ply" || blender_format(ext)) {
        auto m = mesh(path);
        if (!m) { j["error"] = m.error().to_string(); return j; }
        j = (*m)->describe();
        j["kind"] = "mesh";
    } else if (ext == ".tmj") {
        auto t = tilemap(path);
        if (!t) { j["error"] = t.error().to_string(); return j; }
        j = (*t)->describe();
    } else {
        auto i = image(path);
        if (!i) { j["error"] = i.error().to_string(); return j; }
        j = (*i)->describe();
        j["kind"] = "image";
    }
    return j;
}

Json AssetStore::stats() const {
    Json j;
    j["meshes"] = meshes_.size();
    j["images"] = images_.size();
    j["failures"] = failures_.size();
    j["version"] = version_;
    return j;
}

}  // namespace pocket::assets
