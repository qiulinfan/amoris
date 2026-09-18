#include <pocket/app/runtime.hpp>

#include <pocket/core/core.hpp>
#include <pocket/platform/platform.hpp>
#include <pocket/rhi/device.hpp>
#include <pocket/script/script_host.hpp>

#include <stb_image_write.h>

#include <cstdio>
#include <print>
#include <thread>

namespace pocket::app {

std::string usage() {
    return R"(pocket_runtime --project <dir> --bundle <file.js> [options]

  --headless            no window, no OS video; each frame runs exactly one tick
  --hidden              create the window but keep it hidden
  --frames N            stop after N frames (default: until the window closes)
  --capture <file.png>  write the last frame to a PNG
  --json                print a JSON report on stdout
  --seed N              deterministic RNG seed (default 1)
  --tick-rate HZ        simulation rate (default 60)
  --size WxH            render target size in pixels
  --log-level LEVEL     trace|debug|info|warn|error (default info)
  --log-file <file>     append JSON lines log records
  --title TEXT          window title
  --inspectable         allow Web Inspector to attach to the script engine
  --project-config <f>  JSON project settings (default <bundle>.project.json)
)";
}

Result<Options> parse_args(const std::vector<std::string>& args) {
    Options o;
    auto need = [&](std::size_t i, const char* flag) -> Result<std::string> {
        if (i + 1 >= args.size()) return fail("bad_args", "{} needs a value", flag);
        return args[i + 1];
    };
    for (std::size_t i = 0; i < args.size(); ++i) {
        const std::string& a = args[i];
        if (a == "--project") { POCKET_TRY(v, need(i, "--project")); o.project_dir = v; ++i; }
        else if (a == "--bundle") { POCKET_TRY(v, need(i, "--bundle")); o.bundle = v; ++i; }
        else if (a == "--project-config") { POCKET_TRY(v, need(i, "--project-config")); o.project_config = v; ++i; }
        else if (a == "--headless") o.headless = true;
        else if (a == "--hidden") o.visible = false;
        else if (a == "--frames") { POCKET_TRY(v, need(i, "--frames")); o.frames = std::stoi(v); ++i; }
        else if (a == "--capture") { POCKET_TRY(v, need(i, "--capture")); o.capture = v; ++i; }
        else if (a == "--json") o.json = true;
        else if (a == "--seed") { POCKET_TRY(v, need(i, "--seed")); o.seed = std::stoull(v); ++i; }
        else if (a == "--tick-rate") { POCKET_TRY(v, need(i, "--tick-rate")); o.tick_rate = std::stod(v); ++i; }
        else if (a == "--size") {
            POCKET_TRY(v, need(i, "--size"));
            auto x = v.find('x');
            if (x == std::string::npos) return fail("bad_args", "--size expects WxH");
            o.width = std::stoi(v.substr(0, x));
            o.height = std::stoi(v.substr(x + 1));
            ++i;
        }
        else if (a == "--log-level") { POCKET_TRY(v, need(i, "--log-level")); o.log_level = v; ++i; }
        else if (a == "--log-file") { POCKET_TRY(v, need(i, "--log-file")); o.log_file = v; ++i; }
        else if (a == "--title") { POCKET_TRY(v, need(i, "--title")); o.title = v; ++i; }
        else if (a == "--inspectable") o.inspectable = true;
        else if (a == "--no-tick-hash") o.hash_every_tick = false;
        else if (a == "--help" || a == "-h") return fail("help", "{}", usage());
        else return fail("bad_args", "unknown argument '{}'", a);
    }
    if (o.bundle.empty()) return fail("bad_args", "--bundle is required");
    if (o.project_config.empty()) o.project_config = o.bundle.string() + ".project.json";
    return o;
}

namespace {

struct ScriptState {
    rhi::Color clear{0.1f, 0.1f, 0.12f, 1.0f};
    Random rng;
    std::int64_t tick = 0;
    double tick_seconds = 1.0 / 60.0;
    int frame_width = 0, frame_height = 0;
    std::vector<Json> events;  // events produced by script for the report (M0: log only)
};

Status write_png(const std::filesystem::path& path, const rhi::Image& img) {
    if (path.has_parent_path()) POCKET_TRY_VOID(fs::ensure_dir(path.parent_path()));
    int ok = stbi_write_png(path.string().c_str(), static_cast<int>(img.width), static_cast<int>(img.height), 4, img.rgba.data(), static_cast<int>(img.width * 4));
    if (!ok) return fail("io_error", "cannot write PNG {}", path.string());
    return {};
}

Json error_json(const Error& e) {
    Json j;
    j["code"] = e.code;
    j["message"] = e.message;
    if (!e.detail.empty()) j["detail"] = e.detail;
    return j;
}

}  // namespace

Result<Json> run(const Options& opt) {
    Stopwatch total;
    auto& logger = log::global();
    logger.set_level(log::level_from_name(opt.log_level));
    logger.clear_sinks();
    logger.add_sink(log::stderr_sink());
    if (!opt.log_file.empty()) logger.add_sink(log::jsonl_file_sink(opt.log_file.string()));

    Json project = Json::object();
    if (std::filesystem::exists(opt.project_config)) {
        POCKET_TRY(text, fs::read_text(opt.project_config));
        project = Json::parse(text, nullptr, false);
        if (project.is_discarded()) return fail("bad_project_config", "{} is not valid JSON", opt.project_config.string());
    }
    std::string name = project.value("name", opt.project_dir.filename().string());
    Json window = project.contains("window") && project["window"].is_object() ? project["window"] : Json::object();
    int width = opt.width > 0 ? opt.width : window.value("width", 960);
    int height = opt.height > 0 ? opt.height : window.value("height", 540);
    std::string title = !opt.title.empty() ? opt.title : window.value("title", name);

    POCKET_TRY(bundle_src, fs::read_text(opt.bundle));

    platform::Config pc;
    pc.title = title.empty() ? "Pocket" : title;
    pc.width = width;
    pc.height = height;
    pc.headless = opt.headless;
    pc.visible = opt.visible;
    POCKET_TRY(platform, platform::Platform::create(pc));

    rhi::Config rc;
    rc.metal_layer = platform->metal_layer();
    rc.width = static_cast<std::uint32_t>(platform->pixel_width());
    rc.height = static_cast<std::uint32_t>(platform->pixel_height());
    POCKET_TRY(device, rhi::Device::create(rc));

    script::Config sc;
    sc.inspectable = opt.inspectable;
    POCKET_TRY(host, script::ScriptHost::create(sc));

    ScriptState state;
    state.rng.reseed(opt.seed);
    state.tick_seconds = 1.0 / opt.tick_rate;
    state.frame_width = static_cast<int>(device->width());
    state.frame_height = static_cast<int>(device->height());

    host->bind("log", [](const Json& args) -> Result<Json> {
        std::string level = args.size() > 0 && args[0].is_string() ? args[0].get<std::string>() : "info";
        std::string msg = args.size() > 1 && args[1].is_string() ? args[1].get<std::string>() : (args.size() > 1 ? args[1].dump() : "");
        Json fields = nullptr;
        if (args.size() > 2 && args[2].is_string()) fields = Json::parse(args[2].get<std::string>(), nullptr, false);
        else if (args.size() > 2) fields = args[2];
        log::global().emit(log::level_from_name(level), "script", msg, fields);
        return nullptr;
    });
    host->bind("setClearColor", [&state](const Json& args) -> Result<Json> {
        if (args.size() < 3) return fail("bad_args", "setClearColor(r, g, b, a?)");
        state.clear = {args[0].get<float>(), args[1].get<float>(), args[2].get<float>(), args.size() > 3 ? args[3].get<float>() : 1.0f};
        return nullptr;
    });
    host->bind("random", [&state](const Json&) -> Result<Json> { return state.rng.next_double(); });
    host->bind("info", [&](const Json&) -> Result<Json> {
        Json j;
        j["tickRate"] = opt.tick_rate;
        j["headless"] = opt.headless;
        j["width"] = state.frame_width;
        j["height"] = state.frame_height;
        j["seed"] = opt.seed;
        j["project"] = name;
        j["version"] = POCKET_VERSION;
        return j;
    });

    Json report;
    report["ok"] = true;
    report["project"] = name;
    report["bundle"] = opt.bundle.string();
    report["seed"] = opt.seed;
    report["tick_rate"] = opt.tick_rate;
    report["platform"] = platform->describe();
    report["gpu"] = device->describe();
    report["script"] = host->describe();
    std::vector<Json> errors;

    auto record_error = [&](const Error& e) {
        log::error("runtime", "{}", e.to_string());
        errors.push_back(error_json(e));
        report["ok"] = false;
    };

    if (auto r = host->evaluate(bundle_src, opt.bundle.string()); !r) {
        record_error(r.error());
    }
    bool has_dispatch = host->has_function("__pocket_dispatch");
    if (!has_dispatch) {
        log::warn("runtime", "bundle does not define __pocket_dispatch; scripts will not receive ticks");
    }

    auto dispatch = [&](const char* kind, Json arg) -> Json {
        if (!has_dispatch || !report["ok"].get<bool>()) return nullptr;
        Json args = Json::array({kind, std::move(arg)});
        auto r = host->call("__pocket_dispatch", args);
        if (!r) {
            record_error(r.error());
            return nullptr;
        }
        return *r;
    };

    dispatch("start", Json::object());

    TickClock clock;
    clock.tick_seconds = state.tick_seconds;
    StateHasher hasher;
    Json last_state = Json::object();
    std::uint64_t frames_run = 0;
    std::int64_t ticks_run = 0;
    Stopwatch frame_timer;
    std::vector<std::uint64_t> tick_hashes;

    auto run_tick = [&]() {
        state.tick = clock.tick;
        logger.set_tick(state.tick);
        Json t;
        t["tick"] = state.tick;
        t["dt"] = state.tick_seconds;
        t["time"] = clock.sim_seconds();
        dispatch("tick", t);
        Json s = dispatch("state", nullptr);
        if (!s.is_object()) s = Json::object();
        last_state = s;
        if (opt.hash_every_tick) {
            hasher.i64(state.tick);
            hasher.str(s.dump());
            hasher.f32(state.clear.r);
            hasher.f32(state.clear.g);
            hasher.f32(state.clear.b);
            tick_hashes.push_back(hasher.digest());
        }
        clock.tick++;
        ticks_run++;
    };

    while (report["ok"].get<bool>()) {
        if (opt.frames >= 0 && frames_run >= static_cast<std::uint64_t>(opt.frames)) break;
        auto events = platform->poll();
        if (platform->quit_requested()) break;
        if (!events.empty()) {
            Json ev = Json::array();
            for (auto& e : events) {
                if (e.type == platform::EventType::Resize) {
                    if (auto r = device->resize(static_cast<std::uint32_t>(e.width), static_cast<std::uint32_t>(e.height)); !r) record_error(r.error());
                    state.frame_width = e.width;
                    state.frame_height = e.height;
                }
                ev.push_back(platform::event_to_json(e));
            }
            dispatch("input", ev);
        }
        int ticks = 1;
        if (!opt.headless) {
            double elapsed = frame_timer.lap();
            if (frames_run == 0) elapsed = state.tick_seconds;
            ticks = clock.advance(elapsed);
        }
        for (int i = 0; i < ticks && report["ok"].get<bool>(); ++i) run_tick();
        host->drain_microtasks();

        auto frame = device->begin_frame();
        if (!frame) { record_error(frame.error()); break; }
        WGPURenderPassEncoder pass = device->begin_main_pass(*frame, state.clear);
        wgpuRenderPassEncoderEnd(pass);
        wgpuRenderPassEncoderRelease(pass);
        if (auto r = device->end_frame(*frame); !r) { record_error(r.error()); break; }
        frames_run++;
        if (opt.headless && opt.frames < 0) {
            // Headless with no frame budget would spin forever; cap at one simulated minute.
            if (frames_run >= 3600) break;
        }
    }
    dispatch("stop", Json::object());

    if (!opt.capture.empty()) {
        auto img = device->capture();
        if (!img) {
            record_error(img.error());
        } else if (auto w = write_png(opt.capture, *img); !w) {
            record_error(w.error());
        } else {
            Json c;
            c["path"] = opt.capture.string();
            c["width"] = img->width;
            c["height"] = img->height;
            std::size_t center = (static_cast<std::size_t>(img->height / 2) * img->width + img->width / 2) * 4;
            c["center_pixel"] = Json::array({img->rgba[center], img->rgba[center + 1], img->rgba[center + 2], img->rgba[center + 3]});
            report["capture"] = c;
        }
    }

    report["gpu"] = device->describe();
    report["frames"] = frames_run;
    report["ticks"] = ticks_run;
    report["sim_seconds"] = clock.sim_seconds();
    report["state"] = last_state;
    report["state_hash"] = hex64(hasher.digest());
    if (!tick_hashes.empty()) {
        Json last = Json::array();
        std::size_t start = tick_hashes.size() > 8 ? tick_hashes.size() - 8 : 0;
        for (std::size_t i = start; i < tick_hashes.size(); ++i) last.push_back(hex64(tick_hashes[i]));
        report["tick_hash_tail"] = last;
    }
    report["clear_color"] = Json::array({state.clear.r, state.clear.g, state.clear.b, state.clear.a});
    Json tail = Json::array();
    for (auto& r : logger.recent(20)) tail.push_back(log::to_json(r));
    report["log_tail"] = tail;
    if (!errors.empty()) report["errors"] = errors;
    report["elapsed_ms"] = total.ms();
    logger.set_tick(-1);
    return report;
}

int main(int argc, char** argv) {
    std::vector<std::string> args(argv + 1, argv + argc);
    auto opts = parse_args(args);
    if (!opts) {
        if (opts.error().code == "help") {
            std::print("{}", usage());
            return 0;
        }
        std::println(stderr, "error: {}\n{}", opts.error().message, usage());
        return 2;
    }
    auto report = run(*opts);
    if (!report) {
        if (opts->json) {
            Json j;
            j["ok"] = false;
            j["errors"] = Json::array({error_json(report.error())});
            std::println("{}", j.dump(2));
        } else {
            std::println(stderr, "error: {}", report.error().to_string());
        }
        return 1;
    }
    bool ok = (*report)["ok"].get<bool>();
    if (opts->json) {
        std::println("{}", report->dump(2));
    } else {
        std::println("{}: {} frames, {} ticks, state {}{}", (*report)["project"].get<std::string>(), (*report)["frames"].get<std::uint64_t>(), (*report)["ticks"].get<std::int64_t>(), (*report)["state_hash"].get<std::string>(), ok ? "" : " (with errors)");
    }
    return ok ? 0 : 1;
}

}  // namespace pocket::app
