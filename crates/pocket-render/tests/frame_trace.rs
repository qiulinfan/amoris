//! A per-frame trace (trace.rs) of a few headless frames: the frames after the skipped ones are
//! written as Chrome Trace Event JSON when the host finishes the trace after the last one (or by
//! the last `render` when the host does not bracket frames); CPU spans nest in their frame; and
//! with GPU timestamps every frame's GPU work, placed on the CPU timeline by the clock calibration,
//! lies between the start of its submit and the end of the wait for the GPU that follows (the
//! loop waits for the GPU every frame, so the work cannot be anywhere else). Skips without a GPU.

use pocket_render::trace::TraceSpec;
use pocket_render::{BackendChoice, Gpu, Renderer, demo};
use serde_json::Value;

#[test]
fn a_headless_trace_places_gpu_work_inside_its_frame() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    let (w, h) = (320, 180);
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut r = Renderer::new(&gpu, format, w, h);
    let out = std::env::temp_dir().join(format!("pocket-frame-trace-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&out);
    r.start_trace(TraceSpec {
        frames: 6,
        skip: 2,
        out: Some(out.clone()),
    });
    let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("trace target"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    r.apply(demo::many_cubes(2_000, true, false), 0.0);
    let mut complete = Vec::new();
    for i in 0..8 {
        let now = f64::from(i) / 60.0;
        r.trace_begin_frame();
        r.render(&view, now);
        let ts = r.trace_mark();
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        r.trace_span("wait idle", ts);
        complete.push(r.trace_end_frame());
    }
    // The sixth recorded frame completed the trace. Writing it stalls, so the host that brackets
    // its frames writes it when that does not count (a benchmark after its measured frames), not
    // the end of the frame.
    assert_eq!(
        complete,
        [false, false, false, false, false, false, false, true]
    );
    assert!(
        r.tracing() && !out.exists(),
        "the trace was written inside the host's last frame"
    );
    assert_eq!(r.finish_trace().as_deref(), Some(out.as_path()));
    assert!(!r.tracing(), "the trace did not stop once written");
    let trace: Value = serde_json::from_slice(&std::fs::read(&out).expect("trace written"))
        .expect("the trace is JSON");
    let _ = std::fs::remove_file(&out);
    // Without the host's brackets each `render` is a frame, and the last one writes the trace.
    let implicit = out.with_extension("implicit.json");
    let _ = std::fs::remove_file(&implicit);
    r.start_trace(TraceSpec {
        frames: 2,
        skip: 1,
        out: Some(implicit.clone()),
    });
    for i in 0..3 {
        r.render(&view, f64::from(i) / 60.0);
    }
    assert!(
        !r.tracing() && implicit.exists(),
        "render did not write its trace"
    );
    let _ = std::fs::remove_file(&implicit);
    let events = trace["traceEvents"].as_array().expect("traceEvents");
    let x = |pid: u64, name: &str| -> Vec<&Value> {
        events
            .iter()
            .filter(|e| e["ph"] == "X" && e["pid"] == pid && e["name"] == name)
            .collect()
    };
    let frames = x(1, "frame");
    assert_eq!(frames.len(), 6);
    let bounds = |e: &Value| {
        let ts = e["ts"].as_f64().unwrap();
        (ts, ts + e["dur"].as_f64().unwrap())
    };
    let of = |list: &[&Value], frame: usize| -> (f64, f64) {
        bounds(
            list.iter()
                .find(|e| e["args"]["frame"] == frame)
                .unwrap_or_else(|| panic!("frame {frame}")),
        )
    };
    let (renders, submits, waits) = (x(1, "render"), x(1, "submit"), x(1, "wait idle"));
    for f in 0..6 {
        let frame = of(&frames, f);
        for (name, span) in [
            ("render", of(&renders, f)),
            ("submit", of(&submits, f)),
            ("wait idle", of(&waits, f)),
        ] {
            assert!(
                span.0 >= frame.0 && span.1 <= frame.1 + 0.002,
                "frame {f}: {name} {span:?} outside {frame:?}"
            );
        }
    }
    if trace["otherData"]["gpu_timestamps"] != true {
        eprintln!("no GPU timestamps: placement not checked");
        return;
    }
    assert_eq!(trace["otherData"]["gpu_frames_missing"], 0);
    let uncertainty = trace["otherData"]["clock"]["uncertainty_us"]
        .as_f64()
        .expect("a calibrated trace");
    // The calibration bounds the offset within `uncertainty`; timestamps are written at pass
    // boundaries, so allow 20 us more.
    let slack = uncertainty + 20.0;
    let gpu_frames = x(2, "GPU frame");
    assert_eq!(gpu_frames.len(), 6);
    for f in 0..6 {
        let (gpu_start, gpu_end) = of(&gpu_frames, f);
        let (submit_start, _) = of(&submits, f);
        let (_, wait_end) = of(&waits, f);
        assert!(
            gpu_start >= submit_start - slack && gpu_end <= wait_end + slack,
            "frame {f}: GPU {gpu_start:.1}..{gpu_end:.1} us outside submit {submit_start:.1} .. \
             wait end {wait_end:.1} (slack {slack:.1})"
        );
    }
}
