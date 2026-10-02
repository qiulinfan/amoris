// Pocket's door into the Basis Universal transcoder (compiled as C++20 beside it, so the engine's
// C++26 never sees its headers): a KTX2 texture's first level as RGBA8.
#pragma once

#include <cstddef>
#include <cstdint>
#include <string>
#include <vector>

namespace pocket_ktx2 {

// Whether `data` starts like a KTX2 file.
bool is_ktx2(const void* data, std::size_t size);

// The first level (layer 0, face 0) of a KTX2 file holding Basis Universal data (ETC1S or UASTC,
// zstd-supercompressed or not) as RGBA8; false with the reason otherwise (HDR textures included).
bool decode_rgba(const void* data, std::size_t size, std::uint32_t& width, std::uint32_t& height, std::vector<std::uint8_t>& rgba, std::string& why);

}  // namespace pocket_ktx2
