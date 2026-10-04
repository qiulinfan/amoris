//! The host's presentation services (host-protocol.md 5 and 7): the render feed on `/render` for
//! editor viewports, and `capture` for agents, which renders a camera view offscreen on the native
//! GPU and saves it as a PNG under the project's `.pocket/captures/`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use glam::{Quat, Vec3};
use pocket_assets::{Feed, Mailbox};
use pocket_contract::{Problem, detail};
use pocket_render::loader::FileAssets;
use pocket_render::{BackendChoice, CameraState, Gpu, Renderer};
use pocket_server::{BoxFuture, CaptureHub, RenderFeed};
use serde_json::{Value, json};

/// Serves `/render`: each socket subscribes to the feed and receives the merged frames at up to
/// 60 per second (a reset first).
pub struct FeedServer {
    pub feed: Feed,
}

impl RenderFeed for FeedServer {
    fn serve(&self, mut socket: WebSocket) -> BoxFuture<()> {
        let mailbox = self.feed.subscribe();
        Box::pin(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(16));
            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        let frame = mailbox.take();
                        if frame.is_empty() && !frame.reset && frame.tick == 0 {
                            continue;
                        }
                        if socket.send(Message::Binary(frame.encode().into())).await.is_err() {
                            break;
                        }
                    }
                    msg = socket.recv() => {
                        match msg {
                            Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                            // Camera and picking requests from the page come here (host-protocol.md 5).
                            Some(Ok(_)) => {}
                        }
                    }
                }
            }
            mailbox.close();
        })
    }
}

struct CaptureJob {
    params: Value,
    reply: tokio::sync::oneshot::Sender<Result<Value, Problem>>,
}

/// `capture`: a thread owning an offscreen renderer subscribed to the feed.
pub struct CaptureServer {
    jobs: mpsc::Sender<CaptureJob>,
}

impl CaptureServer {
    pub fn start(feed: &Feed, root: PathBuf) -> CaptureServer {
        let (jobs, rx) = mpsc::channel::<CaptureJob>();
        let mailbox = feed.subscribe();
        std::thread::Builder::new()
            .name("pocket-capture".into())
            .spawn(move || capture_thread(rx, mailbox, root))
            .ok();
        CaptureServer { jobs }
    }
}

fn camera_from(params: &Value) -> Option<CameraState> {
    let pos = params.get("position")?.as_array()?;
    let p = Vec3::new(
        pos.first()?.as_f64()? as f32,
        pos.get(1)?.as_f64()? as f32,
        pos.get(2)?.as_f64()? as f32,
    );
    let mut cam = if let Some(t) = params.get("look_at").and_then(Value::as_array) {
        CameraState::look_at(
            p,
            Vec3::new(
                t.first()?.as_f64()? as f32,
                t.get(1)?.as_f64()? as f32,
                t.get(2)?.as_f64()? as f32,
            ),
        )
    } else if let Some(q) = params.get("rotation").and_then(Value::as_array) {
        CameraState {
            position: p,
            rotation: Quat::from_xyzw(
                q.first()?.as_f64()? as f32,
                q.get(1)?.as_f64()? as f32,
                q.get(2)?.as_f64()? as f32,
                q.get(3)?.as_f64()? as f32,
            )
            .normalize(),
            ..CameraState::default()
        }
    } else {
        CameraState::look_at(p, Vec3::ZERO)
    };
    if let Some(f) = params.get("fov_deg").and_then(Value::as_f64) {
        cam.fov_y = (f as f32).to_radians();
    }
    Some(cam)
}

fn capture_thread(rx: mpsc::Receiver<CaptureJob>, mailbox: Arc<Mailbox>, root: PathBuf) {
    let mut renderer: Option<Renderer> = None;
    let start = std::time::Instant::now();
    let mut n = 0u32;
    for job in rx {
        let now = start.elapsed().as_secs_f64();
        let w = job.params.get("width").and_then(Value::as_u64).unwrap_or(1280).clamp(16, 4096) as u32;
        let h = job.params.get("height").and_then(Value::as_u64).unwrap_or(720).clamp(16, 4096) as u32;
        if renderer.is_none() {
            match Gpu::headless(BackendChoice::from_env()) {
                Ok(gpu) => {
                    let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, w, h);
                    r.set_asset_source(Box::new(FileAssets::new(root.clone())));
                    renderer = Some(r);
                }
                Err(e) => {
                    let _ = job.reply.send(Err(Problem::new("capture.no_gpu", e.to_string(), detail([]))));
                    continue;
                }
            }
        }
        let Some(r) = renderer.as_mut() else { continue };
        r.resize(w, h);
        r.apply(mailbox.take(), now);
        r.set_camera_override(camera_from(&job.params));
        // A few frames so assets requested by this frame's scene can arrive (bounded wait).
        let mut frames = 0;
        let (mut cw, mut ch, mut px) = r.capture_rgba(now);
        while r.last.pending_assets > 0 && frames < 120 {
            std::thread::sleep(Duration::from_millis(25));
            (cw, ch, px) = r.capture_rgba(start.elapsed().as_secs_f64());
            frames += 1;
        }
        n += 1;
        let dir = root.join(".pocket").join("captures");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("capture-{n:04}.png"));
        let reply = image::save_buffer(&path, &px, cw, ch, image::ColorType::Rgba8)
            .map(|_| {
                let s = &r.last;
                json!({
                    "path": path.display().to_string(),
                    "width": cw, "height": ch, "tick": s.tick,
                    "instances": s.instances, "gpu_ms": s.gpu_ms,
                    "pending_assets": s.pending_assets,
                })
            })
            .map_err(|e| Problem::new("capture.write_failed", e.to_string(), detail([])));
        let _ = job.reply.send(reply);
    }
}

impl CaptureHub for CaptureServer {
    fn capture(&self, params: Value) -> BoxFuture<Result<Value, Problem>> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        let sent = self.jobs.send(CaptureJob { params, reply }).is_ok();
        Box::pin(async move {
            if !sent {
                return Err(Problem::new("capture.unavailable", "The capture thread stopped.", detail([])));
            }
            rx.await.unwrap_or_else(|_| {
                Err(Problem::new("capture.unavailable", "The capture thread stopped.", detail([])))
            })
        })
    }
}

/// The script debugger behind the host's `debug.*` methods and the editor's debugging panel
/// (docs/spec/debugger.md): agents' calls go to the hub; `debug.rewind` restores the kept snapshot
/// at or before a tick and runs the world forward to it.
pub struct DebugBridge {
    pub hub: pocket_debug::DebugHub,
    pub host: pocket_server::Host,
}

impl pocket_server::DebugHub for DebugBridge {
    fn call(&self, method: &str, params: Value) -> BoxFuture<Result<Value, Problem>> {
        let hub = self.hub.clone();
        let host = self.host.clone();
        let method = method.to_owned();
        Box::pin(async move {
            if method == "debug.rewind" {
                let tick = params.get("tick").and_then(Value::as_u64).ok_or_else(|| {
                    Problem::new(
                        "request.invalid_value",
                        "debug.rewind needs {tick}: the tick to stand at.",
                        detail([]),
                    )
                })?;
                let via = pocket_server::Via::Api;
                let restored = host.call(&via, "snapshots.restore", json!({"tick": tick})).await?;
                let at = restored.get("tick").and_then(Value::as_u64).unwrap_or(tick);
                let mut out = json!({"restored": at, "tick": at});
                if tick > at {
                    let stepped = host.call(&via, "time.step", json!({"ticks": tick - at})).await?;
                    out["tick"] = stepped.get("tick").cloned().unwrap_or(json!(tick));
                }
                return Ok(out);
            }
            tokio::task::spawn_blocking(move || hub.call(&method, &params))
                .await
                .unwrap_or_else(|e| Err(Problem::new("debug.failed", e.to_string(), detail([]))))
        })
    }
}

/// Forwards the debugger's pauses, resumes, console lines and exceptions to editors (`debug` and
/// `log` topics).
pub fn forward_debug_events(hub: &pocket_debug::DebugHub, host: pocket_server::Host) {
    let rx = hub.subscribe();
    std::thread::Builder::new()
        .name("pocket-debug-events".into())
        .spawn(move || {
            for ev in rx {
                if let Some(j) = ev.json() {
                    let topic = if j["event"] == "debug" { "debug" } else { "log" };
                    host.push(topic, j["data"].clone());
                }
            }
        })
        .ok();
}
