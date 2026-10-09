//! Creates the renderer's kinds of resources one by one inside error scopes and prints the first
//! failure of each, then builds the whole renderer (every pipeline, so every shader goes through
//! the backend's compiler) and draws one empty frame: a quick check of a backend
//! (`POCKET_BACKEND=vulkan`, `POCKET_BACKEND=dx12`, `POCKET_ADAPTER=780m`).

fn main() {
    let choice = pocket_render::BackendChoice::from_env();
    if choice.backends().contains(wgpu::Backends::DX12) {
        println!("shader compiler: {}", pocket_render::gpu::dx12_compiler().1);
    }
    let gpu = pocket_render::Gpu::headless(choice).expect("gpu");
    println!(
        "{} on {} ({:?}), driver {} {}",
        gpu.backend_name(),
        gpu.info.name,
        gpu.info.device_type,
        gpu.info.driver,
        gpu.info.driver_info
    );
    println!("capabilities: {:?}", gpu.caps);
    let d = &gpu.device;
    {
        // Shader compilation failures (HLSL through DXC or FXC, MSL, SPIR-V) are internal errors.
        let internal = d.push_error_scope(wgpu::ErrorFilter::Internal);
        let validation = d.push_error_scope(wgpu::ErrorFilter::Validation);
        let t = std::time::Instant::now();
        let mut r =
            pocket_render::Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, 640, 360);
        let built = t.elapsed();
        let (w, h, px) = r.capture_rgba(0.0);
        let errors = [
            pollster::block_on(validation.pop()),
            pollster::block_on(internal.pop()),
        ];
        let errors: Vec<String> = errors
            .into_iter()
            .flatten()
            .map(|e| e.to_string())
            .collect();
        println!(
            "renderer: {} (built in {:.0} ms, frame {w}x{h}, {} bytes)",
            if errors.is_empty() {
                "ok".to_owned()
            } else {
                errors.join("; ")
            },
            built.as_secs_f64() * 1000.0,
            px.len()
        );
    }
    let check = |what: &str, f: &dyn Fn()| {
        let scope = d.push_error_scope(wgpu::ErrorFilter::Validation);
        f();
        let err = pollster::block_on(scope.pop());
        println!("{what}: {}", err.map_or("ok".into(), |e| format!("{e}")));
    };
    let uni = |size: u64, label: &str| {
        let _ = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
    };
    check("uniform 656", &|| uni(656, "view"));
    check("uniform 8192", &|| uni(8192, "sky"));
    check("storage 32 MB", &|| {
        let _ = d.create_buffer(&wgpu::BufferDescriptor {
            label: Some("big"),
            size: 32 << 20,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
    });
    for (name, entry) in [
        ("cull", "main"),
        ("cluster", "assign"),
        ("ibl", "project_sh"),
        ("ibl", "prefilter"),
        ("sky_bake", "bake"),
    ] {
        if std::env::var("PROBE_ONLY").is_ok_and(|o| o != entry) {
            continue;
        }
        check(&format!("compute {name}/{entry}"), &|| {
            let m = d.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(name),
                source: wgpu::ShaderSource::Wgsl(pocket_render::shader_source(name).into()),
            });
            let _ = d.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: &m,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            });
        });
    }
    let lost = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let l2 = lost.clone();
    d.set_device_lost_callback(move |_, m| {
        eprintln!("LOST: {m}");
        l2.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let tex = |label: &str,
               w: u32,
               layers: u32,
               mips: u32,
               samples: u32,
               format: wgpu::TextureFormat,
               usage: wgpu::TextureUsages| {
        let t = d.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: w,
                height: w,
                depth_or_array_layers: layers,
            },
            mip_level_count: mips,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        });
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        println!(
            "texture {label}: lost={}",
            lost.load(std::sync::atomic::Ordering::SeqCst)
        );
        t
    };
    use wgpu::TextureFormat as F;
    use wgpu::TextureUsages as U;
    let _a = tex(
        "msaa hdr",
        1600,
        1,
        1,
        4,
        F::Rgba16Float,
        U::RENDER_ATTACHMENT,
    );
    let _b = tex(
        "msaa depth",
        1600,
        1,
        1,
        4,
        F::Depth32Float,
        U::RENDER_ATTACHMENT,
    );
    let _c = tex(
        "tex array",
        1024,
        2,
        11,
        1,
        F::Rgba8UnormSrgb,
        U::TEXTURE_BINDING | U::RENDER_ATTACHMENT | U::COPY_DST | U::COPY_SRC,
    );
    let _d = tex(
        "shadow",
        2048,
        4,
        1,
        1,
        F::Depth32Float,
        U::RENDER_ATTACHMENT | U::TEXTURE_BINDING,
    );
    let _e = tex(
        "sky cube",
        128,
        6,
        6,
        1,
        F::Rgba16Float,
        U::TEXTURE_BINDING | U::STORAGE_BINDING,
    );
    let _q = d.create_query_set(&wgpu::QuerySetDescriptor {
        label: Some("q"),
        ty: wgpu::QueryType::Timestamp,
        count: 64,
    });
    let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
    println!(
        "query set: lost={}",
        lost.load(std::sync::atomic::Ordering::SeqCst)
    );
    for name in [
        "cull", "cluster", "ibl", "sky_bake", "forward", "sky", "post",
    ] {
        check(&format!("shader {name}"), &|| {
            let src = pocket_render::shader_source(name);
            let _ = d.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(name),
                source: wgpu::ShaderSource::Wgsl(src.into()),
            });
        });
    }
}
