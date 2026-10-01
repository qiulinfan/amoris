#include <pocket/app/runtime.hpp>

namespace {
int run(int argc, char** argv) { return pocket::app::main(argc, argv); }
}  // namespace

#ifdef POCKET_IOS
// On iOS UIKit starts the app: SDL's main runs UIApplicationMain and calls this one, as SDL_main,
// once the application has launched. (Its `main` macro is why the call above comes first.)
#include <SDL3/SDL_main.h>
#endif

int main(int argc, char** argv) { return run(argc, argv); }
