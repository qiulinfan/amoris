#include <pocket/app/runtime.hpp>
#include <pocket/app/session.hpp>

#include "server.hpp"

#include <cstdio>
#include <print>

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
  --serve [PORT]        HTTP control server on 127.0.0.1 (PORT 0 or omitted: any free port)
  --paused              start paused; advance with the `step` command (a window keeps drawing)
  --editor <bundle>     load an editor bundle (script context "editor") before the project bundle
  --record <file>       write an input journal for replay
  --replay <file>       replay an input journal (use with --headless for exact reproduction)
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
        else if (a == "--serve") {
            o.serve = 0;
            if (i + 1 < args.size() && !args[i + 1].empty() && std::isdigit(static_cast<unsigned char>(args[i + 1][0]))) { o.serve = std::stoi(args[i + 1]); ++i; }
        }
        else if (a == "--paused") o.paused = true;
        else if (a == "--editor") { POCKET_TRY(v, need(i, "--editor")); o.editor_bundle = v; ++i; }
        else if (a == "--record") { POCKET_TRY(v, need(i, "--record")); o.record = v; ++i; }
        else if (a == "--replay") { POCKET_TRY(v, need(i, "--replay")); o.replay = v; ++i; }
        else if (a == "--help" || a == "-h") return fail("help", "{}", usage());
        else return fail("bad_args", "unknown argument '{}'", a);
    }
    if (o.bundle.empty()) return fail("bad_args", "--bundle is required");
    if (o.project_config.empty()) o.project_config = o.bundle.string() + ".project.json";
    if (!o.replay.empty() && !o.record.empty()) return fail("bad_args", "--record and --replay are exclusive");
    return o;
}

namespace {

Json error_json(const Error& e) {
    Json j;
    j["code"] = e.code;
    j["message"] = e.message;
    if (!e.detail.empty()) j["detail"] = e.detail;
    return j;
}

}  // namespace

Result<Json> run(const Options& options) {
    Session session(options);
    POCKET_TRY_VOID(session.start());
    std::unique_ptr<ControlServer> server;
    if (options.serve >= 0) {
        server = std::make_unique<ControlServer>(session, options.serve);
        POCKET_TRY_VOID(server->start());
        if (options.json) {
            // Announce the port immediately so a driver process can connect before the run ends.
            std::println(stderr, "{{\"event\":\"listening\",\"url\":\"http://127.0.0.1:{}\"}}", server->port());
        }
    }
    session.set_paused(options.paused);
    while (!session.finished() && session.ok()) {
        if (server) server->pump(session.paused() ? 50 : 0);
        if (session.paused()) {
            // Paused: a window keeps polling input, running UI scripts and drawing (the editor
            // lives here); headless sessions only serve commands, and without a controller a
            // paused headless run would hang forever.
            if (options.headless) {
                if (!server) break;
                continue;
            }
            if (auto r = session.idle_frame(); !r) break;
            continue;
        }
        if (auto r = session.frame(); !r) break;
    }
    (void)session.finish();
    if (server) server->pump(0);
    Json report = session.report();
    if (server) report["control"] = server->describe();
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
        std::println("{}: {} frames, {} ticks, {} entities, state {}{}", (*report)["project"].get<std::string>(), (*report)["frames"].get<std::uint64_t>(), (*report)["ticks"].get<std::int64_t>(), (*report)["world"]["entities"].get<std::size_t>(), (*report)["state_hash"].get<std::string>(), ok ? "" : " (with errors)");
    }
    return ok ? 0 : 1;
}

}  // namespace pocket::app
