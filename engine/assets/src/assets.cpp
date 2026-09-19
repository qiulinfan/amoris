#include <pocket/assets/assets.hpp>

#include <pocket/core/fs.hpp>
#include <pocket/core/log.hpp>

#include <stb_image.h>

#include <algorithm>
#include <cmath>
#include <cstring>
#include <functional>
#include <optional>

namespace pocket::assets {

Json Mesh::describe() const {
    Json j;
    j["path"] = path;
    j["vertices"] = vertices.size();
    j["triangles"] = indices.size() / 3;
    j["submeshes"] = submeshes.size();
    j["nodes"] = node_count;
    Json mats = Json::array();
    for (const auto& m : materials) {
        Json mj;
        mj["name"] = m.name;
        mj["base_color"] = Json::array({m.base_color.x, m.base_color.y, m.base_color.z, m.base_color.w});
        if (!m.texture.empty()) mj["texture"] = m.texture;
        mj["metallic"] = m.metallic;
        mj["roughness"] = m.roughness;
        if (m.double_sided) mj["double_sided"] = true;
        mats.push_back(mj);
    }
    j["materials"] = mats;
    j["aabb"] = Json{{"min", Json::array({aabb_min.x, aabb_min.y, aabb_min.z})}, {"max", Json::array({aabb_max.x, aabb_max.y, aabb_max.z})}};
    return j;
}

Json Image::describe() const {
    Json j;
    j["path"] = path;
    j["width"] = width;
    j["height"] = height;
    j["bytes"] = rgba.size();
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
    for (const Json& m : g.doc.value("materials", Json::array())) {
        Material mat;
        mat.name = m.value("name", "");
        mat.double_sided = m.value("doubleSided", false);
        if (m.contains("pbrMetallicRoughness")) {
            const Json& pbr = m["pbrMetallicRoughness"];
            if (pbr.contains("baseColorFactor") && pbr["baseColorFactor"].size() == 4) {
                mat.base_color = {pbr["baseColorFactor"][0].get<float>(), pbr["baseColorFactor"][1].get<float>(), pbr["baseColorFactor"][2].get<float>(), pbr["baseColorFactor"][3].get<float>()};
            }
            mat.metallic = pbr.value("metallicFactor", 1.0f);
            mat.roughness = pbr.value("roughnessFactor", 1.0f);
            if (pbr.contains("baseColorTexture") && pbr["baseColorTexture"].contains("index")) {
                int ti = pbr["baseColorTexture"]["index"].get<int>();
                const Json& textures = g.doc.value("textures", Json::array());
                if (ti >= 0 && ti < static_cast<int>(textures.size()) && textures[static_cast<std::size_t>(ti)].contains("source")) {
                    int si = textures[static_cast<std::size_t>(ti)]["source"].get<int>();
                    const Json& images = g.doc.value("images", Json::array());
                    if (si >= 0 && si < static_cast<int>(images.size())) {
                        const Json& img = images[static_cast<std::size_t>(si)];
                        if (img.contains("uri") && !img["uri"].get<std::string>().starts_with("data:")) {
                            // Relative to the glTF file; expressed relative to the project like every asset path.
                            std::filesystem::path rel = std::filesystem::path(display_path).parent_path() / img["uri"].get<std::string>();
                            mat.texture = rel.lexically_normal().generic_string();
                        } else if (img.contains("bufferView")) {
                            // Embedded image: exposed as "<file>#image<N>" and decoded on request.
                            mat.texture = display_path + "#image" + std::to_string(si);
                        }
                    }
                }
            }
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
    bool any_geometry = false;
    std::function<Status(int, const Mat4&, int)> visit = [&](int ni, const Mat4& parent, int depth) -> Status {
        if (depth > 64 || ni < 0 || ni >= static_cast<int>(nodes.size())) return {};
        const Json& node = nodes[static_cast<std::size_t>(ni)];
        Mat4 world = parent * node_matrix(node);
        if (node.contains("mesh")) {
            int mi = node["mesh"].get<int>();
            if (mi < 0 || mi >= static_cast<int>(meshes.size())) return fail("bad_gltf", "{}: mesh {} out of range", display_path, mi);
            mesh.node_count++;
            Mat4 normal_m = transpose_of(world.inverse_affine());
            for (const Json& prim : meshes[static_cast<std::size_t>(mi)].value("primitives", Json::array())) {
                int mode = prim.value("mode", 4);
                if (mode != 4) continue;  // triangles only
                const Json& attrs = prim.value("attributes", Json::object());
                if (!attrs.contains("POSITION")) continue;
                POCKET_TRY(pos, g.accessor(attrs["POSITION"].get<int>()));
                std::optional<Accessor> nrm, uv;
                if (attrs.contains("NORMAL")) { POCKET_TRY(a, g.accessor(attrs["NORMAL"].get<int>())); nrm = a; }
                if (attrs.contains("TEXCOORD_0")) { POCKET_TRY(a, g.accessor(attrs["TEXCOORD_0"].get<int>())); uv = a; }
                std::uint32_t base = static_cast<std::uint32_t>(mesh.vertices.size());
                for (std::size_t v = 0; v < pos.count; ++v) {
                    MeshVertex mv;
                    const std::uint8_t* pp = pos.data + v * pos.stride;
                    std::size_t cs = component_size(pos.component_type);
                    Vec3 p{read_float(pp, pos.component_type, pos.normalized), read_float(pp + cs, pos.component_type, pos.normalized), read_float(pp + 2 * cs, pos.component_type, pos.normalized)};
                    mv.position = world.transform_point(p);
                    if (nrm && v < nrm->count) {
                        const std::uint8_t* np = nrm->data + v * nrm->stride;
                        std::size_t ncs = component_size(nrm->component_type);
                        Vec3 n{read_float(np, nrm->component_type, nrm->normalized), read_float(np + ncs, nrm->component_type, nrm->normalized), read_float(np + 2 * ncs, nrm->component_type, nrm->normalized)};
                        Vec4 n4 = normal_m * Vec4{n.x, n.y, n.z, 0};
                        mv.normal = normalize(Vec3{n4.x, n4.y, n4.z});
                    } else {
                        mv.normal = {0, 1, 0};
                    }
                    if (uv && v < uv->count) {
                        const std::uint8_t* up = uv->data + v * uv->stride;
                        std::size_t ucs = component_size(uv->component_type);
                        mv.uv = {read_float(up, uv->component_type, uv->normalized), read_float(up + ucs, uv->component_type, uv->normalized)};
                    }
                    mesh.vertices.push_back(mv);
                }
                Submesh sm;
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
    if (!any_geometry) return fail("bad_gltf", "{}: no triangle geometry", display_path);
    // Merge submeshes that share a material so each material is one draw.
    bool first = true;
    for (const auto& v : mesh.vertices) {
        if (first) { mesh.aabb_min = mesh.aabb_max = v.position; first = false; continue; }
        mesh.aabb_min = {std::min(mesh.aabb_min.x, v.position.x), std::min(mesh.aabb_min.y, v.position.y), std::min(mesh.aabb_min.z, v.position.z)};
        mesh.aabb_max = {std::max(mesh.aabb_max.x, v.position.x), std::max(mesh.aabb_max.y, v.position.y), std::max(mesh.aabb_max.z, v.position.z)};
    }
    return mesh;
}

Result<Image> decode_image(const std::string& bytes, const std::string& display_path) {
    int w = 0, h = 0, channels = 0;
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

Result<const Mesh*> AssetStore::mesh(const std::string& path) {
    if (auto it = meshes_.find(path); it != meshes_.end()) return it->second.get();
    if (auto f = failures_.find("mesh:" + path); f != failures_.end()) return fail("bad_asset", "{}", f->second);
    POCKET_TRY(full, resolve(path));
    POCKET_TRY(bytes, fs::read_bytes(full));
    auto parsed = parse_gltf(std::string(bytes.begin(), bytes.end()), full.parent_path(), path);
    if (!parsed) {
        failures_["mesh:" + path] = parsed.error().message;
        return fail(parsed.error());
    }
    auto owned = std::make_unique<Mesh>(std::move(*parsed));
    const Mesh* raw = owned.get();
    meshes_[path] = std::move(owned);
    version_++;
    log::info("assets", "loaded mesh {} ({} vertices, {} triangles, {} materials)", path, raw->vertices.size(), raw->indices.size() / 3, raw->materials.size());
    return raw;
}

Result<const Image*> AssetStore::image(const std::string& path) {
    if (auto it = images_.find(path); it != images_.end()) return it->second.get();
    if (auto f = failures_.find("image:" + path); f != failures_.end()) return fail("bad_asset", "{}", f->second);
    Result<Image> decoded = fail("bad_asset", "unresolved");
    if (std::size_t hash = path.find("#image"); hash != std::string::npos) {
        // An image embedded in a glTF buffer view.
        std::string gltf_path = path.substr(0, hash);
        int index = std::atoi(path.c_str() + hash + 6);
        POCKET_TRY(full, resolve(gltf_path));
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

void AssetStore::invalidate(const std::string& path) {
    meshes_.erase(path);
    images_.erase(path);
    failures_.erase("mesh:" + path);
    failures_.erase("image:" + path);
    version_++;
}

void AssetStore::invalidate_all() {
    meshes_.clear();
    images_.clear();
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
            std::string kind = ext == ".glb" || ext == ".gltf" ? "mesh" : ext == ".png" || ext == ".jpg" || ext == ".jpeg" || ext == ".bmp" || ext == ".tga" ? "image" : "other";
            Json f;
            f["path"] = std::filesystem::relative(p, project_dir_).generic_string();
            f["kind"] = kind;
            f["bytes"] = std::filesystem::file_size(p);
            f["loaded"] = kind == "mesh" ? meshes_.contains(f["path"].get<std::string>()) : kind == "image" ? images_.contains(f["path"].get<std::string>()) : false;
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
    if (ext == ".glb" || ext == ".gltf") {
        auto m = mesh(path);
        if (!m) { j["error"] = m.error().to_string(); return j; }
        j = (*m)->describe();
        j["kind"] = "mesh";
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
