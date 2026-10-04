# CMake toolchain for jolt_bench on wasm32-wasip1 (Node's WASI), with Homebrew's LLVM, wasi-libc and
# wasi-runtimes (libc++ for WASI): brew install llvm lld wasi-libc wasi-runtimes.
# Jolt recognizes WebAssembly only through __EMSCRIPTEN__ (Jolt/Core/Core.h), and nothing else in Jolt
# uses that define, so it is set by hand, and so is CMake's EMSCRIPTEN (see below); the job system
# runs on the calling thread only.
set(CMAKE_SYSTEM_NAME Generic)
set(CMAKE_SYSTEM_PROCESSOR wasm32)
set(LLVM /opt/homebrew/opt/llvm/bin)
set(WASI_SYSROOT /opt/homebrew/share/wasi-sysroot)
file(GLOB WASI_RESOURCE_DIR /opt/homebrew/Cellar/wasi-runtimes/*/share/wasi-runtimes)
set(CMAKE_C_COMPILER ${LLVM}/clang)
set(CMAKE_CXX_COMPILER ${LLVM}/clang++)
set(CMAKE_AR ${LLVM}/llvm-ar)
set(CMAKE_RANLIB ${LLVM}/llvm-ranlib)
set(CMAKE_C_COMPILER_TARGET wasm32-wasip1)
set(CMAKE_CXX_COMPILER_TARGET wasm32-wasip1)
set(CMAKE_SYSROOT ${WASI_SYSROOT})
set(CMAKE_CXX_FLAGS_INIT "-resource-dir ${WASI_RESOURCE_DIR} -D__EMSCRIPTEN__ -D_WASI_EMULATED_SIGNAL")
set(CMAKE_C_FLAGS_INIT "-resource-dir ${WASI_RESOURCE_DIR}")
set(CMAKE_EXE_LINKER_FLAGS_INIT "-resource-dir ${WASI_RESOURCE_DIR} --ld-path=/opt/homebrew/opt/lld/bin/wasm-ld -lwasi-emulated-signal -Wl,-z,stack-size=8388608 -Wl,--initial-memory=1073741824")
set(CMAKE_TRY_COMPILE_TARGET_TYPE STATIC_LIBRARY)
# Jolt.cmake adds -pthread on every non-Windows platform except Emscripten; -pthread would make
# wasm-ld ask for shared memory, which a wasip1 build has not.
set(EMSCRIPTEN ON)
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
