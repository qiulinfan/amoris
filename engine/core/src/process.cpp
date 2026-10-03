#include <pocket/core/process.hpp>

#include <system_error>

#if defined(_WIN32)
#include <windows.h>
#elif !defined(__EMSCRIPTEN__)
#include <fcntl.h>
#include <spawn.h>
#include <sys/wait.h>
#include <unistd.h>

#include <cerrno>
#include <cstring>
extern char** environ;
#endif

namespace pocket::process {

#if defined(_WIN32)

namespace {

std::wstring wide(const std::string& s) {
    if (s.empty()) return {};
    const int n = MultiByteToWideChar(CP_UTF8, 0, s.data(), static_cast<int>(s.size()), nullptr, 0);
    std::wstring out(static_cast<std::size_t>(n), L'\0');
    MultiByteToWideChar(CP_UTF8, 0, s.data(), static_cast<int>(s.size()), out.data(), n);
    return out;
}

// One argument as the C runtime's command-line parser (CommandLineToArgvW's rules) reads it back:
// quoted when it has a space, a tab or a quote or is empty; a quote escaped by a backslash, and
// the backslashes before a quote (or the closing one) doubled.
void append_argument(std::wstring& line, const std::wstring& arg) {
    if (!line.empty()) line += L' ';
    if (!arg.empty() && arg.find_first_of(L" \t\n\v\"") == std::wstring::npos) {
        line += arg;
        return;
    }
    line += L'"';
    std::size_t backslashes = 0;
    for (wchar_t c : arg) {
        if (c == L'\\') {
            ++backslashes;
            continue;
        }
        if (c == L'"') line.append(backslashes * 2 + 1, L'\\');
        else line.append(backslashes, L'\\');
        backslashes = 0;
        line += c;
    }
    line.append(backslashes * 2, L'\\');
    line += L'"';
}

std::string last_error() { return std::system_category().message(static_cast<int>(GetLastError())); }

}  // namespace

Result<int> run(const std::vector<std::string>& args, const std::filesystem::path& log) {
    if (args.empty()) return fail("bad_args", "no program to run");
    std::wstring line;
    for (const std::string& a : args) append_argument(line, wide(a));
    SECURITY_ATTRIBUTES sa{};
    sa.nLength = sizeof sa;
    sa.bInheritHandle = TRUE;
    HANDLE out = CreateFileW(log.wstring().c_str(), GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, &sa, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (out == INVALID_HANDLE_VALUE) return fail("unavailable", "cannot write {}: {}", log.string(), last_error());
    // Only the log handle is inherited, as both standard output and error; no console window.
    SIZE_T size = 0;
    InitializeProcThreadAttributeList(nullptr, 1, 0, &size);
    std::vector<unsigned char> attr_buf(size);
    auto* attrs = reinterpret_cast<LPPROC_THREAD_ATTRIBUTE_LIST>(attr_buf.data());
    InitializeProcThreadAttributeList(attrs, 1, 0, &size);
    UpdateProcThreadAttribute(attrs, 0, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, &out, sizeof out, nullptr, nullptr);
    STARTUPINFOEXW si{};
    si.StartupInfo.cb = sizeof si;
    si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    si.StartupInfo.hStdInput = GetStdHandle(STD_INPUT_HANDLE);
    si.StartupInfo.hStdOutput = out;
    si.StartupInfo.hStdError = out;
    si.lpAttributeList = attrs;
    PROCESS_INFORMATION pi{};
    const std::wstring program = wide(args[0]);
    const BOOL started = CreateProcessW(program.c_str(), line.data(), nullptr, nullptr, TRUE, CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT, nullptr, nullptr, &si.StartupInfo, &pi);
    const std::string why = started ? std::string() : last_error();
    DeleteProcThreadAttributeList(attrs);
    CloseHandle(out);
    if (!started) return fail("unavailable", "cannot start {}: {}", args[0], why);
    WaitForSingleObject(pi.hProcess, INFINITE);
    DWORD code = 0;
    const bool got = GetExitCodeProcess(pi.hProcess, &code) != 0;
    CloseHandle(pi.hThread);
    CloseHandle(pi.hProcess);
    return got ? static_cast<int>(code) : -1;
}

unsigned long id() { return GetCurrentProcessId(); }

#elif !defined(__EMSCRIPTEN__)

Result<int> run(const std::vector<std::string>& args, const std::filesystem::path& log) {
    if (args.empty()) return fail("bad_args", "no program to run");
    std::vector<std::string> owned = args;
    std::vector<char*> argv;
    for (std::string& a : owned) argv.push_back(a.data());
    argv.push_back(nullptr);
    const std::string log_file = log.string();
    posix_spawn_file_actions_t actions;
    posix_spawn_file_actions_init(&actions);
    posix_spawn_file_actions_addopen(&actions, STDOUT_FILENO, log_file.c_str(), O_WRONLY | O_CREAT | O_TRUNC, 0644);
    posix_spawn_file_actions_adddup2(&actions, STDOUT_FILENO, STDERR_FILENO);
    pid_t pid = 0;
    const int rc = posix_spawn(&pid, args[0].c_str(), &actions, nullptr, argv.data(), environ);
    posix_spawn_file_actions_destroy(&actions);
    if (rc != 0) return fail("unavailable", "cannot start {}: {}", args[0], std::strerror(rc));
    int status = 0;
    while (waitpid(pid, &status, 0) < 0 && errno == EINTR) {}
    return WIFEXITED(status) ? WEXITSTATUS(status) : -1;
}

unsigned long id() { return static_cast<unsigned long>(getpid()); }

#else

Result<int> run(const std::vector<std::string>& args, const std::filesystem::path&) {
    return fail("unsupported", "running {} is not available in the browser", args.empty() ? std::string("a program") : args[0]);
}

unsigned long id() { return 0; }

#endif

}  // namespace pocket::process
