#include <pocket/core/fs.hpp>

#include <fstream>
#include <sstream>

namespace pocket::fs {

Result<std::string> read_text(const std::filesystem::path& path) {
    std::ifstream in(path, std::ios::binary);
    if (!in) return fail("file_not_found", "cannot open {}", path.string());
    std::ostringstream ss;
    ss << in.rdbuf();
    return ss.str();
}

Result<std::vector<std::uint8_t>> read_bytes(const std::filesystem::path& path) {
    std::ifstream in(path, std::ios::binary | std::ios::ate);
    if (!in) return fail("file_not_found", "cannot open {}", path.string());
    auto size = in.tellg();
    in.seekg(0);
    std::vector<std::uint8_t> out(static_cast<std::size_t>(size));
    if (size > 0 && !in.read(reinterpret_cast<char*>(out.data()), size)) {
        return fail("io_error", "short read on {}", path.string());
    }
    return out;
}

Status write_text(const std::filesystem::path& path, std::string_view text) {
    return write_bytes(path, text.data(), text.size());
}

Status write_bytes(const std::filesystem::path& path, const void* data, std::size_t size) {
    if (path.has_parent_path()) {
        POCKET_TRY_VOID(ensure_dir(path.parent_path()));
    }
    std::ofstream out(path, std::ios::binary | std::ios::trunc);
    if (!out) return fail("io_error", "cannot write {}", path.string());
    out.write(static_cast<const char*>(data), static_cast<std::streamsize>(size));
    if (!out) return fail("io_error", "short write on {}", path.string());
    return {};
}

Status ensure_dir(const std::filesystem::path& path) {
    std::error_code ec;
    std::filesystem::create_directories(path, ec);
    if (ec && !std::filesystem::is_directory(path)) return fail("io_error", "cannot create directory {}: {}", path.string(), ec.message());
    return {};
}

}  // namespace pocket::fs
