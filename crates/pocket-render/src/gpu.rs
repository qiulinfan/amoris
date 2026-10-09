//! The GPU: instance, adapter, device and queue, with the backend chosen explicitly (charter 4.1:
//! Metal on macOS, Vulkan elsewhere and on demand through MoltenVK, Direct3D 12 on Windows on
//! demand, WebGPU in the browser) and the optional features the renderer uses when the adapter has
//! them.
//!
//! Environment: `POCKET_BACKEND` (`metal`, `vulkan`, `dx12`), `POCKET_ADAPTER` (an adapter's index
//! or a case-insensitive part of its name, for machines with two GPUs) and, for Direct3D 12,
//! `POCKET_DXC` (the `dxcompiler.dll` to use, its directory, or `fxc`). `POCKET_INDIRECT_VALIDATION`
//! is a measurement switch (see [`instance`]).

use std::path::PathBuf;
use std::sync::Arc;

/// Which backend to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendChoice {
    /// Metal on Apple platforms, Vulkan elsewhere, WebGPU in the browser.
    Auto,
    Metal,
    Vulkan,
    /// Direct3D 12 (Windows).
    Dx12,
}

impl BackendChoice {
    /// From `POCKET_BACKEND` (`metal`, `vulkan` or `vk`, `dx12` or `d3d12`, any case); `Auto`
    /// otherwise.
    pub fn from_env() -> BackendChoice {
        std::env::var("POCKET_BACKEND")
            .ok()
            .and_then(|v| BackendChoice::parse(&v))
            .unwrap_or(BackendChoice::Auto)
    }

    /// A backend's name as `POCKET_BACKEND` spells it.
    pub fn parse(name: &str) -> Option<BackendChoice> {
        match name.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(BackendChoice::Auto),
            "metal" | "mtl" => Some(BackendChoice::Metal),
            "vulkan" | "vk" => Some(BackendChoice::Vulkan),
            "dx12" | "d3d12" | "direct3d12" => Some(BackendChoice::Dx12),
            _ => None,
        }
    }

    pub fn backends(self) -> wgpu::Backends {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = self;
            wgpu::Backends::BROWSER_WEBGPU
        }
        #[cfg(not(target_arch = "wasm32"))]
        match self {
            BackendChoice::Metal => wgpu::Backends::METAL,
            BackendChoice::Vulkan => wgpu::Backends::VULKAN,
            BackendChoice::Dx12 => wgpu::Backends::DX12,
            BackendChoice::Auto => {
                if cfg!(any(target_os = "macos", target_os = "ios")) {
                    wgpu::Backends::METAL
                } else {
                    wgpu::Backends::VULKAN
                }
            }
        }
    }
}

/// What the renderer may use beyond WebGPU's defaults.
#[derive(Clone, Copy, Debug, Default)]
pub struct Capabilities {
    /// `first_instance` in indirect draws: one multi-draw per pass for every batch.
    pub indirect_first_instance: bool,
    /// GPU timestamps around passes (the profiler).
    pub timestamps: bool,
    /// Half-precision arithmetic in shaders (neural decoding).
    pub shader_f16: bool,
    pub float32_filterable: bool,
}

/// The device the renderer draws with.
#[derive(Clone)]
pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub caps: Capabilities,
    pub info: Arc<wgpu::AdapterInfo>,
}

/// Why there is no GPU.
#[derive(Debug)]
pub struct GpuError(pub String);

impl std::fmt::Display for GpuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for GpuError {}

pub fn instance(choice: BackendChoice) -> wgpu::Instance {
    let backends = choice.backends();
    let mut desc = wgpu::InstanceDescriptor {
        backends,
        flags: if cfg!(debug_assertions) {
            wgpu::InstanceFlags::debugging()
        } else {
            wgpu::InstanceFlags::empty()
        },
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    };
    if backends.contains(wgpu::Backends::DX12) {
        let (compiler, why) = dx12_compiler();
        log::info!("Direct3D 12 shader compiler: {why}");
        desc.backend_options.dx12.shader_compiler = compiler;
        // Direct3D's SV_InstanceID ignores an indirect draw's first instance; wgpu feeds it to the
        // shader through root constants only when it rewrites indirect arguments, which is part of
        // indirect validation. The renderer's multi-draws rely on `first_instance`
        // (docs/bench/dx12.md): without this every batch but the first draws the wrong instances.
        desc.flags |= wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL;
    }
    // A measurement switch (docs/bench/dx12.md): `POCKET_INDIRECT_VALIDATION=0` or `1` forces
    // wgpu's indirect-call validation off or on for any backend, to time what it costs.
    match std::env::var("POCKET_INDIRECT_VALIDATION").as_deref() {
        Ok("0") => desc
            .flags
            .remove(wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL),
        Ok("1") => desc.flags |= wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL,
        _ => {}
    }
    wgpu::Instance::new(desc)
}

/// The Direct3D 12 shader compiler: DXC from `POCKET_DXC` or from the newest Windows SDK that has
/// one, FXC otherwise or with `POCKET_DXC=fxc`. DXC is loaded at run time; wgpu's `static-dxc`
/// feature (which downloads binaries at build time) is not used. Returns the choice and, for the
/// log, where it came from.
pub fn dx12_compiler() -> (wgpu::Dx12Compiler, String) {
    let dxc = |path: PathBuf, from: String| {
        let dxc_path = path.display().to_string();
        let why = format!("DXC {dxc_path} ({from})");
        (wgpu::Dx12Compiler::DynamicDxc { dxc_path }, why)
    };
    let fxc = |why: String| (wgpu::Dx12Compiler::Fxc, format!("FXC ({why})"));
    match std::env::var("POCKET_DXC") {
        Ok(v) if v.eq_ignore_ascii_case("fxc") => fxc("POCKET_DXC=fxc".into()),
        Ok(v) if !v.is_empty() => {
            let mut path = PathBuf::from(&v);
            if path.is_dir() {
                path.push("dxcompiler.dll");
            }
            if path.is_file() {
                dxc(path, "POCKET_DXC".into())
            } else {
                fxc(format!("POCKET_DXC={v} has no dxcompiler.dll"))
            }
        }
        _ => match newest_sdk_dxc() {
            Some((sdk, path)) => dxc(path, format!("Windows SDK {sdk}")),
            None => fxc("no Windows SDK dxcompiler.dll found".into()),
        },
    }
}

/// `<Windows Kits>\10\bin\<version>\x64\dxcompiler.dll` of the highest version that has one.
fn newest_sdk_dxc() -> Option<(String, PathBuf)> {
    if !cfg!(all(windows, target_arch = "x86_64")) {
        return None;
    }
    let base = std::env::var_os("ProgramFiles(x86)")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"))
        .join("Windows Kits")
        .join("10")
        .join("bin");
    let mut best: Option<(Vec<u32>, String, PathBuf)> = None;
    for entry in std::fs::read_dir(&base).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(version) = sdk_version(&name) else {
            continue;
        };
        let dll = entry.path().join("x64").join("dxcompiler.dll");
        if dll.is_file() && best.as_ref().is_none_or(|(v, _, _)| version > *v) {
            best = Some((version, name, dll));
        }
    }
    best.map(|(_, name, dll)| (name, dll))
}

/// `10.0.26100.0` as numbers; `None` for anything else (`x64`, `arm64`).
fn sdk_version(name: &str) -> Option<Vec<u32>> {
    let parts: Option<Vec<u32>> = name.split('.').map(|p| p.parse().ok()).collect();
    parts.filter(|p| p.len() == 4)
}

impl Gpu {
    /// A device on the best adapter of `instance`, compatible with `surface` when given.
    pub async fn new(
        instance: wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
    ) -> Result<Gpu, GpuError> {
        let adapter = match adapter_override() {
            Some(wanted) => pick_adapter(&instance, surface, &wanted).await?,
            None => instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    compatible_surface: surface,
                    force_fallback_adapter: false,
                    apply_limit_buckets: false,
                })
                .await
                .map_err(|e| GpuError(format!("no GPU adapter: {e}")))?,
        };
        let have = adapter.features();
        let want = wgpu::Features::INDIRECT_FIRST_INSTANCE
            | wgpu::Features::TIMESTAMP_QUERY
            | wgpu::Features::SHADER_F16
            | wgpu::Features::FLOAT32_FILTERABLE;
        let mut required = have & want;
        if !cfg!(target_arch = "wasm32") {
            // Timestamps inside passes and encoders are native-only extras.
            required |= have
                & (wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS
                    | wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES);
        }
        // Ask for what the adapter offers: the GPU-driven buffers are large.
        let mut limits = adapter.limits();
        if let Ok(v) = std::env::var("POCKET_GPU_MINIMAL") {
            if v.contains("features") {
                required = wgpu::Features::empty();
            }
            if v.contains("timestamps") {
                required.remove(
                    wgpu::Features::TIMESTAMP_QUERY
                        | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS
                        | wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES,
                );
            }
            if v.contains("limits") {
                limits = wgpu::Limits::default().using_resolution(adapter.limits());
            }
        }
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("pocket"),
                required_features: required,
                required_limits: limits,
                memory_hints: wgpu::MemoryHints::Performance,
                ..Default::default()
            })
            .await
            .map_err(|e| GpuError(format!("no GPU device: {e}")))?;
        let caps = Capabilities {
            indirect_first_instance: required.contains(wgpu::Features::INDIRECT_FIRST_INSTANCE),
            timestamps: required.contains(wgpu::Features::TIMESTAMP_QUERY),
            shader_f16: required.contains(wgpu::Features::SHADER_F16),
            float32_filterable: required.contains(wgpu::Features::FLOAT32_FILTERABLE),
        };
        // Log every validation error instead of panicking at the first: an engine keeps running and
        // reports (the editor and agents read the log).
        device.on_uncaptured_error(Arc::new(|e: wgpu::Error| {
            let s = format!("{e:?}");
            log::error!("wgpu: {}", &s[..s.len().min(1200)])
        }));
        device.set_device_lost_callback(|reason, message| {
            log::error!("GPU device lost ({reason:?}): {message}");
        });
        let info = Arc::new(adapter.get_info());
        let l = device.limits();
        log::info!(
            "limits: buffer {} MB, storage binding {} MB, storage buffers/stage {}, texture 2D {}",
            l.max_buffer_size >> 20,
            l.max_storage_buffer_binding_size >> 20,
            l.max_storage_buffers_per_shader_stage,
            l.max_texture_dimension_2d
        );
        log::info!(
            "GPU: {} ({:?}, {:?}); driver {} {}; features {:?}",
            info.name,
            info.backend,
            info.device_type,
            info.driver,
            info.driver_info,
            caps
        );
        Ok(Gpu {
            instance,
            adapter,
            device,
            queue,
            caps,
            info,
        })
    }

    /// A headless device (offscreen rendering, benchmarks, tests), blocking.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn headless(choice: BackendChoice) -> Result<Gpu, GpuError> {
        pollster::block_on(Gpu::new(instance(choice), None))
    }

    pub fn backend_name(&self) -> &'static str {
        match self.info.backend {
            wgpu::Backend::Metal => "Metal",
            wgpu::Backend::Vulkan => "Vulkan",
            wgpu::Backend::BrowserWebGpu => "WebGPU",
            wgpu::Backend::Dx12 => "Direct3D 12",
            wgpu::Backend::Gl => "OpenGL",
            _ => "other",
        }
    }
}

/// `POCKET_ADAPTER`: which adapter to use instead of the high-performance one.
fn adapter_override() -> Option<String> {
    if cfg!(target_arch = "wasm32") {
        return None;
    }
    std::env::var("POCKET_ADAPTER")
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

/// The adapter `wanted` names among those that can present to `surface`: an index into that list or
/// a case-insensitive part of an adapter's name. The error lists them all when none matches.
async fn pick_adapter(
    instance: &wgpu::Instance,
    surface: Option<&wgpu::Surface<'_>>,
    wanted: &str,
) -> Result<wgpu::Adapter, GpuError> {
    let adapters: Vec<wgpu::Adapter> = instance
        .enumerate_adapters(wgpu::Backends::all())
        .await
        .into_iter()
        .filter(|a| surface.is_none_or(|s| a.is_surface_supported(s)))
        .collect();
    let names: Vec<String> = adapters
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let info = a.get_info();
            format!("{i}: {} ({:?})", info.name, info.backend)
        })
        .collect();
    log::info!("adapters: {}", names.join(", "));
    let found = match wanted.parse::<usize>() {
        Ok(i) => (i < adapters.len()).then_some(i),
        Err(_) => {
            let needle = wanted.to_lowercase();
            adapters
                .iter()
                .position(|a| a.get_info().name.to_lowercase().contains(&needle))
        }
    };
    found
        .and_then(|i| adapters.into_iter().nth(i))
        .ok_or_else(|| {
            GpuError(format!(
                "POCKET_ADAPTER={wanted} matches no adapter; have [{}]",
                names.join(", ")
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_names_parse_in_any_case() {
        assert_eq!(BackendChoice::parse("DX12"), Some(BackendChoice::Dx12));
        assert_eq!(BackendChoice::parse("d3d12"), Some(BackendChoice::Dx12));
        assert_eq!(
            BackendChoice::parse(" Vulkan "),
            Some(BackendChoice::Vulkan)
        );
        assert_eq!(BackendChoice::parse("vk"), Some(BackendChoice::Vulkan));
        assert_eq!(BackendChoice::parse("metal"), Some(BackendChoice::Metal));
        assert_eq!(BackendChoice::parse("gl"), None);
    }

    #[test]
    fn sdk_versions_order_numerically() {
        assert_eq!(sdk_version("10.0.26100.0"), Some(vec![10, 0, 26100, 0]));
        assert_eq!(sdk_version("x64"), None);
        assert_eq!(sdk_version("10.0.1"), None);
        assert!(sdk_version("10.0.26100.0") > sdk_version("10.0.22621.0"));
        assert!(sdk_version("10.0.9200.0") < sdk_version("10.0.10240.0"));
    }
}
