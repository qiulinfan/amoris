#![allow(clippy::uninlined_format_args)]
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{self},
};

// WASI logic lifted from https://github.com/bytecodealliance/javy/blob/61616e1507d2bf896f46dc8d72687273438b58b2/crates/quickjs-wasm-sys/build.rs#L18

const WASI_SDK_VERSION_MAJOR: usize = 24;
const WASI_SDK_VERSION_MINOR: usize = 0;

fn download_wasi_sdk() -> PathBuf {
    let mut wasi_sdk_dir: PathBuf = env::var("OUT_DIR").unwrap().into();
    wasi_sdk_dir.push("wasi-sdk");

    fs::create_dir_all(&wasi_sdk_dir).unwrap();

    let major_version = WASI_SDK_VERSION_MAJOR;
    let minor_version = WASI_SDK_VERSION_MINOR;

    let mut archive_path = wasi_sdk_dir.clone();
    archive_path.push(format!("wasi-sdk-{major_version}-{minor_version}.tar.gz"));

    println!("SDK tar: {archive_path:?}");

    // Download archive if necessary
    if !archive_path.try_exists().unwrap() {
        let file_suffix = match (env::consts::OS, env::consts::ARCH) {
            ("linux", "x86") | ("linux", "x86_64") => "x86_64-linux",
            ("linux", "aarch64") => "arm64-linux",
            ("macos", "x86") | ("macos", "x86_64") => "x86_64-macos",
            ("macos", "aarch64") => "arm64-macos",
            ("windows", "x86") | ("windows", "x86_64") => "x86_64-windows",
            ("windows", "aarch64") => "arm64-windows",
            other => panic!("Unsupported platform tuple {:?}", other),
        };

        let uri = format!("https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-{major_version}/wasi-sdk-{major_version}.{minor_version}-{file_suffix}.tar.gz");

        println!("Downloading WASI SDK archive from {uri} to {archive_path:?}");

        let output = process::Command::new("curl")
            .args([
                "--location",
                "-o",
                archive_path.to_string_lossy().as_ref(),
                uri.as_ref(),
            ])
            .output()
            .expect("failed to download the WASI SDK with curl");
        println!("curl output: {}", String::from_utf8_lossy(&output.stdout));
        println!("curl err: {}", String::from_utf8_lossy(&output.stderr));
        if !output.status.success() {
            panic!(
                "curl WASI SDK failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    let mut test_binary = wasi_sdk_dir.clone();
    test_binary.extend(["bin", "wasm-ld"]);
    // Extract archive if necessary
    if !test_binary.try_exists().unwrap() {
        println!("Extracting WASI SDK archive {archive_path:?}");
        let output = process::Command::new("tar")
            .args([
                "-zxf",
                archive_path.to_string_lossy().as_ref(),
                "--strip-components",
                "1",
            ])
            .current_dir(&wasi_sdk_dir)
            .output()
            .unwrap();
        if !output.status.success() {
            panic!(
                "Unpacking WASI SDK failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    wasi_sdk_dir
}

fn get_wasi_sdk_path() -> PathBuf {
    std::env::var_os("WASI_SDK")
        .map(PathBuf::from)
        .unwrap_or_else(download_wasi_sdk)
}

fn main() {
    // amoris: the vendored, patched QuickJS-ng sources (P1 to P10) must rebuild the library when
    // they change; the rerun lines below would otherwise be the only triggers.
    println!("cargo:rerun-if-changed=quickjs");
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(feature = "logging")]
    pretty_env_logger::init();

    let features = [
        "bindgen",
        "update-bindings",
        "dump-bytecode",
        "dump-gc",
        "dump-gc-free",
        "dump-free",
        "dump-leaks",
        "dump-mem",
        "dump-objects",
        "dump-atoms",
        "dump-shapes",
        "dump-module-resolve",
        "dump-promise",
        "dump-read-object",
        "disable-assertions",
    ];

    for feature in &features {
        println!("cargo:rerun-if-env-changed={}", feature_to_cargo(feature));
    }
    println!("cargo:rerun-if-env-changed=CARGO_CFG_SANITIZE");
    println!("cargo:rerun-if-env-changed=POCKET_LLVM");

    let src_dir = Path::new("quickjs");

    let out_dir = env::var("OUT_DIR").expect("No OUT_DIR env var is set by cargo");
    let out_dir = Path::new(&out_dir);

    let header_files = [
        "builtin-array-fromasync.h",
        "builtin-iterator-zip-keyed.h",
        "builtin-iterator-zip.h",
        "cutils.h",
        "dtoa.h",
        "libregexp-opcode.h",
        "libregexp.h",
        "libunicode-table.h",
        "libunicode.h",
        "list.h",
        "quickjs-atom.h",
        "quickjs-opcode.h",
        "quickjs-c-atomics.h",
        "quickjs.h",
    ];

    let source_files = ["libregexp.c", "libunicode.c", "quickjs.c", "dtoa.c"];

    let mut defines: Vec<(String, Option<&str>)> = vec![("_GNU_SOURCE".into(), None)];

    #[cfg(feature = "disable-assertions")]
    defines.push(("NDEBUG".into(), None));

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap();

    let mut builder = cc::Build::new();
    // Amoris P7: the pinned clang when no compiler is named for the target (pocket_compiler).
    let clang_cl = pocket_compiler(&mut builder);
    builder
        .extra_warnings(false)
        .flag_if_supported("-Wno-implicit-const-int-float-conversion")
        //.flag("-Wno-array-bounds")
        //.flag("-Wno-format-truncation")
        ;
    // Amoris P4: no fused multiply-add on one platform and not another. On the MSVC target the
    // CFLAGS of .cargo/config.toml are replaced below, so the flag is passed here too: clang-cl
    // takes it through /clang:, and MSVC's cl contracts only under /fp:contract, never passed.
    if clang_cl {
        builder.flag("/clang:-ffp-contract=off");
    } else {
        builder.flag_if_supported("-ffp-contract=off");
    }

    match env::var("CARGO_CFG_SANITIZE").as_deref() {
        Ok("address") => {
            builder
                .flag("-fsanitize=address")
                .flag("-fno-sanitize-recover=all")
                .flag("-fno-omit-frame-pointer");
        }
        Ok("memory") => {
            builder
                .flag("-fsanitize=memory")
                .flag("-fno-sanitize-recover=all")
                .flag("-fno-omit-frame-pointer");
        }
        Ok("thread") => {
            builder
                .flag("-fsanitize=thread")
                .flag("-fno-sanitize-recover=all")
                .flag("-fno-omit-frame-pointer");
        }
        Ok(x) => println!("cargo:warning=Unsupported sanitize_option: '{x}'"),
        _ => {}
    }

    let mut bindgen_cflags = vec![];

    if target_os == "windows" {
        if target_env == "msvc" {
            env::set_var(
                "CFLAGS",
                "/DWIN32_LEAN_AND_MEAN /std:c11 /experimental:c11atomics",
            );
        } else {
            env::set_var("CFLAGS", "-DWIN32_LEAN_AND_MEAN -std=c11");
        }
    }

    // wasm32-unknown-unknown takes the same emscripten-flavoured config as
    // wasi, but ships no libc: headers and the prebuilt libc.a come from
    // vendor/wasi-libc (see scripts/vendor-wasm-libc.sh), and the OS tail
    // (clock, stdio, abort) is supplied by wasm-shim/shim.c.
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let is_wasm_unknown = target_arch == "wasm32" && target_os == "unknown";

    if target_os == "wasi" || is_wasm_unknown {
        // pretend we're emscripten - there are already ifdefs that match
        // also, wasi doesn't ahve FE_DOWNWARD or FE_UPWARD
        defines.push(("EMSCRIPTEN".into(), Some("1")));
        defines.push(("FE_DOWNWARD".into(), Some("0")));
        defines.push(("FE_UPWARD".into(), Some("0")));
    }

    if is_wasm_unknown {
        println!("cargo:rerun-if-changed=vendor/wasi-libc/include");
        // Amoris P7: an absolute path, not `canonicalize`, which gives a verbatim `\\?\` path
        // on Windows that takes no `/` separator and so hides the nested wasi-libc headers.
        let vendor_include = std::path::absolute("vendor/wasi-libc/include")
            .expect("vendor/wasi-libc/include is missing; run scripts/vendor-wasm-libc.sh");
        let flag = format!("-isystem{}", vendor_include.display());
        builder.flag(&flag);
        bindgen_cflags.push(flag);
    }

    for file in source_files.iter().chain(header_files.iter()) {
        fs::copy(src_dir.join(file), out_dir.join(file))
            .expect("Unable to copy source; try 'git submodule update --init'");
    }
    fs::copy("quickjs.bind.h", out_dir.join("quickjs.bind.h")).expect("Unable to copy source");

    if target_os == "wasi" && !matches!(env::var("RQUICKJS_SYS_NO_WASI_SDK").as_deref(), Ok("1")) {
        let wasi_sdk_path = get_wasi_sdk_path();
        if !wasi_sdk_path.try_exists().unwrap() {
            panic!(
                "wasi-sdk not installed in specified path of {}",
                wasi_sdk_path.display()
            );
        }
        env::set_var("CC", wasi_sdk_path.join("bin/clang").to_str().unwrap());
        env::set_var("AR", wasi_sdk_path.join("bin/ar").to_str().unwrap());
        let sysroot = format!(
            "--sysroot={}",
            wasi_sdk_path.join("share/wasi-sysroot").display()
        );
        env::set_var("CFLAGS", &sysroot);
        bindgen_cflags.push(sysroot);
    }

    // generating bindings
    bindgen(
        out_dir,
        out_dir.join("quickjs.bind.h"),
        &defines,
        bindgen_cflags,
    );

    for (name, value) in &defines {
        builder.define(name, *value);
    }

    for src in &source_files {
        builder.file(out_dir.join(src));
    }

    if is_wasm_unknown {
        // The shim goes into libquickjs.a rather than its own archive: lld
        // resolves an archive to a fixpoint, so every definition here is
        // picked up before libc.a is reached and the matching libc.a member
        // (and whatever WASI import it would have needed) is never
        // extracted.
        println!("cargo:rerun-if-changed=wasm-shim/shim.c");
        builder.file("wasm-shim/shim.c");
    }

    // Amoris P7: which C compiler built QuickJS-ng, for EngineVersion and the perf step.
    let tool = builder.get_compiler();
    println!("cargo:rustc-env=POCKET_QJS_CC={}", compiler_id(&tool));
    // Amoris P7: MSVC's cl, the fallback when the pinned clang-cl is missing, lays out frames that
    // take over three times clang-cl's stack per call, so `POCKET_QJS_MSVC` lets the script host's
    // default stack limit follow it, and its timings are not the reference figures': every build
    // says so.
    println!("cargo:rustc-check-cfg=cfg(pocket_qjs_msvc)");
    if tool.is_like_msvc() && !tool.is_like_clang_cl() {
        println!("cargo:rustc-cfg=pocket_qjs_msvc");
        println!(
            "cargo:warning=QuickJS-ng is compiled by MSVC's cl ({}), not clang-cl: the pinned LLVM \
             (POCKET_LLVM, else ~/.pocket-tools/llvm-23.1.2) has no clang-cl, or the build named \
             cl. Scripts get a larger stack limit and run slower than with clang-cl \
             (docs/spec/architecture.md 7.3).",
            tool.path().display()
        );
    }

    builder.compile("libquickjs.a");

    // Emitted after `compile` so `-lquickjs` precedes `-lc` on the link line.
    if is_wasm_unknown {
        println!("cargo:rerun-if-changed=vendor/wasi-libc/lib/libc.a");
        let vendor_lib = std::path::absolute("vendor/wasi-libc/lib")
            .expect("vendor/wasi-libc/lib is missing; run scripts/vendor-wasm-libc.sh");
        println!("cargo:rustc-link-search=native={}", vendor_lib.display());
        println!("cargo:rustc-link-lib=static=c");
    }
}

/// Amoris P7: QuickJS-ng is compiled by clang-cl on the Windows MSVC target and by clang for
/// wasm32-unknown-unknown (docs/spec/architecture.md 7.3 and 8.1). When the build names no
/// compiler for the target (`CC_<target>`, `TARGET_CC`, `CC`), the pinned LLVM is used if it is
/// installed: `$POCKET_LLVM`, else `~/.pocket-tools/llvm-23.1.2`. Every build of a checkout then
/// compiles QuickJS-ng the same way, whether or not `cargo xtask` started it. Returns whether the
/// compiler is clang-cl (decided from its name: asking cc would cache CFLAGS before the MSVC
/// branch below sets them).
fn pocket_compiler(builder: &mut cc::Build) -> bool {
    let target = env::var("TARGET").unwrap();
    let named = [
        format!("CC_{target}"),
        format!("CC_{}", target.replace('-', "_")),
        "TARGET_CC".to_string(),
        "CC".to_string(),
    ];
    if let Some(cc) = named.iter().find_map(env::var_os) {
        return Path::new(&cc).file_stem().is_some_and(|s| s == "clang-cl");
    }
    let (cc, ar) = match target.as_str() {
        "x86_64-pc-windows-msvc" | "aarch64-pc-windows-msvc" => ("clang-cl", None),
        "wasm32-unknown-unknown" => ("clang", Some("llvm-ar")),
        _ => return false,
    };
    let home = env::var_os("USERPROFILE").or_else(|| env::var_os("HOME"));
    // amoris: Homebrew's LLVM (which has the wasm32 backend Apple's clang lacks) when neither
    // POCKET_LLVM nor the pinned toolchain is there.
    let homebrew = ["/opt/homebrew/opt/llvm", "/usr/local/opt/llvm"]
        .into_iter()
        .map(PathBuf::from)
        .find(|p| p.join("bin").join("clang").is_file());
    let llvm = env::var_os("POCKET_LLVM")
        .map(PathBuf::from)
        .or_else(|| {
            home.map(|h| PathBuf::from(h).join(".pocket-tools").join("llvm-23.1.2"))
                .filter(|p| p.is_dir())
        })
        .or(homebrew);
    let Some(bin) = llvm.map(|l| l.join("bin")) else {
        return false;
    };
    let exe = |name: &str| {
        let p = bin.join(format!("{name}{}", env::consts::EXE_SUFFIX));
        p.is_file().then_some(p)
    };
    if let Some(c) = exe(cc) {
        builder.compiler(c);
        if let Some(a) = ar.and_then(exe) {
            builder.archiver(a);
        }
        return cc == "clang-cl";
    }
    false
}

/// Amoris P7: the compiler's family and the first line of its `--version`.
fn compiler_id(tool: &cc::Tool) -> String {
    let name = tool
        .path()
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let version = process::Command::new(tool.path())
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| {
            let text = String::from_utf8_lossy(&o.stdout).into_owned()
                + &String::from_utf8_lossy(&o.stderr);
            text.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_owned)
        })
        .unwrap_or_default();
    format!("{name}: {version}").replace(['\r', '\n'], " ")
}

fn feature_to_cargo(name: impl AsRef<str>) -> String {
    format!("CARGO_FEATURE_{}", feature_to_define(name))
}

fn feature_to_define(name: impl AsRef<str>) -> String {
    name.as_ref().to_uppercase().replace('-', "_")
}

#[cfg(not(feature = "bindgen"))]
fn bindgen<'a, D, H, X, K, V>(out_dir: D, _header_file: H, _defines: X, _add_cflags: Vec<String>)
where
    D: AsRef<Path>,
    H: AsRef<Path>,
    X: IntoIterator<Item = &'a (K, Option<V>)>,
    K: AsRef<str> + 'a,
    V: AsRef<str> + 'a,
{
    let target = env::var("TARGET").unwrap();

    if !Path::new("./")
        .join("src")
        .join("bindings")
        .join(format!("{}.rs", target))
        .canonicalize()
        .map(|x| x.exists())
        .unwrap_or(false)
    {
        println!(
            "cargo:warning=rquickjs probably doesn't ship bindings for platform `{}({})`. try the `bindgen` feature instead.",
            target,
            env::var("BUILD_TARGET").unwrap_or("n/a".into())
        );
    }

    let bindings_file = out_dir.as_ref().join("bindings.rs");

    fs::write(
        bindings_file,
        format!(
            r#"macro_rules! bindings_env {{
                ("TARGET") => {{ "{target}" }};
            }}"#
        ),
    )
    .unwrap();
}

#[cfg(feature = "bindgen")]
fn bindgen<'a, D, H, X, K, V>(out_dir: D, header_file: H, defines: X, add_cflags: Vec<String>)
where
    D: AsRef<Path>,
    H: AsRef<Path>,
    X: IntoIterator<Item = &'a (K, Option<V>)>,
    K: AsRef<str> + 'a,
    V: AsRef<str> + 'a,
{
    let out_dir = out_dir.as_ref();
    let header_file = header_file.as_ref();

    let target = env::var("TARGET").unwrap();
    let host = env::var("HOST").unwrap();

    // When cross-compiling with the `macro` feature, sys also gets built for the host.
    // If LIBCLANG_PATH points at the cross toolchain (e.g. Android NDK), that host build
    // generates mismatched bindings, so reuse the bundled binding for the host instead.
    // `update-bindings` still regenerates.
    if target == host && env::var("CARGO_FEATURE_UPDATE_BINDINGS").is_err() {
        let bundled = Path::new("src")
            .join("bindings")
            .join(format!("{}.rs", target));
        if bundled.exists() {
            println!(
                "cargo:warning=using bundled bindings for host target `{}` instead of running bindgen (enable the `update-bindings` feature to regenerate)",
                target
            );
            fs::copy(&bundled, out_dir.join("bindings.rs"))
                .expect("Unable to copy bundled bindings");
            return;
        }
    }

    let mut cflags = add_cflags;

    //format!("-I{}", out_dir.parent().display()),

    for (name, value) in defines {
        cflags.push(if let Some(value) = value {
            format!("-D{}={}", name.as_ref(), value.as_ref())
        } else {
            format!("-D{}", name.as_ref())
        });
    }

    let mut builder = bindgen_rs::Builder::default()
        .use_core()
        .detect_include_paths(true)
        .clang_arg("-xc")
        .clang_arg("-v")
        .clang_args(cflags)
        .size_t_is_usize(false)
        .header(header_file.display().to_string())
        .allowlist_type("JS.*")
        .allowlist_function("js.*")
        .allowlist_function("JS.*")
        .allowlist_function("__JS.*")
        .allowlist_var("JS.*")
        .opaque_type("FILE")
        .blocklist_type("FILE")
        .blocklist_function("JS_DumpMemoryUsage");

    if env::var("CARGO_CFG_TARGET_OS").unwrap() == "wasi" {
        builder = builder.clang_arg("-fvisibility=default");
    }

    let bindings = builder.generate().expect("Unable to generate bindings");

    let bindings_file = out_dir.join("bindings.rs");

    bindings
        .write_to_file(&bindings_file)
        .expect("Couldn't write bindings");

    // Special case to support bundled bindings
    if env::var("CARGO_FEATURE_UPDATE_BINDINGS").is_ok() {
        let dest_dir = Path::new("src").join("bindings");
        fs::create_dir_all(&dest_dir).unwrap();

        let dest_file = format!("{}.rs", env::var("TARGET").unwrap());
        fs::copy(&bindings_file, dest_dir.join(dest_file)).unwrap();
    }
}
