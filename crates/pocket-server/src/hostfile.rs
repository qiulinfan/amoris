//! `.pocket/host.json` (docs/spec/server.md, CLI): where a running host serves a project, so the
//! `pocket` CLI and `pocket mcp` find it from the project's directory.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file's content.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HostFile {
    /// `http://127.0.0.1:<port>`.
    pub url: String,
    pub port: u16,
    pub pid: u32,
    /// The project's directory.
    pub project: String,
}

/// Where the file of `project` is.
pub fn path(project: &Path) -> PathBuf {
    project.join(".pocket").join("host.json")
}

/// Writes the file for a host on `port`.
pub fn write(project: &Path, port: u16) -> std::io::Result<HostFile> {
    let f = HostFile {
        url: format!("http://127.0.0.1:{port}"),
        port,
        pid: std::process::id(),
        project: project.display().to_string(),
    };
    let p = path(project);
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(&f).unwrap_or_default();
    std::fs::write(&p, text + "\n")?;
    Ok(f)
}

/// Removes the file if it still names this process.
pub fn remove(project: &Path) {
    let p = path(project);
    if read(&p).is_some_and(|f| f.pid == std::process::id()) {
        let _ = std::fs::remove_file(p);
    }
}

/// Reads a host file.
pub fn read(p: &Path) -> Option<HostFile> {
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

/// The nearest host file at or above `dir`.
pub fn find(dir: &Path) -> Option<(PathBuf, HostFile)> {
    dir.ancestors().find_map(|d| {
        let p = path(d);
        read(&p).map(|f| (p, f))
    })
}

/// The project directory `dir` as a host reports it (its file, its readiness line, `project.info`):
/// absolute, with links resolved, and in the form other programs resolve it to. `canonicalize`
/// gives a verbatim path on Windows (`\\?\C:\...`), which no other program's canonical form (Node's
/// `fs.realpath`, the desktop's) equals; [`plain`] drops that prefix where it can.
pub fn project_root(dir: &Path) -> std::io::Result<PathBuf> {
    dir.canonicalize().map(|p| plain(&p))
}

/// The verbatim prefix, and its UNC form.
const VERBATIM: &str = r"\\?\";
const VERBATIM_UNC: &str = r"\\?\UNC\";

/// `path` without Windows' verbatim prefix when it means the same without it: `\\?\C:\a` is
/// `C:\a`, `\\?\UNC\server\share\a` is `\\server\share\a`. A path only the verbatim form can name
/// keeps it: one of 260 characters or more (MAX_PATH), a component Win32 would change or refuse
/// (`.` or `..`, a trailing dot or space, a reserved device name such as `CON` or `nul.txt`, a
/// character Win32 forbids), another verbatim prefix (`\\?\Volume{...}`), or text that is not
/// UTF-8. Every other path, on every system, is returned as it is.
pub fn plain(path: &Path) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };
    // The plain form, and how many of its names are the drive, or the server and the share.
    let (simple, root_names) = if let Some(rest) = text.strip_prefix(VERBATIM_UNC) {
        (format!(r"\\{rest}"), 2)
    } else if let Some(rest) = text.strip_prefix(VERBATIM)
        && let [drive, b':', b'\\', ..] = rest.as_bytes()
        && drive.is_ascii_alphabetic()
    {
        (rest.to_owned(), 1)
    } else {
        return path.to_path_buf();
    };
    let representable = simple.len() < 260
        && simple
            .trim_start_matches('\\')
            .split('\\')
            .skip(root_names)
            .filter(|name| !name.is_empty())
            .all(win32_name);
    if representable {
        PathBuf::from(simple)
    } else {
        path.to_path_buf()
    }
}

/// Whether Win32 takes `name` as it is: not `.` or `..`, no trailing dot or space, no character it
/// forbids, and not a reserved device name, with or without an extension.
fn win32_name(name: &str) -> bool {
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "COM¹", "COM²", "COM³", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7",
        "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³",
    ];
    if name == "." || name == ".." || name.ends_with(['.', ' ']) {
        return false;
    }
    if name
        .chars()
        .any(|c| c < ' ' || matches!(c, '<' | '>' | ':' | '"' | '/' | '|' | '?' | '*'))
    {
        return false;
    }
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    !RESERVED.iter().any(|r| stem.eq_ignore_ascii_case(r))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain_str(p: &str) -> String {
        plain(Path::new(p)).to_string_lossy().into_owned()
    }

    #[test]
    fn verbatim_paths_lose_the_prefix_only_where_they_mean_the_same() {
        assert_eq!(
            plain_str(r"\\?\C:\Users\me\Sailing"),
            r"C:\Users\me\Sailing"
        );
        assert_eq!(plain_str(r"\\?\d:\x"), r"d:\x");
        assert_eq!(plain_str(r"\\?\C:\"), r"C:\");
        assert_eq!(
            plain_str(r"\\?\UNC\server\share\proj"),
            r"\\server\share\proj"
        );
        // A console or a conductor is not a device.
        assert_eq!(
            plain_str(r"\\?\C:\console\CONductor"),
            r"C:\console\CONductor"
        );
        // Plain paths, and those only the verbatim form names, are kept.
        for kept in [
            r"C:\Users\me",
            "/home/me/Sailing",
            r"\\server\share\proj",
            r"\\?\Volume{0b1c}\proj",
            r"\\?\C:\a\CON",
            r"\\?\C:\a\nul.txt",
            r"\\?\C:\a\Com1 .log",
            r"\\?\C:\a\trailing.",
            r"\\?\C:\a\trailing ",
            r"\\?\C:\a\..\b",
            r"\\?\C:\a\what?",
            r"\\?\UNC\server\share\aux",
        ] {
            assert_eq!(plain_str(kept), kept);
        }
        // MAX_PATH counts the terminating NUL: 259 characters fit, 260 do not.
        let long = format!(r"\\?\C:\{}\{}", "a".repeat(200), "b".repeat(56));
        assert_eq!(long.len() - 4, 260);
        assert_eq!(plain_str(&long), long);
        let fits = format!(r"\\?\C:\{}", "a".repeat(250));
        assert_eq!(plain_str(&fits), &fits[4..]);
    }

    /// On Windows `canonicalize` gives a verbatim path, which the host reports without the prefix;
    /// elsewhere the canonical path is plain already.
    #[test]
    fn a_project_root_is_canonical_and_plain() {
        let dir = std::env::temp_dir();
        let root = project_root(&dir).unwrap();
        assert!(root.is_absolute(), "{}", root.display());
        assert!(
            !root.to_string_lossy().starts_with(VERBATIM),
            "{}",
            root.display()
        );
        assert_eq!(
            std::fs::canonicalize(&root).unwrap(),
            std::fs::canonicalize(&dir).unwrap()
        );
    }
}
