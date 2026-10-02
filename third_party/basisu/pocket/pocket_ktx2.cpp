#include "pocket_ktx2.h"

#include "../transcoder/basisu_transcoder.h"

#include <cstring>

namespace pocket_ktx2 {

bool is_ktx2(const void* data, std::size_t size) {
    static const unsigned char magic[12] = {0xAB, 'K', 'T', 'X', ' ', '2', '0', 0xBB, '\r', '\n', 0x1A, '\n'};
    return size >= 12 && std::memcmp(data, magic, 12) == 0;
}

bool decode_rgba(const void* data, std::size_t size, std::uint32_t& width, std::uint32_t& height, std::vector<std::uint8_t>& rgba, std::string& why) {
    static const bool ready = [] { basist::basisu_transcoder_init(); return true; }();
    (void)ready;
    basist::ktx2_transcoder t;
    if (!t.init(data, static_cast<std::uint32_t>(size)) || !t.start_transcoding()) {
        why = "not a KTX2 file with Basis Universal data the transcoder can read";
        return false;
    }
    if (t.is_hdr()) {
        why = "an HDR KTX2 texture, which is not read yet";
        return false;
    }
    width = t.get_width();
    height = t.get_height();
    rgba.assign(static_cast<std::size_t>(width) * height * 4, 0);
    if (!t.transcode_image_level(0, 0, 0, rgba.data(), width * height, basist::transcoder_texture_format::cTFRGBA32, 0, width, height)) {
        why = "its first level did not transcode";
        return false;
    }
    return true;
}

}  // namespace pocket_ktx2
