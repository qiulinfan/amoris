// Engine version, assembled from the defines in pocket.toml so there is one source of truth.
#pragma once

#define POCKET_STRINGIZE_IMPL(x) #x
#define POCKET_STRINGIZE(x) POCKET_STRINGIZE_IMPL(x)

#ifndef POCKET_VERSION_MAJOR
#define POCKET_VERSION_MAJOR 0
#endif
#ifndef POCKET_VERSION_MINOR
#define POCKET_VERSION_MINOR 0
#endif
#ifndef POCKET_VERSION_PATCH
#define POCKET_VERSION_PATCH 0
#endif

#define POCKET_VERSION POCKET_STRINGIZE(POCKET_VERSION_MAJOR) "." POCKET_STRINGIZE(POCKET_VERSION_MINOR) "." POCKET_STRINGIZE(POCKET_VERSION_PATCH)

namespace pocket {
constexpr const char* kVersion = POCKET_VERSION;
}
