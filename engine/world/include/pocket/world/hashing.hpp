#pragma once

#include <functional>

#include <pocket/core/hash.hpp>

namespace pocket::world {

// The hasher generated component code writes into. Same contract as pocket::StateHasher, and an
// entity a component refers to is hashed by the key `key_of` gives it (its place in the world, its
// path) when set: ids depend on how many entities the build made before the scene (a web build
// numbers the same scene differently from a native one), and two peers of a lockstep game must hash
// the same world alike.
struct StateHasherRef : pocket::StateHasher {
    std::function<std::uint64_t(std::uint64_t)> key_of;

    void entity(std::uint64_t id) { u64(id != 0 && key_of ? key_of(id) : id); }
};

}  // namespace pocket::world
