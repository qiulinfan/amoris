#include <pocket/app/runtime.hpp>
#include <pocket/app/session.hpp>

#include <pocket/app/server.hpp>

#ifdef __EMSCRIPTEN__
#include <emscripten.h>
#include <cstdlib>
#include <cstring>
#endif

#include <csignal>
#include <cstdio>
#include <print>

#if defined(_WIN32)
#include <fcntl.h>
#include <io.h>
#include <windows.h>   // after the engine's headers: its macros must not reach them
#include <tlhelp32.h>
#elif !defined(__EMSCRIPTEN__)
#include <unistd.h>
#endif

namespace pocket::app {

std::string usage() {
    return R"(pocket_runtime --project <dir> --bundle <file.js> [options]

  --headless            no window, no OS video; each frame runs exactly one tick
  --hidden              create the window but keep it hidden
  --frames N            stop after N frames (default: until the window closes; a headless run
                        without --serve stops after 3600 frames)
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
  --history N           keep the last N ticks for the recorder.* time-travel commands
  --tick-hash           fold the state and the whole world into the run's hash every tick (the
                        default headless, recording or replaying; a window playing skips it)
  --no-tick-hash        never hash per tick
  --scenario <bundle>   load a gameplay scenario bundle (script context "scenario") after the project
  --scenario-name NAME  which scenario of that bundle runs (default: the first)
  --net-host PORT       host a lockstep game on PORT (0: any free port) for --net-players (default 2)
  --net-players N       how many players the host waits for, itself included
  --net-join HOST:PORT  join a lockstep game (the host's seed and input delay are used)
  --net-delay N         ticks between an input and the tick it acts on (default 3)
  --net-dedicated       with --net-host: run and relay the game without playing it (a server both players reach)
  --exit-with-parent    end when the process that started this one is gone (what a tool starts and may forget)
)";
}

Result<Options> parse_args(const std::vector<std::string>& args) {
    Options o;
    bool tick_hash_given = false;
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
        else if (a == "--no-tick-hash") { o.hash_every_tick = false; tick_hash_given = true; }
        else if (a == "--tick-hash") { o.hash_every_tick = true; tick_hash_given = true; }
        else if (a == "--render") {
            POCKET_TRY(v, need(i, "--render"));
            if (v != "last" && v != "each") return fail("bad_args", "--render is last or each");
            o.render_last = v == "last";
            ++i;
        }
        else if (a == "--history") { POCKET_TRY(v, need(i, "--history")); o.history = std::stoi(v); ++i; }
        else if (a == "--scenario") { POCKET_TRY(v, need(i, "--scenario")); o.scenario_bundle = v; ++i; }
        else if (a == "--scenario-name") { POCKET_TRY(v, need(i, "--scenario-name")); o.scenario = v; ++i; }
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
        else if (a == "--net-host") { POCKET_TRY(v, need(i, "--net-host")); o.net_host = std::stoi(v); ++i; }
        else if (a == "--net-players") { POCKET_TRY(v, need(i, "--net-players")); o.net_players = std::stoi(v); ++i; }
        else if (a == "--net-join") { POCKET_TRY(v, need(i, "--net-join")); o.net_join = v; ++i; }
        else if (a == "--net-delay") { POCKET_TRY(v, need(i, "--net-delay")); o.net_delay = std::stoi(v); ++i; }
        else if (a == "--net-dedicated") o.net_dedicated = true;
        else if (a == "--exit-with-parent") o.exit_with_parent = true;
        else if (a == "--help" || a == "-h") return fail("help", "{}", usage());
        else return fail("bad_args", "unknown argument '{}'", a);
    }
    if (o.bundle.empty()) return fail("bad_args", "--bundle is required");
    if (o.project_config.empty()) o.project_config = o.bundle.string() + ".project.json";
    if (!o.replay.empty() && !o.record.empty()) return fail("bad_args", "--record and --replay are exclusive");
    // The per-tick hash is what replays, tests and determinism checks compare; a window someone is
    // playing (a packed game, pocket run, the editor) has no one to compare it with, and at ten
    // thousand entities it is over a millisecond a tick.
    if (!tick_hash_given && !o.headless && o.record.empty() && o.replay.empty()) o.hash_every_tick = false;
    if (o.net_host >= 0 && !o.net_join.empty()) return fail("bad_args", "--net-host and --net-join are exclusive");
    if (o.net_dedicated && o.net_host < 0) return fail("bad_args", "--net-dedicated goes with --net-host: it is the host that does not play");
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
// The same entry for commands that wait on the GPU (capture): the page awaits it. Under Asyncify
// any export may unwind; under JSPI only the exports listed at link time may suspend, and this is
// the one listed (pocket_command stays synchronous for everything else).
EMSCRIPTEN_KEEPALIVE char* pocket_command_async(const char* name, const char* params) { return pocket_command(name, params); }
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
namespace {
// A stop asked of the process (Ctrl-C, kill, a closed terminal): the loop ends and the run reports
// as at its end. Installed after the session starts, over what the platform layer put there, so a
// paused headless server, which polls no window events, still hears it.
volatile std::sig_atomic_t g_stop = 0;
extern "C" void on_stop_signal(int) { g_stop = 1; }

#if defined(_WIN32)
// Windows raises no SIGTERM or SIGHUP: a console's Ctrl-Break, its window closing, logoff and
// shutdown come as console events instead (Ctrl-C reaches SIGINT through the C runtime).
BOOL WINAPI on_console_event(DWORD event) {
    if (event == CTRL_BREAK_EVENT || event == CTRL_CLOSE_EVENT || event == CTRL_LOGOFF_EVENT || event == CTRL_SHUTDOWN_EVENT) {
        g_stop = 1;
        return TRUE;
    }
    return FALSE;
}

// The process that started this one, held open so its exit is seen even after its id is reused;
// null when it cannot be found.
HANDLE open_parent() {
    const DWORD self = GetCurrentProcessId();
    DWORD parent = 0;
    HANDLE snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
    if (snap == INVALID_HANDLE_VALUE) return nullptr;
    PROCESSENTRY32W e{};
    e.dwSize = sizeof e;
    for (BOOL more = Process32FirstW(snap, &e); more; more = Process32NextW(snap, &e)) {
        if (e.th32ProcessID == self) {
            parent = e.th32ParentProcessID;
            break;
        }
    }
    CloseHandle(snap);
    return parent ? OpenProcess(SYNCHRONIZE, FALSE, parent) : nullptr;
}
#endif
}  // namespace

Result<Json> run(const Options& options) {
    Session session(options);
    POCKET_TRY_VOID(session.start());
    std::signal(SIGINT, on_stop_signal);
    std::signal(SIGTERM, on_stop_signal);
#if defined(_WIN32)
    SetConsoleCtrlHandler(on_console_event, TRUE);
    HANDLE parent = options.exit_with_parent ? open_parent() : nullptr;
    auto orphaned = [&] { return parent && WaitForSingleObject(parent, 0) == WAIT_OBJECT_0; };
#elif !defined(__EMSCRIPTEN__)
    std::signal(SIGHUP, on_stop_signal);
    const pid_t parent = getppid();
    auto orphaned = [&] { return options.exit_with_parent && getppid() != parent; };
#else
    auto orphaned = [] { return false; };
#endif
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
    bool stopped_by_error = false;
    while (!session.finished() && (session.ok() || server)) {
        if (g_stop || orphaned()) {
            log::warn("runtime", "{}", g_stop ? "asked to stop: ending the run" : "the process that started this runtime is gone: ending the run");
            break;
        }
        if (server) server->pump(session.paused() || !session.ok() ? 50 : 0);
        if (!session.ok()) {
            // Serving, a script error stops the simulation but not the runtime: whoever drives it (an
            // agent, the editor) reads the error in `state` and fixes it; project.reload clears it.
            if (!stopped_by_error) {
                log::warn("runtime", "a script error stopped the simulation; the control server keeps serving (state shows the error, project.reload starts over)");
                stopped_by_error = true;
            }
            session.set_paused(true);
            continue;
        }
        stopped_by_error = false;
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
#if defined(_WIN32)
    // The JSON channel is bytes: no CR before each LF as Windows' text mode would write.
    _setmode(_fileno(stdout), _O_BINARY);
    _setmode(_fileno(stderr), _O_BINARY);
#endif
    std::vector<std::string> args(argv + 1, argv + argc);
#ifdef POCKET_IOS
    // An app (`pocket pack --ios`) carries its game in game/ beside the executable, laid out as a
    // desktop pack's; what it is launched with (simctl launch ... --serve 4711) comes after.
    if (std::find(args.begin(), args.end(), "--bundle") == args.end() && argc > 0) {
        const std::filesystem::path game = std::filesystem::path(argv[0]).parent_path() / "game";
        args.insert(args.begin(), {"--project", (game / "project").string(), "--bundle", (game / "project.js").string(), "--project-config", (game / "project.json").string()});
    }
#endif
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
