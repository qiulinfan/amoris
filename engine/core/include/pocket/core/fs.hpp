// Filesystem helpers with Result errors.
#pragma once

#include <pocket/core/result.hpp>

#include <filesystem>
#include <string>
#include <vector>

namespace pocket::fs {

Result<std::string> read_text(const std::filesystem::path& path);
Result<std::vector<std::uint8_t>> read_bytes(const std::filesystem::path& path);
Status write_text(const std::filesystem::path& path, std::string_view text);
Status write_bytes(const std::filesystem::path& path, const void* data, std::size_t size);
Status ensure_dir(const std::filesystem::path& path);

}  // namespace pocket::fs
