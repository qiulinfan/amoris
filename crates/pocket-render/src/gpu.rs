//! The GPU: instance, adapter, device and queue, with the backend chosen explicitly (charter 4.1:
//! Metal on macOS, Vulkan elsewhere and on demand through MoltenVK, WebGPU in the browser) and the
//! optional features the renderer uses when the adapter has them.

use std::sync::Arc;

/// Which backend to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendChoice {
    /// Metal on Apple platforms, Vulkan elsewhere, WebGPU in the browser.
    Auto,
    Metal,
    Vulkan,
}

impl BackendChoice {
    /// From `POCKET_BACKEND` (`metal`, `vulkan`); `Auto` otherwise.
    pub fn from_env() -> BackendChoice {
        match std::env::var("POCKET_BACKEND").as_deref() {
            Ok("metal") => BackendChoice::Metal,
            Ok("vulkan") | Ok("vk") => BackendChoice::Vulkan,
            _ => BackendChoice::Auto,
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
    wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: choice.backends(),
        flags: if cfg!(debug_assertions) {
            wgpu::InstanceFlags::debugging()
        } else {
            wgpu::InstanceFlags::empty()
        },
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    })
}

impl Gpu {
    /// A device on the best adapter of `instance`, compatible with `surface` when given.
    pub async fn new(
        instance: wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
    ) -> Result<Gpu, GpuError> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: surface,
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|e| GpuError(format!("no GPU adapter: {e}")))?;
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
        device.on_uncaptured_error(Arc::new(|e: wgpu::Error| { let s = format!("{e:?}"); log::error!("wgpu: {}", &s[..s.len().min(1200)]) }));
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
            "GPU: {} ({:?}, {:?}); features {:?}",
            info.name,
            info.backend,
            info.device_type,
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
