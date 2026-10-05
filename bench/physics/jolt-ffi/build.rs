//! Builds joltc (the C wrapper) and Jolt as static libraries with CMake.
//!
//! JOLTC_ROOT (default ~/Reference/joltc-amerkoleci) and JOLT_ROOT (default ~/Reference/JoltPhysics-v5.6.0, the release joltc targets)
//! name the checkouts. joltc is copied into OUT_DIR and patched there:
//! - joltc maps a thread count of 0 to "all cores"; the patch lets 0 mean no worker threads, so a
//!   single-threaded run is possible.
//! - Only when JOLT_ROOT is newer than v5.6.0: Jolt master removed
//!   `PhysicsSettings::mDeterministicSimulation` (the simulation is always deterministic now), which
//!   joltc still copies; those lines are dropped. With v5.6.0, the release joltc targets, this is a
//!   no-op.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let p = e.path();
        let q = to.join(e.file_name());
        if p.is_dir() {
            copy_dir(&p, &q);
        } else {
            fs::copy(&p, &q).unwrap();
        }
    }
}

fn main() {
    let home = env::var("HOME").unwrap();
    let joltc = PathBuf::from(
        env::var("JOLTC_ROOT").unwrap_or(format!("{home}/Reference/joltc-amerkoleci")),
    );
    let jolt = PathBuf::from(
        env::var("JOLT_ROOT").unwrap_or(format!("{home}/Reference/JoltPhysics-v5.6.0")),
    );
    println!("cargo:rerun-if-env-changed=JOLTC_ROOT");
    println!("cargo:rerun-if-env-changed=JOLT_ROOT");
    println!("cargo:rerun-if-changed=build.rs");

    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let src = out.join("joltc-src");
    let _ = fs::remove_dir_all(&src);
    fs::create_dir_all(&src).unwrap();
    fs::copy(joltc.join("CMakeLists.txt"), src.join("CMakeLists.txt")).unwrap();
    copy_dir(&joltc.join("include"), &src.join("include"));
    copy_dir(&joltc.join("src"), &src.join("src"));

    let cpp_path = src.join("src/joltc.cpp");
    let mut cpp = fs::read_to_string(&cpp_path).unwrap();
    let settings_h = fs::read_to_string(jolt.join("Jolt/Physics/PhysicsSettings.h")).unwrap();
    if !settings_h.contains("mDeterministicSimulation") {
        cpp = cpp
            .lines()
            .filter(|l| !l.contains("mDeterministicSimulation"))
            .collect::<Vec<_>>()
            .join("\n");
    }
    cpp = cpp.replace(
        "createConfig.numThreads > 0 ? createConfig.numThreads : -1",
        "createConfig.numThreads >= 0 ? createConfig.numThreads : -1",
    );
    fs::write(&cpp_path, cpp).unwrap();

    let det = env::var("CARGO_FEATURE_DET").is_ok();
    if env::var("TARGET").unwrap() == "wasm32-unknown-unknown" {
        build_wasm(&src, &jolt, &out, det);
        return;
    }
    let dst = cmake::Config::new(&src)
        .generator("Ninja")
        .profile("Distribution")
        .define("JOLT_PHYSICS_ROOT", &jolt)
        .define("CMAKE_OSX_ARCHITECTURES", "arm64")
        .define("JPH_BUILD_SHARED", "OFF")
        .define("JPH_SAMPLES", "OFF")
        .define("JPH_TESTS", "OFF")
        .define("JPH_USE_DX12", "OFF")
        .define("JPH_USE_VK", "OFF")
        .define("JPH_USE_MTL", "OFF")
        .define("DEBUG_RENDERER_IN_DISTRIBUTION", "OFF")
        .define("PROFILER_IN_DISTRIBUTION", "OFF")
        .define("ENABLE_ALL_WARNINGS", "OFF")
        .define(
            "CROSS_PLATFORM_DETERMINISTIC",
            if det { "ON" } else { "OFF" },
        )
        .build_target("joltc")
        .build();
    let lib = dst.join("build/lib");
    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-lib=static=joltc");
    println!("cargo:rustc-link-lib=static=Jolt");
    println!("cargo:rustc-link-lib=c++");
}

/// The browser target: joltc and Jolt compiled by Homebrew's clang for wasm32 with wasi-libc and
/// wasi-runtimes' libc++ (bench/physics/jolt/wasi/wasm32-wasip1.cmake, the toolchain jolt_bench's
/// wasm build uses), then linked by rustc into the Rust `wasm32-unknown-unknown` module together
/// with libc++, libc++abi, libc and compiler-rt's builtins from the same sysroot. The objects are
/// plain wasm32 objects whatever their triple says; what the C and C++ libraries still need from
/// the system (WASI imports) stays as imports of the module, which the host stubs
/// (scripts/run_jolt_wasm.mjs lists them). No wasm SIMD (Jolt only has it through Emscripten's SSE
/// headers) and no link-time optimization across the two LLVMs (rustc's and Homebrew's).
fn build_wasm(src: &Path, jolt: &Path, out: &Path, det: bool) {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let toolchain = manifest.join("../jolt/wasi/wasm32-wasip1.cmake");
    let sysroot = PathBuf::from(
        env::var("WASI_SYSROOT").unwrap_or("/opt/homebrew/share/wasi-sysroot".into()),
    );
    println!("cargo:rerun-if-env-changed=WASI_SYSROOT");
    println!("cargo:rerun-if-changed={}", toolchain.display());
    let build = out.join("build-wasm");
    let jobs = env::var("NUM_JOBS").unwrap_or("3".into());
    let run = |cmd: &mut Command| {
        let status = cmd.status().expect("cmake");
        assert!(status.success(), "{cmd:?} failed");
    };
    run(Command::new("cmake")
        .arg("-S")
        .arg(src)
        .arg("-B")
        .arg(&build)
        .args(["-G", "Ninja", "-DCMAKE_BUILD_TYPE=Distribution"])
        .arg(format!("-DCMAKE_TOOLCHAIN_FILE={}", toolchain.display()))
        .arg(format!("-DJOLT_PHYSICS_ROOT={}", jolt.display()))
        .arg(format!(
            "-DCROSS_PLATFORM_DETERMINISTIC={}",
            if det { "ON" } else { "OFF" }
        ))
        .args([
            "-DJPH_BUILD_SHARED=OFF",
            "-DJPH_SAMPLES=OFF",
            "-DJPH_TESTS=OFF",
            "-DJPH_USE_DX12=OFF",
            "-DJPH_USE_VK=OFF",
            "-DJPH_USE_MTL=OFF",
            "-DJPH_USE_CPU_COMPUTE=OFF",
            "-DDEBUG_RENDERER_IN_DISTRIBUTION=OFF",
            "-DPROFILER_IN_DISTRIBUTION=OFF",
            "-DENABLE_ALL_WARNINGS=OFF",
            "-DUSE_WASM_SIMD=OFF",
            "-DINTERPROCEDURAL_OPTIMIZATION=OFF",
        ]));
    run(Command::new("cmake")
        .arg("--build")
        .arg(&build)
        .args(["--target", "joltc", "-j", &jobs]));

    let mut lib_dirs = vec![build.join("lib"), build.clone()];
    lib_dirs.push(sysroot.join("lib/wasm32-wasip1"));
    for d in &lib_dirs {
        println!("cargo:rustc-link-search=native={}", d.display());
    }
    // compiler-rt's builtins for wasm32 (128-bit integer helpers and the like).
    let runtimes = sysroot
        .parent()
        .unwrap()
        .join("wasi-runtimes/lib/wasm32-unknown-wasip1");
    let runtimes = if runtimes.exists() {
        runtimes
    } else {
        // Homebrew links share/wasi-runtimes only inside the keg.
        let cellar = PathBuf::from("/opt/homebrew/Cellar/wasi-runtimes");
        let version = fs::read_dir(&cellar)
            .expect("brew install wasi-runtimes")
            .next()
            .unwrap()
            .unwrap()
            .path();
        version.join("share/wasi-runtimes/lib/wasm32-unknown-wasip1")
    };
    println!("cargo:rustc-link-search=native={}", runtimes.display());
    for lib in ["joltc", "Jolt", "c++", "c++abi", "c", "clang_rt.builtins"] {
        println!("cargo:rustc-link-lib=static={lib}");
    }
}
