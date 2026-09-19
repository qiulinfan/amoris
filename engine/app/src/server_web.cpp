// In the browser there is no listening socket: agents and pages drive the runtime through the
// exported pocket_command function instead (docs/web.md).
#ifdef __EMSCRIPTEN__
#include <pocket/app/server.hpp>

namespace pocket::app {

struct ControlServer::Impl {
    int port = 0;
};

ControlServer::ControlServer(Session&, int port) : impl_(std::make_unique<Impl>()) { impl_->port = port; }
ControlServer::~ControlServer() = default;
Status ControlServer::start() { return fail("unsupported", "the control server is not available in the browser; call pocket_command from the page"); }
void ControlServer::pump(int) {}
int ControlServer::port() const { return 0; }
Json ControlServer::describe() const { return Json{{"enabled", false}}; }

}  // namespace pocket::app
#endif
