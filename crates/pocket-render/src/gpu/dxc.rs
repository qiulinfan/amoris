//! The Direct3D 12 shader compiler: DXC from `POCKET_DXC` or from the newest Windows SDK whose
//! `dxcompiler.dll` wgpu supports, FXC otherwise (docs/bench/dx12.md 1 and 2.3).
//!
//! wgpu supports DXC 1.8.2502 and newer (wgpu-types' `Dx12Compiler::DynamicDxc`). Older DXCs sign
//! their output through a `dxil.dll` they look for on the DLL search path, which loading
//! `dxcompiler.dll` by its full path does not extend: DXC 1.6 from SDK 10.0.22621 compiles with
//! "DXIL.dll not found" and the device is lost as soon as wgpu builds its first pipeline (its
//! indirect validation, during `request_device`). So each candidate's file version is read from its
//! version resource before it is chosen, and an older one is passed over.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// The oldest DXC wgpu supports: major, minor, build.
const MIN_DXC: [u16; 3] = [1, 8, 2502];

/// The Direct3D 12 shader compiler chosen, with why (for the log).
pub struct Dx12CompilerChoice {
    pub compiler: wgpu::Dx12Compiler,
    pub why: String,
    /// A `dxcompiler.dll` was passed over or could not be used: worth a warning.
    pub degraded: bool,
}

/// A `dxcompiler.dll` to consider and where it came from.
#[derive(Debug)]
struct Candidate {
    from: String,
    path: PathBuf,
}

/// DXC from `POCKET_DXC` (a DLL or its directory) or the newest Windows SDK that has a supported
/// one; FXC with `POCKET_DXC=fxc` or when none is usable. DXC is loaded at run time; wgpu's
/// `static-dxc` feature (which downloads binaries at build time) is not used.
pub fn dx12_compiler() -> Dx12CompilerChoice {
    let candidates = match std::env::var("POCKET_DXC") {
        Ok(v) if v.eq_ignore_ascii_case("fxc") => {
            return fxc("POCKET_DXC=fxc".into(), false);
        }
        Ok(v) if !v.is_empty() => {
            let mut path = PathBuf::from(&v);
            if path.is_dir() {
                path.push("dxcompiler.dll");
            }
            if !path.is_file() {
                return fxc(format!("POCKET_DXC={v} has no dxcompiler.dll"), true);
            }
            vec![Candidate {
                from: "POCKET_DXC".into(),
                path,
            }]
        }
        _ => sdk_dxcs(),
    };
    if candidates.is_empty() {
        return fxc("no Windows SDK dxcompiler.dll found".into(), false);
    }
    let (found, passed_over) = first_supported(candidates, file_version);
    let passed_over = passed_over.join("; ");
    match found {
        Some((c, version)) => Dx12CompilerChoice {
            why: format!(
                "DXC {} {} ({}){}",
                dotted(&version),
                c.path.display(),
                c.from,
                if passed_over.is_empty() {
                    String::new()
                } else {
                    format!("; passed over {passed_over}")
                }
            ),
            degraded: !passed_over.is_empty(),
            compiler: wgpu::Dx12Compiler::DynamicDxc {
                dxc_path: c.path.display().to_string(),
            },
        },
        None => fxc(format!("no supported DXC: {passed_over}"), true),
    }
}

fn fxc(why: String, degraded: bool) -> Dx12CompilerChoice {
    Dx12CompilerChoice {
        compiler: wgpu::Dx12Compiler::Fxc,
        why: format!("FXC ({why})"),
        degraded,
    }
}

/// Every Windows SDK's `bin\<version>\x64\dxcompiler.dll`, newest SDK first (versions compared
/// numerically).
fn sdk_dxcs() -> Vec<Candidate> {
    if !cfg!(all(windows, target_arch = "x86_64")) {
        return Vec::new();
    }
    let base = std::env::var_os("ProgramFiles(x86)")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"))
        .join("Windows Kits")
        .join("10")
        .join("bin");
    let Ok(dir) = std::fs::read_dir(&base) else {
        return Vec::new();
    };
    let mut found: Vec<(Vec<u32>, Candidate)> = dir
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let version = sdk_version(&name)?;
            let path = entry.path().join("x64").join("dxcompiler.dll");
            path.is_file().then(|| {
                let from = format!("Windows SDK {name}");
                (version, Candidate { from, path })
            })
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().map(|(_, c)| c).collect()
}

/// `10.0.26100.0` as numbers; `None` for anything else (`x64`, `arm64`).
fn sdk_version(name: &str) -> Option<Vec<u32>> {
    let parts: Option<Vec<u32>> = name.split('.').map(|p| p.parse().ok()).collect();
    parts.filter(|p| p.len() == 4)
}

/// The first candidate whose version (`version` reads it) is at least [`MIN_DXC`], and why each
/// one before it was passed over.
fn first_supported(
    candidates: Vec<Candidate>,
    version: impl Fn(&Path) -> Option<[u16; 4]>,
) -> (Option<(Candidate, [u16; 4])>, Vec<String>) {
    let mut passed_over = Vec::new();
    for c in candidates {
        match version(&c.path) {
            Some(v) if v[..3] >= MIN_DXC[..] => return (Some((c, v)), passed_over),
            Some(v) => passed_over.push(format!(
                "{} ({}): DXC {} is older than {}",
                c.path.display(),
                c.from,
                dotted(&v),
                dotted(&MIN_DXC)
            )),
            None => passed_over.push(format!(
                "{} ({}): no version resource",
                c.path.display(),
                c.from
            )),
        }
    }
    (None, passed_over)
}

fn dotted(parts: &[u16]) -> String {
    let parts: Vec<String> = parts.iter().map(u16::to_string).collect();
    parts.join(".")
}

/// A Windows DLL's file version (major, minor, build, private) from its version resource. The PE
/// headers locate the section that holds the resource directory, which is read whole (a few KB)
/// and searched for the `VS_FIXEDFILEINFO` block. Reading the file keeps this free of Win32 calls
/// and lets it run on any platform; a DLL that embeds another PE as a resource could report that
/// one's version, which Microsoft's compiler DLLs do not do.
fn file_version(path: &Path) -> Option<[u16; 4]> {
    let mut file = File::open(path).ok()?;
    let mut head = Vec::new();
    file.by_ref().take(4096).read_to_end(&mut head).ok()?;
    let (offset, len) = resource_section(&head)?;
    let mut resources = vec![0; len];
    file.seek(SeekFrom::Start(offset)).ok()?;
    file.read_exact(&mut resources).ok()?;
    fixed_file_version(&resources)
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// The file offset and size of the section holding a PE file's resource directory, from its
/// headers (`head`, the start of the file).
fn resource_section(head: &[u8]) -> Option<(u64, usize)> {
    if head.get(..2)? != b"MZ" {
        return None;
    }
    let pe = u32_at(head, 0x3c)? as usize;
    if head.get(pe..pe + 4)? != b"PE\0\0" {
        return None;
    }
    let sections = usize::from(u16_at(head, pe + 6)?);
    let optional = pe + 24;
    let optional_size = usize::from(u16_at(head, pe + 20)?);
    let directories = match u16_at(head, optional)? {
        0x10b => optional + 96,  // PE32
        0x20b => optional + 112, // PE32+
        _ => return None,
    };
    // Data directory 2 is the resource table: its relative virtual address, then its size.
    let resource_rva = u64::from(u32_at(head, directories + 2 * 8)?);
    if resource_rva == 0 {
        return None;
    }
    (0..sections).find_map(|i| {
        let s = optional + optional_size + i * 40;
        let virtual_size = u64::from(u32_at(head, s + 8)?);
        let virtual_address = u64::from(u32_at(head, s + 12)?);
        let raw_size = u32_at(head, s + 16)?;
        let raw_offset = u64::from(u32_at(head, s + 20)?);
        let end = virtual_address + virtual_size.max(u64::from(raw_size));
        // A version resource is small; refuse absurd sizes rather than allocate them.
        ((virtual_address..end).contains(&resource_rva) && raw_size <= 64 << 20)
            .then_some((raw_offset, raw_size as usize))
    })
}

/// The file version in a `VS_FIXEDFILEINFO`: the signature 0xFEEF04BD and structure version 1.0,
/// then the version's most and least significant 32 bits.
fn fixed_file_version(resources: &[u8]) -> Option<[u16; 4]> {
    const START: [u8; 8] = [0xbd, 0x04, 0xef, 0xfe, 0x00, 0x00, 0x01, 0x00];
    let at = resources.windows(8).position(|w| w == START)?;
    let ms = u32_at(resources, at + 8)?;
    let ls = u32_at(resources, at + 12)?;
    Some([(ms >> 16) as u16, ms as u16, (ls >> 16) as u16, ls as u16])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_versions_order_numerically() {
        assert_eq!(sdk_version("10.0.26100.0"), Some(vec![10, 0, 26100, 0]));
        assert_eq!(sdk_version("x64"), None);
        assert_eq!(sdk_version("10.0.1"), None);
        assert!(sdk_version("10.0.26100.0") > sdk_version("10.0.22621.0"));
        assert!(sdk_version("10.0.9200.0") < sdk_version("10.0.10240.0"));
    }

    fn candidates(names: &[&str]) -> Vec<Candidate> {
        names
            .iter()
            .map(|n| Candidate {
                from: format!("Windows SDK {n}"),
                path: PathBuf::from(n),
            })
            .collect()
    }

    /// Fake versions by path, standing in for the DLLs.
    fn versions<'a>(table: &'a [(&'a str, [u16; 4])]) -> impl Fn(&Path) -> Option<[u16; 4]> + 'a {
        move |p: &Path| {
            table
                .iter()
                .find(|(n, _)| Path::new(n) == p)
                .map(|(_, v)| *v)
        }
    }

    #[test]
    fn the_newest_sdk_wins_when_its_dxc_is_supported() {
        let table = [
            ("10.0.26100.0", [1, 8, 2502, 11]),
            ("10.0.22621.0", [1, 6, 2112, 16]),
        ];
        let (found, passed_over) = first_supported(
            candidates(&["10.0.26100.0", "10.0.22621.0"]),
            versions(&table),
        );
        let (c, v) = found.expect("a DXC");
        assert_eq!(c.path, Path::new("10.0.26100.0"));
        assert_eq!(v, [1, 8, 2502, 11]);
        assert!(passed_over.is_empty());
    }

    #[test]
    fn an_old_dxc_is_passed_over_for_the_next_sdk_or_for_fxc() {
        // The newest SDK carries DXC 1.6 (SDK 10.0.22621, Visual Studio 2022's default for long):
        // an older SDK with a supported DXC is used instead, and with none, nothing is.
        let table = [
            ("10.0.22621.0", [1, 6, 2112, 16]),
            ("10.0.20348.0", [1, 8, 2502, 0]),
        ];
        let (found, passed_over) = first_supported(
            candidates(&["10.0.22621.0", "10.0.20348.0"]),
            versions(&table),
        );
        assert_eq!(found.expect("a DXC").0.path, Path::new("10.0.20348.0"));
        assert_eq!(passed_over.len(), 1);
        assert!(
            passed_over[0].contains("DXC 1.6.2112.16 is older than 1.8.2502"),
            "{passed_over:?}"
        );

        let (found, passed_over) = first_supported(candidates(&["10.0.22621.0"]), versions(&table));
        assert!(found.is_none());
        assert_eq!(passed_over.len(), 1);

        // A DLL whose version cannot be read is not trusted either.
        let (found, passed_over) = first_supported(candidates(&["unreadable"]), versions(&table));
        assert!(found.is_none());
        assert!(passed_over[0].contains("no version resource"));
    }

    #[test]
    fn versions_compare_by_major_minor_build() {
        let ok = |v: [u16; 4]| {
            first_supported(candidates(&["dxc"]), move |_: &Path| Some(v))
                .0
                .is_some()
        };
        assert!(ok([1, 8, 2502, 0]));
        assert!(ok([1, 8, 2505, 1]));
        assert!(ok([1, 9, 0, 0]));
        assert!(ok([2, 0, 0, 0]));
        assert!(!ok([1, 8, 2407, 99]));
        assert!(!ok([1, 7, 9999, 0]));
    }

    #[test]
    fn fixed_file_info_is_found_in_a_resource_section() {
        let mut rsrc = vec![0u8; 96];
        rsrc[40..48].copy_from_slice(&[0xbd, 0x04, 0xef, 0xfe, 0x00, 0x00, 0x01, 0x00]);
        rsrc[48..52].copy_from_slice(&((1u32 << 16) | 8).to_le_bytes());
        rsrc[52..56].copy_from_slice(&((2502u32 << 16) | 11).to_le_bytes());
        assert_eq!(fixed_file_version(&rsrc), Some([1, 8, 2502, 11]));
        // The signature without the structure version is not a match.
        rsrc[44] = 7;
        assert_eq!(fixed_file_version(&rsrc), None);
        assert_eq!(fixed_file_version(&[]), None);
        assert_eq!(resource_section(b"not a PE file"), None);
    }

    /// Real DLLs: kernel32 is Windows 10's or 11's (major 10) and every SDK's `dxcompiler.dll` is
    /// some DXC 1.x.
    #[test]
    #[cfg(windows)]
    fn windows_dll_versions_read() {
        let system = std::env::var_os("SystemRoot")
            .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        let kernel32 = file_version(&system.join("System32").join("kernel32.dll"));
        assert_eq!(kernel32.map(|v| v[0]), Some(10), "{kernel32:?}");
        for c in sdk_dxcs() {
            let v = file_version(&c.path);
            assert_eq!(v.map(|v| v[0]), Some(1), "{}: {v:?}", c.path.display());
        }
    }
}
