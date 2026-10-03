// Running another program to its end: the pocket tool for `project.apply`, Blender for imports.
// One implementation per host (posix_spawn, CreateProcessW); there are none in the web build.
#pragma once

#include <pocket/core/result.hpp>

#include <filesystem>
#include <string>
#include <vector>

namespace pocket::process {

// Runs args[0] with args (each passed as it is, whatever spaces or quotes it holds), its standard
// output and error both written to `log` (made anew), and waits for it: its exit code, or -1 when
// it did not exit by itself. Fails ("unavailable") when it cannot be started.
Result<int> run(const std::vector<std::string>& args, const std::filesystem::path& log);

// This process's id, for names that must not clash with another runtime's.
[[nodiscard]] unsigned long id();

}  // namespace pocket::process
