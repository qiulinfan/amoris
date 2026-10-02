// The ICU data in Bun's JavaScriptCore build keeps its display-name items (currencies, languages,
// regions, units, zones) as zstd frames; its patched udata.cpp calls this hook on every item it
// looks up and uses what it returns. Frames are decompressed once and kept for the process.
#define ZSTD_STATIC_LINKING_ONLY  // ZSTD_createDDict_byReference
#include <zstd.h>

#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <mutex>
#include <unordered_map>

extern "C" const unsigned char bun_icu_zstd_dict[];
extern "C" const unsigned int bun_icu_zstd_dict_size;

namespace {

struct Decompressor {
    std::mutex lock;
    std::unordered_map<const void*, std::pair<void*, int32_t>> cache;
    ZSTD_DCtx* dctx = ZSTD_createDCtx();
    ZSTD_DDict* ddict = bun_icu_zstd_dict_size ? ZSTD_createDDict_byReference(bun_icu_zstd_dict, bun_icu_zstd_dict_size) : nullptr;

    const void* get(const void* p, int32_t* length) {
        std::lock_guard guard(lock);
        if (auto it = cache.find(p); it != cache.end()) {
            *length = it->second.second;
            return it->second.first;
        }
        size_t bound = *length > 0 ? static_cast<size_t>(*length) : size_t(1) << 20;
        size_t compressed = ZSTD_findFrameCompressedSize(p, bound);
        if (ZSTD_isError(compressed)) return p;
        unsigned long long size = ZSTD_getFrameContentSize(p, compressed);
        if (size == ZSTD_CONTENTSIZE_UNKNOWN || size == ZSTD_CONTENTSIZE_ERROR) return p;
        void* out = _aligned_malloc((static_cast<size_t>(size) + 15) & ~size_t(15), 16);
        if (!out) return p;
        size_t r = ddict ? ZSTD_decompress_usingDDict(dctx, out, size, p, compressed, ddict) : ZSTD_decompressDCtx(dctx, out, size, p, compressed);
        if (ZSTD_isError(r)) {
            _aligned_free(out);
            return p;
        }
        cache.emplace(p, std::pair{out, static_cast<int32_t>(size)});
        *length = static_cast<int32_t>(size);
        return out;
    }
};

}  // namespace

extern "C" const void* bun_icu_maybe_decompress(const void* p, int32_t* length) {
    if (!p) return p;
    uint32_t magic;
    std::memcpy(&magic, p, sizeof magic);
    if (magic != ZSTD_MAGICNUMBER) return p;
    static Decompressor* d = new Decompressor;  // never destroyed: ICU may look items up during exit
    return d->get(p, length);
}
