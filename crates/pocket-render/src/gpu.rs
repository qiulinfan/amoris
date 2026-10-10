//! The GPU: instance, adapter, device and queue, with the backend chosen explicitly (charter 4.1 and
//! 4.4: Metal on macOS, Direct3D 12 on Windows, Vulkan elsewhere and on demand on Windows and
//! through MoltenVK, WebGPU in the browser) and the optional features the renderer uses when the
//! adapter has them.
//!
//! Environment: `POCKET_BACKEND` (`metal`, `vulkan`, `dx12`), `POCKET_ADAPTER` (an adapter's index
//! or a case-insensitive part of its name, for machines with two GPUs) and, for Direct3D 12,
//! `POCKET_DXC` (the `dxcompiler.dll` to use, its directory, or `fxc`; see [`dx12_compiler`]).
//! wgpu's `WGPU_*` instance flags (`WGPU_VALIDATION`, `WGPU_VALIDATION_INDIRECT_CALL`) apply on top
//! (see [`instance`]).

mod dxc;

use std::sync::Arc;

pub use dxc::{Dx12CompilerChoice, dx12_compiler};

/// Which backend to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendChoice {
    /// Metal on Apple platforms, Direct3D 12 on Windows (the owner's choice, charter 4.4), Vulkan
    /// elsewhere, WebGPU in the browser.
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
                } else if cfg!(windows) {
                    wgpu::Backends::DX12
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
    /// A nonzero `first_instance` in indirect draws (WebGPU's optional `indirect-first-instance`):
    /// a batch's instance base travels in its draw arguments. Without it a nonzero value makes the
    /// draw a no-op, so the base goes through a dynamic-offset uniform instead (batches.rs).
    pub indirect_first_instance: bool,
    /// `multi_draw_indexed_indirect` runs as native commands (wgpu-core on Vulkan, Metal, DX12).
    /// wgpu 30 offers the call wherever indirect execution exists, but in the browser it is a loop
    /// of single draws (WebGPU has no multi-draw), so there the renderer skips empty batches itself.
    pub multi_draw_indirect: bool,
    /// GPU timestamps around passes (the profiler).
    pub timestamps: bool,
    /// Half-precision arithmetic in shaders (neural decoding).
    pub shader_f16: bool,
    pub float32_filterable: bool,
    /// Inline ray queries (`EXPERIMENTAL_RAY_QUERY`), requested only for ray-traced sun shadows
    /// (`POCKET_RT_SHADOWS=1`, rt_shadows.rs) on a native adapter that has them.
    pub ray_query: bool,
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

/// What to leave out of the device, to run natively as a browser would (`POCKET_GPU_MINIMAL`, or
/// the viewport page's `?gpu_minimal=`): a comma-separated list of `features` (no optional
/// features at all), `first-instance` (no `indirect-first-instance` only), `multi-draw` (draw
/// batch by batch as in the browser even where multi-draw is native), `timestamps` (no GPU
/// timestamps) and `limits` (WebGPU's default limits).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Minimal {
    pub features: bool,
    pub first_instance: bool,
    pub multi_draw: bool,
    pub timestamps: bool,
    pub limits: bool,
}

impl Minimal {
    pub fn parse(s: &str) -> Minimal {
        const TOKENS: [&str; 5] = [
            "features",
            "first-instance",
            "multi-draw",
            "timestamps",
            "limits",
        ];
        for w in s.split(',').map(str::trim).filter(|w| !w.is_empty()) {
            if !TOKENS.contains(&w) {
                log::warn!("POCKET_GPU_MINIMAL: unknown token {w:?} ignored (known: {TOKENS:?})");
            }
        }
        let has = |t: &str| s.split(',').any(|w| w.trim() == t);
        Minimal {
            features: has("features"),
            first_instance: has("first-instance"),
            multi_draw: has("multi-draw"),
            timestamps: has("timestamps"),
            limits: has("limits"),
        }
    }

    /// From `POCKET_GPU_MINIMAL` (nothing left out when it is unset, and in the browser).
    pub fn from_env() -> Minimal {
        Minimal::parse(&std::env::var("POCKET_GPU_MINIMAL").unwrap_or_default())
    }

    /// Whether `indirect-first-instance` is left out.
    pub fn drops_first_instance(self) -> bool {
        self.features || self.first_instance
    }
}

/// An instance on `choice`'s backends, leaving out what `POCKET_GPU_MINIMAL` says.
pub fn instance(choice: BackendChoice) -> wgpu::Instance {
    instance_with(choice, Minimal::from_env())
}

/// An instance on `choice`'s backends, leaving out what `minimal` says (see [`instance_flags_with`]
/// for its flags).
pub fn instance_with(choice: BackendChoice, minimal: Minimal) -> wgpu::Instance {
    let backends = choice.backends();
    let mut desc = wgpu::InstanceDescriptor {
        backends,
        flags: instance_flags_with(backends, minimal),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    };
    if backends.contains(wgpu::Backends::DX12) {
        let dxc = dx12_compiler();
        if dxc.degraded {
            log::warn!("Direct3D 12 shader compiler: {}", dxc.why);
        } else {
            log::info!("Direct3D 12 shader compiler: {}", dxc.why);
        }
        desc.backend_options.dx12.shader_compiler = dxc.compiler;
    }
    wgpu::Instance::new(desc)
}

/// The renderer's instance flags for `backends`, before wgpu's `WGPU_*` environment overrides:
/// wgpu's debugging set in debug builds, nothing in release builds, except that Direct3D 12 always
/// keeps `VALIDATION_INDIRECT_CALL`. Direct3D's `SV_InstanceID` ignores an indirect draw's first
/// instance; wgpu feeds it to the shader through root constants only when it rewrites indirect
/// arguments, which is part of indirect validation. The renderer's multi-draws rely on
/// `first_instance` (docs/bench/dx12.md 2.1): without the flag every batch but the first draws the
/// wrong instances (the test `indirect_draws_keep_their_first_instance` catches it). A release
/// build on Direct3D 12 also keeps labels away from the command lists (`DISCARD_HAL_LABELS`): each
/// pass's label became a `BeginEvent` marker, about 8% of the frame's command recording there and
/// nothing measurable on Vulkan (docs/bench/dx12.md 10); `WGPU_DISCARD_HAL_LABELS=0` brings them
/// back for a PIX capture.
pub fn instance_flags(backends: wgpu::Backends, debug: bool) -> wgpu::InstanceFlags {
    let mut flags = if debug {
        wgpu::InstanceFlags::debugging()
    } else {
        wgpu::InstanceFlags::empty()
    };
    if backends.contains(wgpu::Backends::DX12) {
        flags |= wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL;
        if !debug {
            flags |= wgpu::InstanceFlags::DISCARD_HAL_LABELS;
        }
    }
    flags
}

/// The flags of an instance on `backends` made by [`instance_with`]: [`instance_flags`], then wgpu's
/// own switches, which work in release builds too (`WGPU_VALIDATION=1` with the Direct3D 12 debug
/// layer or Vulkan's validation layers, `WGPU_VALIDATION_INDIRECT_CALL=0|1` to time what indirect
/// validation costs, docs/bench/dx12.md). When `minimal` leaves out `indirect-first-instance`,
/// indirect-call validation stays on whatever the environment says: it turns an indirect draw whose
/// `first_instance` is nonzero into a no-op, as browsers do, where a native driver would draw it
/// anyway and hide a missing baseline path (docs/spec/webgpu-baseline.md).
pub fn instance_flags_with(backends: wgpu::Backends, minimal: Minimal) -> wgpu::InstanceFlags {
    let mut flags = instance_flags(backends, cfg!(debug_assertions)).with_env();
    if minimal.drops_first_instance() {
        flags |= wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL;
    }
    flags
}

impl Gpu {
    /// A device on the best adapter of `instance`, compatible with `surface` when given, leaving
    /// out what `POCKET_GPU_MINIMAL` says.
    pub async fn new(
        instance: wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
    ) -> Result<Gpu, GpuError> {
        Gpu::new_with(instance, surface, Minimal::from_env()).await
    }

    /// A device on the best adapter of `instance` (or the one `POCKET_ADAPTER` names), leaving out
    /// what `minimal` says.
    pub async fn new_with(
        instance: wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
        minimal: Minimal,
    ) -> Result<Gpu, GpuError> {
        Gpu::create(instance, surface, minimal, crate::rt_shadows::requested()).await
    }

    /// [`Gpu::new_with`], asking for ray queries when `wants_ray_query` (instead of when
    /// `POCKET_RT_SHADOWS=1` does).
    async fn create(
        instance: wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
        minimal: Minimal,
        wants_ray_query: bool,
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
        if minimal.features {
            required = wgpu::Features::empty();
        }
        if minimal.first_instance {
            required.remove(wgpu::Features::INDIRECT_FIRST_INSTANCE);
        }
        if minimal.timestamps {
            required.remove(
                wgpu::Features::TIMESTAMP_QUERY
                    | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS
                    | wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES,
            );
        }
        if minimal.limits {
            limits = wgpu::Limits::default().using_resolution(adapter.limits());
        }
        // Ray-traced sun shadows are opt-in: only then does the device carry experimental ray
        // queries (charter 4.4, Pioneer 2026-10-09).
        // The ray-traced forward pass binds 3 storage buffers beyond the cascaded one's 8, so a
        // device held to WebGPU's default limits (8 per stage) keeps the cascades.
        let rt_bindings = limits.max_storage_buffers_per_shader_stage >= 11;
        let ray_query = wants_ray_query
            && !cfg!(target_arch = "wasm32")
            && !minimal.features
            && rt_bindings
            && have.contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY);
        if wants_ray_query && !ray_query {
            log::warn!(
                "ray-traced shadows asked for, but this device has no ray queries{}: cascades",
                if rt_bindings {
                    ""
                } else {
                    " within its storage-buffer limit"
                }
            );
        }
        let mut experimental_features = wgpu::ExperimentalFeatures::disabled();
        if ray_query {
            required |= wgpu::Features::EXPERIMENTAL_RAY_QUERY;
            limits = limits.using_acceleration_structure_values(adapter.limits());
            // SAFETY: requested only when POCKET_RT_SHADOWS=1 opts into wgpu's experimental ray
            // queries; validation stays on and errors are logged like any other.
            experimental_features = unsafe { wgpu::ExperimentalFeatures::enabled() };
        }
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("pocket"),
                required_features: required,
                required_limits: limits,
                memory_hints: wgpu::MemoryHints::Performance,
                experimental_features,
                ..Default::default()
            })
            .await
            .map_err(|e| GpuError(format!("no GPU device: {e}")))?;
        let info = Arc::new(adapter.get_info());
        // Direct3D 12 sees an indirect draw's first instance only through wgpu's indirect
        // validation (instance_flags). Where the environment turned it off, the batches take the
        // baseline path, whose draws all start at instance 0, instead of drawing wrong instances.
        let dx12_first_instance = info.backend != wgpu::Backend::Dx12
            || instance_flags_with(wgpu::Backends::DX12, minimal)
                .contains(wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL);
        if !dx12_first_instance {
            log::warn!(
                "Direct3D 12 without indirect validation: indirect draws lose their first                  instance, so batches take the baseline path"
            );
        }
        let caps = Capabilities {
            indirect_first_instance: required.contains(wgpu::Features::INDIRECT_FIRST_INSTANCE)
                && dx12_first_instance,
            multi_draw_indirect: info.backend != wgpu::Backend::BrowserWebGpu
                && !minimal.multi_draw
                && adapter
                    .get_downlevel_capabilities()
                    .flags
                    .contains(wgpu::DownlevelFlags::INDIRECT_EXECUTION),
            timestamps: required.contains(wgpu::Features::TIMESTAMP_QUERY),
            shader_f16: required.contains(wgpu::Features::SHADER_F16),
            float32_filterable: required.contains(wgpu::Features::FLOAT32_FILTERABLE),
            ray_query,
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
        Gpu::headless_with(choice, Minimal::from_env())
    }

    /// A headless device leaving out what `minimal` says (a test comparing both draw paths in one
    /// process).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn headless_with(choice: BackendChoice, minimal: Minimal) -> Result<Gpu, GpuError> {
        pollster::block_on(Gpu::new_with(instance_with(choice, minimal), None, minimal))
    }

    /// A headless device with ray queries when `ray_query` and the adapter has them (what
    /// `POCKET_RT_SHADOWS=1` asks), or without: both shadow paths in one process (tests,
    /// the `rt_shadows` example). `caps.ray_query` tells which it got.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn headless_ray_query(choice: BackendChoice, ray_query: bool) -> Result<Gpu, GpuError> {
        let minimal = Minimal::from_env();
        pollster::block_on(Gpu::create(
            instance_with(choice, minimal),
            None,
            minimal,
            ray_query,
        ))
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

/// The adapter `wanted` names among those that can present to `surface` (see [`match_adapter`]).
/// The error lists them all when none matches.
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
    let infos: Vec<wgpu::AdapterInfo> = adapters.iter().map(wgpu::Adapter::get_info).collect();
    let listed: Vec<String> = infos
        .iter()
        .enumerate()
        .map(|(i, info)| format!("{i}: {} ({:?})", info.name, info.backend))
        .collect();
    log::info!("adapters: {}", listed.join(", "));
    let names: Vec<&str> = infos.iter().map(|i| i.name.as_str()).collect();
    match_adapter(&names, wanted)
        .and_then(|i| adapters.into_iter().nth(i))
        .ok_or_else(|| {
            GpuError(format!(
                "POCKET_ADAPTER={wanted} matches no adapter; have [{}]",
                listed.join(", ")
            ))
        })
}

/// Which of the adapters `names` `wanted` means: an index into the list, or else the first name
/// that contains it, ignoring case (so `5060` finds an RTX 5060 although it is no valid index).
pub fn match_adapter(names: &[&str], wanted: &str) -> Option<usize> {
    let wanted = wanted.trim();
    if let Ok(i) = wanted.parse::<usize>()
        && i < names.len()
    {
        return Some(i);
    }
    let needle = wanted.to_lowercase();
    names
        .iter()
        .position(|n| n.to_lowercase().contains(&needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Auto` is Metal on Apple platforms, Direct3D 12 on Windows (the owner's choice, charter
    /// 4.4) and Vulkan elsewhere.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn auto_picks_the_platform_default() {
        let want = if cfg!(any(target_os = "macos", target_os = "ios")) {
            wgpu::Backends::METAL
        } else if cfg!(windows) {
            wgpu::Backends::DX12
        } else {
            wgpu::Backends::VULKAN
        };
        assert_eq!(BackendChoice::Auto.backends(), want);
        assert_eq!(BackendChoice::Vulkan.backends(), wgpu::Backends::VULKAN);
    }

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
    fn direct3d_12_keeps_indirect_validation() {
        use wgpu::{Backends, InstanceFlags};
        let indirect = InstanceFlags::VALIDATION_INDIRECT_CALL;
        // Release builds also discard the command lists' labels there.
        assert_eq!(
            instance_flags(Backends::DX12, false),
            indirect | InstanceFlags::DISCARD_HAL_LABELS
        );
        assert_eq!(
            instance_flags(Backends::VULKAN, false),
            InstanceFlags::empty()
        );
        assert_eq!(
            instance_flags(Backends::METAL, false),
            InstanceFlags::empty()
        );
        assert!(instance_flags(Backends::VULKAN | Backends::DX12, false).contains(indirect));
        assert_eq!(
            instance_flags(Backends::DX12, true),
            InstanceFlags::debugging() | indirect
        );
        assert_eq!(
            instance_flags(Backends::VULKAN, true),
            InstanceFlags::debugging()
        );
    }

    #[test]
    fn adapters_match_by_index_or_name() {
        let names = [
            "NVIDIA GeForce RTX 5060 Laptop GPU",
            "AMD Radeon 780M Graphics",
            "Microsoft Basic Render Driver",
        ];
        assert_eq!(match_adapter(&names, "0"), Some(0));
        assert_eq!(match_adapter(&names, " 2 "), Some(2));
        assert_eq!(match_adapter(&names, "nvidia"), Some(0));
        assert_eq!(match_adapter(&names, "780M"), Some(1));
        assert_eq!(match_adapter(&names, "radeon 780m"), Some(1));
        // A number that is no index is part of a name.
        assert_eq!(match_adapter(&names, "5060"), Some(0));
        assert_eq!(match_adapter(&names, "780"), Some(1));
        // The first name that matches wins.
        assert_eq!(match_adapter(&names, "r"), Some(0));
        assert_eq!(match_adapter(&names, "intel"), None);
        assert_eq!(match_adapter(&names, "3"), None);
        assert_eq!(match_adapter(&[], "0"), None);
    }

    /// The native backends of this platform. A GPU test that guards a difference between backends
    /// runs on each (Vulkan and Direct3D 12 on Windows), skipping any without an adapter.
    fn native_backends() -> &'static [BackendChoice] {
        if cfg!(windows) {
            &[BackendChoice::Vulkan, BackendChoice::Dx12]
        } else if cfg!(any(target_os = "macos", target_os = "ios")) {
            &[BackendChoice::Metal]
        } else {
            &[BackendChoice::Vulkan]
        }
    }

    /// Indirect draws at a non-zero first instance, as the renderer's multi-draws make them (each
    /// batch at `view * stride + offset`), reach `instance_index` with the first instance included,
    /// on every native backend. Direct3D 12 does so only through wgpu's indirect validation
    /// ([`instance_flags`]): with `WGPU_VALIDATION_INDIRECT_CALL=0` this fails there. Skipped
    /// without a GPU or without `INDIRECT_FIRST_INSTANCE`.
    #[test]
    fn indirect_draws_keep_their_first_instance() {
        // Draw A: one instance at 0, column 0; draw B: two instances at 3, columns 3 and 4.
        let expected = [1, 0, 0, 4, 5, 0, 0, 0];
        for &choice in native_backends() {
            let Ok(gpu) = Gpu::headless(choice) else {
                eprintln!("{choice:?}: no GPU, skipped");
                continue;
            };
            // The device's feature, not `caps`: on Direct3D 12 without indirect validation `caps`
            // sends the renderer down the baseline path, and this test should then fail.
            if !gpu
                .device
                .features()
                .contains(wgpu::Features::INDIRECT_FIRST_INSTANCE)
            {
                eprintln!(
                    "{}: no INDIRECT_FIRST_INSTANCE, skipped",
                    gpu.backend_name()
                );
                continue;
            }
            let rows = draw_instance_columns(&gpu);
            for (row, how) in rows
                .iter()
                .zip(["multi-draw indirect", "indirect", "direct"])
            {
                assert_eq!(
                    row,
                    &expected,
                    "{} on {}, {how} draws",
                    gpu.backend_name(),
                    gpu.info.name
                );
            }
        }
    }

    /// Leaving out `indirect-first-instance` keeps indirect validation on for every backend, so the
    /// native browser emulation drops nonzero first instances as browsers do; Direct3D 12 keeps it
    /// regardless (when no `WGPU_*` override is set, as in the test environment).
    #[test]
    fn browser_emulation_and_d3d12_keep_indirect_validation() {
        let v = wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL;
        let drop_fi = Minimal::parse("first-instance");
        assert!(instance_flags_with(wgpu::Backends::VULKAN, drop_fi).contains(v));
        assert!(instance_flags_with(wgpu::Backends::METAL, Minimal::parse("features")).contains(v));
        if std::env::var_os("WGPU_VALIDATION_INDIRECT_CALL").is_none() {
            assert!(instance_flags_with(wgpu::Backends::DX12, Minimal::default()).contains(v));
        }
    }

    /// Draws A and B (first_instance_check.wgsl) into an 8x3 `R32Uint` target, one row each way:
    /// one indirect multi-draw, two indirect draws, and two direct draws (the control).
    fn draw_instance_columns(gpu: &Gpu) -> Vec<[u32; 8]> {
        use wgpu::util::DeviceExt;
        const W: u32 = 8;
        const H: u32 = 3;
        let device = &gpu.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("first instance check"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../shaders/first_instance_check.wgsl").into(),
            ),
        });
        let format = wgpu::TextureFormat::R32Uint;
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("first instance check"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let buffer = |label: &str, contents: &[u32], usage: wgpu::BufferUsages| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::cast_slice(contents),
                usage,
            })
        };
        let indices = buffer("quad", &[0, 1, 2, 2, 1, 3], wgpu::BufferUsages::INDEX);
        // Index count, instance count, first index, base vertex, first instance.
        let draws = buffer(
            "draws",
            &[6, 1, 0, 0, 0, 6, 2, 0, 0, 3],
            wgpu::BufferUsages::INDIRECT,
        );
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("columns"),
            size: wgpu::Extent3d {
                width: W,
                height: H,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("columns"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&pipeline);
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
            let row = |pass: &mut wgpu::RenderPass<'_>, y: u32| {
                pass.set_viewport(0.0, y as f32, W as f32, 1.0, 0.0, 1.0);
            };
            row(&mut pass, 0);
            pass.multi_draw_indexed_indirect(&draws, 0, 2);
            row(&mut pass, 1);
            pass.draw_indexed_indirect(&draws, 0);
            pass.draw_indexed_indirect(&draws, 20);
            row(&mut pass, 2);
            pass.draw_indexed(0..6, 0, 0..1);
            pass.draw_indexed(0..6, 0, 3..5);
        }
        let stride = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("columns readback"),
            size: u64::from(stride * H),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(H),
                },
            },
            target.size(),
        );
        gpu.queue.submit([enc.finish()]);
        readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let bytes = readback.slice(..).get_mapped_range().expect("mapped");
        (0..H as usize)
            .map(|y| {
                let start = y * stride as usize;
                let texels: &[u32] = bytemuck::cast_slice(&bytes[start..start + W as usize * 4]);
                texels.try_into().expect("8 columns")
            })
            .collect()
    }
}
