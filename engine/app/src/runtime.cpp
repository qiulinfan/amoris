#include <pocket/app/runtime.hpp>
#include <pocket/app/session.hpp>

#include "server.hpp"

#ifdef __EMSCRIPTEN__
#include <emscripten.h>
#include <cstdlib>
#include <cstring>
#endif

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
  --save-dir <dir>      where save slots are written and read (default: the user data directory)
  --font <file>         UI font file (overrides the project config and POCKET_FONT)
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
        else if (a == "--save-dir") { POCKET_TRY(v, need(i, "--save-dir")); o.save_dir = v; ++i; }
        else if (a == "--font") { POCKET_TRY(v, need(i, "--font")); o.font = v; ++i; }
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

#ifdef __EMSCRIPTEN__
namespace {
// The browser owns the loop: one Session lives for the page, the frame callback advances it,
// and the page (or an agent in it) calls pocket_command between frames.
Session* g_web_session = nullptr;
// A command that waits on the GPU (capture) yields to the event loop; frames must not run
// inside it, so the frame callback skips while a command is in flight.
bool g_web_busy = false;

void web_frame(void* arg) {
    auto* session = static_cast<Session*>(arg);
    if (g_web_busy) return;
    if (session->finished() || !session->ok()) {
        (void)session->finish();
        log::info("runtime", "web: run finished");
        emscripten_cancel_main_loop();
        return;
    }
    if (session->paused()) {
        (void)session->idle_frame();
    } else {
        (void)session->frame();
    }
}
}  // namespace

extern "C" {
// Any runtime command from the page: pocket_command("world.tree", "{}") -> JSON string (freed by the caller).
EMSCRIPTEN_KEEPALIVE char* pocket_command(const char* name, const char* params) {
    Json out;
    if (!g_web_session) {
        out["ok"] = false;
        out["error"] = Json{{"code", "not_running"}, {"message", "the runtime has not started"}};
    } else {
        Json p = Json::parse(params && *params ? params : "{}", nullptr, false);
        if (p.is_discarded()) p = Json::object();
        g_web_busy = true;
        Result<Json> r = g_web_session->command(name ? name : "", p);
        g_web_busy = false;
        if (r) {
            out["ok"] = true;
            out["result"] = *r;
        } else {
            out["ok"] = false;
            out["error"] = error_json(r.error());
        }
    }
    std::string text = out.dump();
    char* buf = static_cast<char*>(std::malloc(text.size() + 1));
    std::memcpy(buf, text.c_str(), text.size() + 1);
    return buf;
}
}

Result<Json> run(const Options& options) {
    static Session* session = new Session(options);
    POCKET_TRY_VOID(session->start());
    g_web_session = session;
    session->set_paused(options.paused);
    log::info("runtime", "web: main loop handed to the page");
    emscripten_set_main_loop_arg(web_frame, session, 0, 1);
    return session->report();   // not reached: the loop above never returns
}
#else
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
            if (options.headless && options.editor_bundle.empty()) {
                if (!server) break;
                continue;
            }
            // A headless editor session still runs idle frames so the editor's scripts (and an
            // agent driving them) see a live interface.
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
#endif

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
