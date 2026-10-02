#define STB_IMAGE_IMPLEMENTATION
#define STB_IMAGE_WRITE_IMPLEMENTATION
#define STBI_NO_STDIO_CALLBACKS_UNUSED 1
#include "stb_image.h"
#include "stb_image_write.h"
#define STB_VORBIS_NO_STDIO
#include "stb_vorbis.c"
#define DR_MP3_NO_STDIO
#define DR_MP3_IMPLEMENTATION
#include "dr_mp3.h"
#define DR_FLAC_NO_STDIO
#define DR_FLAC_IMPLEMENTATION
#include "dr_flac.h"
// nanosvg (zlib): SVG images parsed and rasterized for the assets module.
#define NANOSVG_IMPLEMENTATION
#include "nanosvg.h"
#define NANOSVGRAST_IMPLEMENTATION
#include "nanosvgrast.h"
