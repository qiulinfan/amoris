//! Embed only the new foundation's source identity; no runtime checkout reads.
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn collect(root: &Path, dir: &Path, files: &mut Vec<String>) {
    println!("cargo:rerun-if-changed={}", dir.display());
    for entry in fs::read_dir(dir).expect("source directory") {
        let path = entry.expect("source entry").path();
        if path.is_dir() {
            collect(root, &path, files);
        } else if path
            .extension()
            .is_some_and(|e| matches!(e.to_str(), Some("rs" | "toml" | "txt")))
        {
            files.push(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
}

fn main() {
    let crate_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let root = crate_dir.parent().unwrap().parent().unwrap();
    let mut files = vec![
        "Cargo.toml".to_owned(),
        "Cargo.lock".to_owned(),
        "rust-toolchain.toml".to_owned(),
        ".cargo/config.toml".to_owned(),
        "tools/crate-graph.toml".to_owned(),
        "tools/imports.toml".to_owned(),
    ];
    for dir in ["crates", "shared/contract/rust"] {
        collect(root, &root.join(dir), &mut files);
    }
    files.sort();
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
    };
    let commit = git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unavailable".to_owned());
    let dirty = git(&["status", "--porcelain"]).is_some_and(|s| !s.is_empty());
    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        println!("cargo:rerun-if-changed={dir}/HEAD");
    }
    if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"])
        && let Some(path) = git(&["rev-parse", "--git-path", &reference])
    {
        println!("cargo:rerun-if-changed={}", root.join(path).display());
    }
    let mut code = format!(
        "const BUILD_COMMIT: &str = {:?};\nconst BUILD_TARGET: &str = {:?};\nconst BUILD_PROFILE: &str = {:?};\n",
        format!("{commit}{}", if dirty { "+dirty" } else { "" }),
        std::env::var("TARGET").unwrap(),
        std::env::var("PROFILE").unwrap()
    );
    code.push_str("const SOURCE_FILES: &[(&str, &[u8])] = &[\n");
    for file in files {
        println!("cargo:rerun-if-changed={}", root.join(&file).display());
        code.push_str(&format!(
            "({file:?}, include_bytes!({:?})),\n",
            root.join(&file).to_string_lossy()
        ));
    }
    code.push_str("];\n");
    fs::write(
        PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("source.rs"),
        code,
    )
    .unwrap();
}
