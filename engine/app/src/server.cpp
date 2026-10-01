#ifndef __EMSCRIPTEN__
#include <pocket/app/server.hpp>

#include <pocket/core/log.hpp>

#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Weverything"
#define CPPHTTPLIB_NO_EXCEPTIONS 0
#include <httplib.h>
#pragma clang diagnostic pop

#include <condition_variable>
#include <deque>
#include <future>
#include <mutex>
#include <thread>

namespace pocket::app {

struct Pending {
    std::string method;
    Json params;
    std::string source;
    std::promise<Result<Json>> promise;
};

struct ControlServer::Impl {
    Session& session;
    int port;
    httplib::Server server;
    std::thread thread;
    std::mutex mutex;
    std::condition_variable cv;
    std::deque<std::shared_ptr<Pending>> queue;
    bool stopping = false;   // set while shutting down: new requests fail instead of waiting for a pump that never comes
    std::uint64_t handled = 0;

    Impl(Session& s, int p) : session(s), port(p) {}

    Result<Json> enqueue(std::string method, Json params) {
        auto pending = std::make_shared<Pending>();
        pending->method = std::move(method);
        pending->params = std::move(params);
        pending->source = "agent";
        auto future = pending->promise.get_future();
        {
            std::lock_guard lock(mutex);
            if (stopping) return fail("shutting_down", "runtime is shutting down");
            queue.push_back(pending);
        }
        cv.notify_all();
        return future.get();
    }

    static std::string rpc_error(const Json& id, int code, const std::string& message, const Json& data = nullptr) {
        Json j;
        j["jsonrpc"] = "2.0";
        j["id"] = id;
        j["error"] = Json{{"code", code}, {"message", message}};
        if (!data.is_null()) j["error"]["data"] = data;
        return j.dump();
    }

    void setup_routes() {
        server.Get("/", [](const httplib::Request&, httplib::Response& res) {
            Json j;
            j["service"] = "pocket runtime";
            j["version"] = POCKET_VERSION;
            j["rpc"] = "POST /rpc with {\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"world.tree\",\"params\":{}}";
            j["get"] = Json::array({"/tree", "/state", "/summary", "/events?since=0", "/commands"});
            res.set_content(j.dump(2), "application/json");
        });
        auto get_command = [&](const char* route, const char* method) {
            server.Get(route, [this, method](const httplib::Request& req, httplib::Response& res) {
                Json params = Json::object();
                for (auto& [k, v] : req.params) {
                    // Numbers stay numbers so ?since=12 works.
                    char* end = nullptr;
                    double d = std::strtod(v.c_str(), &end);
                    if (end && *end == '\0' && !v.empty()) params[k] = d;
                    else params[k] = v;
                }
                if (params.contains("since")) params["seq"] = params["since"];
                auto r = enqueue(method, params);
                if (!r) {
                    res.status = 400;
                    res.set_content(rpc_error(nullptr, -32000, r.error().message, Json{{"code", r.error().code}}), "application/json");
                    return;
                }
                if (r->is_object() && r->contains("text") && r->size() == 1) {
                    res.set_content((*r)["text"].get<std::string>(), "text/plain; charset=utf-8");
                } else {
                    res.set_content(r->dump(2), "application/json");
                }
            });
        };
        get_command("/tree", "world.tree");
        get_command("/state", "state");
        get_command("/summary", "world.summary");
        get_command("/events", "events.since");
        get_command("/commands", "commands");
        get_command("/schema", "world.schema");
        server.Post("/rpc", [this](const httplib::Request& req, httplib::Response& res) {
            Json body = Json::parse(req.body, nullptr, false);
            if (body.is_discarded()) {
                res.status = 400;
                res.set_content(rpc_error(nullptr, -32700, "parse error"), "application/json");
                return;
            }
            // {"calls": [{method, params}, ...]}, the shape the agent tools take, is answered like
            // the tools answer it: each call's method with its result or its error, in order.
            if (body.is_object() && !body.contains("method") && body.contains("calls") && body["calls"].is_array()) {
                Json out = Json::array();
                for (const Json& call : body["calls"]) {
                    const std::string method = call.is_object() ? call.value("method", "") : "";
                    auto r = enqueue(method, call.is_object() ? call.value("params", Json::object()) : Json::object());
                    out.push_back(r ? Json{{"method", method}, {"result", *r}} : Json{{"method", method}, {"error", r.error().message}});
                }
                Json resp;
                resp["jsonrpc"] = "2.0";
                resp["id"] = body.value("id", Json(nullptr));
                resp["result"] = std::move(out);
                res.set_content(resp.dump(), "application/json");
                return;
            }
            bool batch = body.is_array();
            Json responses = Json::array();
            for (const Json& call : batch ? body : Json::array({body})) {
                Json id = call.value("id", Json(nullptr));
                if (!call.is_object() || !call.contains("method") || !call["method"].is_string()) {
                    responses.push_back(Json::parse(rpc_error(id, -32600, "invalid request")));
                    continue;
                }
                auto r = enqueue(call["method"].get<std::string>(), call.value("params", Json::object()));
                Json resp;
                resp["jsonrpc"] = "2.0";
                resp["id"] = id;
                if (r) {
                    resp["result"] = *r;
                } else {
                    resp["error"] = Json{{"code", r.error().code == "unknown_command" ? -32601 : -32000}, {"message", r.error().message}, {"data", Json{{"code", r.error().code}, {"detail", r.error().detail}}}};
                }
                responses.push_back(resp);
            }
            res.set_content(batch ? responses.dump() : responses[0].dump(), "application/json");
        });
    }
};

ControlServer::ControlServer(Session& session, int port) : impl_(std::make_unique<Impl>(session, port)) {}

ControlServer::~ControlServer() {
    // Fail whatever is queued and refuse what arrives from now on, before the server is stopped:
    // stopping joins the handler threads, and a handler blocked on a request that no pump will
    // ever run would deadlock the shutdown (and hang its client).
    {
        std::lock_guard lock(impl_->mutex);
        impl_->stopping = true;
        for (auto& p : impl_->queue) p->promise.set_value(fail("shutting_down", "runtime is shutting down"));
        impl_->queue.clear();
    }
    impl_->server.stop();
    if (impl_->thread.joinable()) impl_->thread.join();
}

Status ControlServer::start() {
    impl_->setup_routes();
    if (impl_->port == 0) {
        impl_->port = impl_->server.bind_to_any_port("127.0.0.1");
        if (impl_->port < 0) return fail("bind_failed", "cannot bind a control port");
    } else if (!impl_->server.bind_to_port("127.0.0.1", impl_->port)) {
        return fail("bind_failed", "cannot bind 127.0.0.1:{}", impl_->port);
    }
    impl_->thread = std::thread([this] { impl_->server.listen_after_bind(); });
    log::info("runtime", "control server on http://127.0.0.1:{}", impl_->port);
    return {};
}

void ControlServer::pump(int wait_ms) {
    std::deque<std::shared_ptr<Pending>> batch;
    {
        std::unique_lock lock(impl_->mutex);
        if (impl_->queue.empty() && wait_ms > 0) {
            impl_->cv.wait_for(lock, std::chrono::milliseconds(wait_ms), [this] { return !impl_->queue.empty(); });
        }
        batch.swap(impl_->queue);
    }
    for (auto& p : batch) {
        p->promise.set_value(impl_->session.command(p->method, p->params, p->source));
        impl_->handled++;
    }
}

int ControlServer::port() const { return impl_->port; }

Json ControlServer::describe() const {
    Json j;
    j["url"] = std::format("http://127.0.0.1:{}", impl_->port);
    j["handled"] = impl_->handled;
    std::lock_guard lock(impl_->mutex);
    j["queued"] = impl_->queue.size();
    return j;
}

}  // namespace pocket::app
#endif  // __EMSCRIPTEN__
