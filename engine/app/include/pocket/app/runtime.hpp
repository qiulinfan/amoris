// Runtime entry points.
//
// `pocket run <project>` bundles the project's TypeScript and executes pocket_runtime with
// --project/--bundle. Everything here is also usable in-process (tests do that), and every run
// can produce a JSON report describing what happened: ticks run, final observable state, the
// state hash, captures, GPU/script engine facts and the log tail.
#pragma once

#include <pocket/core/json.hpp>
#include <pocket/core/result.hpp>

#include <cstdint>
#include <filesystem>
#include <string>
#include <vector>

namespace pocket::app {

struct Options {
    std::filesystem::path project_dir;
    std::filesystem::path bundle;          // JavaScript produced by `pocket ts`
    std::filesystem::path project_config;  // JSON produced next to the bundle (defaults to <bundle>.project.json)
    bool headless = false;
    bool visible = true;
    int frames = -1;                       // -1: run until the window closes
    std::filesystem::path capture;         // PNG written after the last frame
    bool json = false;                     // print the report as JSON on stdout
    std::uint64_t seed = 1;
    double tick_rate = 60.0;
    int width = 0, height = 0;             // 0: from project config or defaults
    std::string log_level = "info";
    std::filesystem::path log_file;        // JSONL
    std::string title;
    bool inspectable = false;
    bool hash_every_tick = true;
    int serve = -1;                        // -1: no control server; 0: any port; else the port
    bool paused = false;                   // start paused; agents advance with `step`
    std::filesystem::path record;          // write an input journal
    std::filesystem::path replay;          // replay an input journal (headless-exact)
    std::filesystem::path editor_bundle;   // evaluated before the project bundle as script context "editor"
    std::filesystem::path save_dir;        // where save slots live (default: the OS user data directory for the project)
    std::filesystem::path font;            // UI font file; overrides the project config and POCKET_FONT
};

std::string usage();
Result<Options> parse_args(const std::vector<std::string>& args);

// Runs the project and returns the report. Errors that happen before the loop starts come back
// as Error; errors during the run are recorded in the report with ok=false.
Result<Json> run(const Options& options);

// Process entry: parse, run, print, exit code.
int main(int argc, char** argv);

}  // namespace pocket::app
