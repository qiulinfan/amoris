//! The backend a native host draws and computes water with when `AMORIS_BACKEND` names none (the
//! owner, 2026-10-24): on Windows, Direct3D 12 when a supported DXC is found, and Vulkan when
//! Direct3D 12 would have to compile with FXC, which compiles neither the water's surface
//! (`fs_shallow`, whose invalid pipeline, bound for every water region every frame, voids the submit
//! of every frame of a scene with a water region) nor its deterministic kernels
//! (docs/bench/gpu-matrix.md 14), so that a machine without the Windows SDK still draws water;
//! Direct3D 12 with FXC when it has no Vulkan adapter either. `AMORIS_DXC=fxc` asks for FXC and keeps
//! Direct3D 12. Elsewhere, and whenever `AMORIS_BACKEND` names a backend (anything but `auto` or a
//! value it does not know), [`BackendChoice::from_env`]. Kept out of gpu.rs, which the water's
//! kernel key hashes.

use std::sync::OnceLock;

use crate::gpu::{self, BackendChoice};

/// The choice for `choice` (from `AMORIS_BACKEND`) on a host that runs Windows or not, where
/// `AMORIS_DXC=fxc` asked for FXC or not and Direct3D 12 would compile with FXC or not; `vulkan`
/// says whether a Vulkan adapter exists, asked only when that decides.
fn pick(
    choice: BackendChoice,
    windows: bool,
    fxc_asked: bool,
    fxc: bool,
    vulkan: impl FnOnce() -> bool,
) -> BackendChoice {
    if choice != BackendChoice::Auto || !windows || fxc_asked || !fxc || !vulkan() {
        return choice;
    }
    BackendChoice::Vulkan
}

/// The host's backend, worked out once a process (looking for DXC reads the Windows SDK's
/// directories, and asking for a Vulkan adapter loads the Vulkan loader).
pub fn host_backend() -> BackendChoice {
    static CHOICE: OnceLock<BackendChoice> = OnceLock::new();
    *CHOICE.get_or_init(|| {
        let choice = BackendChoice::from_env();
        if choice != BackendChoice::Auto || !cfg!(windows) {
            return choice;
        }
        let fxc_asked = std::env::var("AMORIS_DXC").is_ok_and(|v| v.eq_ignore_ascii_case("fxc"));
        let dxc = gpu::dx12_compiler();
        let fxc = matches!(dxc.compiler, wgpu::Dx12Compiler::Fxc);
        let picked = pick(choice, true, fxc_asked, fxc, || {
            let instance = gpu::instance(BackendChoice::Vulkan);
            !pollster::block_on(instance.enumerate_adapters(wgpu::Backends::VULKAN)).is_empty()
        });
        if picked == BackendChoice::Vulkan {
            // A DXC that was named or found but could not be used is worth a warning, as
            // gpu::instance_with gives one on Direct3D 12: the run is not on the backend meant.
            let level = if dxc.degraded {
                log::Level::Warn
            } else {
                log::Level::Info
            };
            log::log!(
                level,
                "Vulkan: Direct3D 12 would compile with {}, under which no frame of a scene with a \
                 water region draws (DXC 1.8.2502 or newer, from the Windows SDK or AMORIS_DXC, \
                 brings Direct3D 12 back)",
                dxc.why
            );
        } else if fxc && !fxc_asked {
            log::warn!(
                "Direct3D 12 with {}, and no Vulkan adapter: no frame of a scene with a water region \
                 draws",
                dxc.why
            );
        }
        picked
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_without_dxc_runs_vulkan_and_nothing_else_changes() {
        let auto = BackendChoice::Auto;
        let vulkan = || true;
        // No supported DXC on Windows, a Vulkan adapter: Vulkan.
        assert_eq!(pick(auto, true, false, true, vulkan), BackendChoice::Vulkan);
        // No Vulkan adapter either: Direct3D 12 with FXC, as before.
        assert_eq!(pick(auto, true, false, true, || false), auto);
        // A supported DXC: Direct3D 12, without asking for Vulkan.
        assert_eq!(pick(auto, true, false, false, || unreachable!()), auto);
        // FXC asked for, a backend named, or not Windows: as given.
        assert_eq!(pick(auto, true, true, true, || unreachable!()), auto);
        for named in [
            BackendChoice::Dx12,
            BackendChoice::Vulkan,
            BackendChoice::Metal,
        ] {
            assert_eq!(pick(named, true, false, true, || unreachable!()), named);
        }
        assert_eq!(pick(auto, false, false, true, || unreachable!()), auto);
    }
}
