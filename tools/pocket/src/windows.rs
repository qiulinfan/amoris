//! Setup steps only Windows needs: JavaScriptCore linked into its own DLL.
//!
//! There is no system JavaScriptCore on Windows. Bun's WebKit build (static libraries compiled
//! with clang-cl for the static C runtime) is linked once at `pocket setup` into
//! `pocket_jsc.dll`, which exports the JavaScriptCore C API and nothing else: the engine keeps the
//! dynamic C runtime and its sanitizers, and the LGPL library stays a replaceable DLL
//! (docs/decisions/0008-windows.md). The DLL holds, besides Bun's libraries, the allocator they
//! were built against (mimalloc), the hook that decompresses their ICU data, and wrappers that put
//! back the API lock Bun's build leaves out of some C API functions (third_party/javascriptcore).

use crate::manifest::{Dependency, Workspace};
use crate::toolchain;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

const SHIM_DIR: &str = "third_party/javascriptcore";

/// The sources a dependency's setup step compiles, which make its stamp stale when they change.
pub fn setup_step_inputs(ws: &Workspace, dep: &Dependency) -> Vec<PathBuf> {
    if !is_jsc(dep) {
        return vec![];
    }
    let shim = ws.root.join(SHIM_DIR);
    vec![shim.join("pocket_jsc.def"), shim.join("jsc_locks.cpp"), shim.join("icu_hook.cpp")]
}

fn is_jsc(dep: &Dependency) -> bool {
    cfg!(windows) && dep.name == "javascriptcore" && dep.kind == "prebuilt"
}

/// Run after a dependency is unpacked on a Windows host.
pub fn setup_step(ws: &Workspace, dep: &Dependency, pfx: &Path, log: &mut Vec<String>) -> Result<()> {
    if is_jsc(dep) {
        link_jsc(ws, pfx, log)?;
    }
    Ok(())
}

fn link_jsc(ws: &Workspace, pfx: &Path, log: &mut Vec<String>) -> Result<()> {
    let tc = toolchain::detect()?;
    let shim = ws.root.join(SHIM_DIR);
    let mimalloc_dep = ws.dependency("mimalloc").context("javascriptcore on Windows needs the mimalloc dependency (pocket.toml)")?;
    let mimalloc = crate::deps::prefix(ws, mimalloc_dep);
    if !mimalloc.join("src").join("static.c").is_file() {
        bail!("mimalloc is not set up at {} (it must come before javascriptcore in pocket.toml)", mimalloc.display());
    }
    let zstd = ws.root.join("third_party").join("basisu").join("zstd");
    let obj = pfx.join("obj");
    toolchain::ensure_dir(&obj)?;
    toolchain::ensure_dir(&pfx.join("bin"))?;
    let static_crt = ["-fms-runtime-lib=static", "-O2", "-DNDEBUG"];

    // mimalloc as Bun builds it for Windows: one translation unit compiled as C++, no malloc
    // override, no heap walk or teardown at exit.
    let mut c = Command::new(&tc.cxx);
    c.args(["-x", "c++", "-std=c++20", "-w"]).args(static_crt);
    c.args(["-DMI_STATIC_LIB", "-DMI_SKIP_COLLECT_ON_EXIT=1", "-DMI_NO_PROCESS_DETACH=1", "-DMI_BUILD_RELEASE", "-DMI_CMAKE_BUILD_TYPE=release"]);
    c.arg(format!("-I{}", mimalloc.join("include").display())).arg("-c").arg(mimalloc.join("src").join("static.c")).arg("-o").arg(obj.join("mimalloc.obj"));
    run(c, "compile mimalloc", log)?;

    // zstd's decoder, for the ICU data items Bun's build stores compressed.
    let mut c = Command::new(&tc.cc);
    c.args(["-w"]).args(static_crt).arg("-c").arg(zstd.join("zstddeclib.c")).arg("-o").arg(obj.join("zstd.obj"));
    run(c, "compile zstd", log)?;

    let mut c = Command::new(&tc.cxx);
    c.args(["-std=c++20", "-Wall", "-Wextra", "-Werror"]).args(static_crt);
    c.arg(format!("-I{}", zstd.display())).arg("-c").arg(shim.join("icu_hook.cpp")).arg("-o").arg(obj.join("icu_hook.obj"));
    run(c, "compile icu_hook.cpp", log)?;

    // The lock wrappers include JavaScriptCore's own headers, so they are compiled the way Bun
    // compiles code against them (scripts/build/flags.ts in oven-sh/bun).
    let mut c = Command::new(&tc.cxx);
    c.args(["-std=c++23", "-fno-exceptions", "-fno-rtti", "-w"]).args(static_crt);
    for d in ["BUILDING_JSCONLY__", "BUILDING_WITH_CMAKE=1", "HAVE_CONFIG_H=1", "STATICALLY_LINKED_WITH_JavaScriptCore=1", "STATICALLY_LINKED_WITH_WTF=1", "STATICALLY_LINKED_WITH_bmalloc=1", "JSC_OBJC_API_ENABLED=0", "_HAS_EXCEPTIONS=0", "NOMINMAX", "WIN32_LEAN_AND_MEAN", "_CRT_SECURE_NO_WARNINGS", "U_STATIC_IMPLEMENTATION"] {
        c.arg(format!("-D{d}"));
    }
    c.arg(format!("-I{}", pfx.join("include").display())).arg(format!("-I{}", pfx.join("include").join("JavaScriptCore").display()));
    c.arg("-c").arg(shim.join("jsc_locks.cpp")).arg("-o").arg(obj.join("jsc_locks.obj"));
    run(c, "compile jsc_locks.cpp", log)?;

    // Identical-code folding stays off: it merges JavaScriptCore functions that must stay apart
    // (Bun saw the BigInt constructor become "not a constructor").
    let lib = pfx.join("lib");
    let mut l = Command::new(&tc.cxx);
    l.args(["-fuse-ld=lld", "-shared", "-fms-runtime-lib=static"]).arg("-o").arg(pfx.join("bin").join("pocket_jsc.dll"));
    l.arg(format!("-Wl,/DEF:{}", shim.join("pocket_jsc.def").display()));
    l.arg(format!("-Wl,/IMPLIB:{}", lib.join("pocket_jsc.lib").display()));
    l.args(["-Wl,/MACHINE:X64", "-Wl,/OPT:REF", "-Wl,/OPT:NOICF"]);
    for o in ["jsc_locks.obj", "icu_hook.obj", "zstd.obj", "mimalloc.obj"] {
        l.arg(obj.join(o));
    }
    for name in ["JavaScriptCore.lib", "WTF.lib", "bmalloc.lib", "sicuin.lib", "sicuuc.lib", "sicudt.lib"] {
        l.arg(lib.join(name));
    }
    for sys in ["kernel32", "user32", "advapi32", "shell32", "ole32", "oleaut32", "winmm", "bcrypt", "ntdll", "userenv", "dbghelp", "shlwapi", "synchronization", "ws2_32", "wsock32", "crypt32", "psapi", "version"] {
        l.arg(format!("-l{sys}"));
    }
    run(l, "link pocket_jsc.dll", log)?;

    // The archive's own programs and their debug databases (about 800 MB) are not needed.
    for entry in std::fs::read_dir(pfx.join("bin"))?.filter_map(|e| e.ok()) {
        let p = entry.path();
        if p.file_name().is_some_and(|n| n != "pocket_jsc.dll") {
            let _ = std::fs::remove_file(&p);
        }
    }
    let _ = std::fs::remove_dir_all(&obj);
    Ok(())
}

fn run(mut cmd: Command, what: &str, log: &mut Vec<String>) -> Result<()> {
    log.push(what.to_string());
    let out = cmd.output().with_context(|| what.to_string())?;
    if !out.status.success() {
        bail!("{what} failed:\n{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    }
    Ok(())
}
