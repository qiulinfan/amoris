//! The host's presentation services (host-protocol.md 5 and 7): the render feed on `/render` for
//! editor viewports, sent as the game thread publishes, and `capture` for agents, which renders a
//! camera view offscreen on the native GPU and saves it as a PNG under the project's
//! `.amoris/captures/`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use amoris_assets::{Feed, Mailbox};
use amoris_contract::{Problem, detail};
use amoris_render::loader::FileAssets;
use amoris_render::{CameraState, Gpu, Renderer};
use amoris_server::{BoxFuture, CaptureHub, RenderFeed};
use axum::extract::ws::{Message, WebSocket};
use glam::{Quat, Vec3};
use serde_json::{Value, json};

/// The least time between two frames on one `/render` socket: publications closer together merge
/// into one frame (a game stepped faster than real time publishes thousands a second).
pub const FEED_GAP: Duration = Duration::from_millis(8);

/// The game thread's wake-up for the render feed: `ThreadOptions::on_publish` counts publications
/// here, and each `/render` socket waits for the count to change.
pub fn feed_wake() -> (
    Arc<dyn Fn() + Send + Sync>,
    tokio::sync::watch::Receiver<u64>,
) {
    let (tx, rx) = tokio::sync::watch::channel(0u64);
    let wake = move || tx.send_modify(|n| *n = n.wrapping_add(1));
    (Arc::new(wake), rx)
}

/// Serves `/render`: each socket subscribes to the feed and receives the merged frame (a reset
/// first) when the game thread publishes, at most one every [`FEED_GAP`].
pub struct FeedServer {
    pub feed: Feed,
    /// Publications so far ([`feed_wake`]).
    pub wake: tokio::sync::watch::Receiver<u64>,
}

impl RenderFeed for FeedServer {
    fn serve(&self, mut socket: WebSocket) -> BoxFuture<()> {
        let mailbox = self.feed.subscribe();
        // Cloned after subscribing and never marked seen first: a publication made since this
        // receiver's last look (the reset frame for this subscriber among them) wakes it at once.
        let mut wake = self.wake.clone();
        Box::pin(async move {
            let mut last: Option<Instant> = None;
            // The game thread is gone: nothing more will come, but the socket stays until closed.
            let mut ended = false;
            loop {
                tokio::select! {
                    changed = wake.changed(), if !ended => {
                        if changed.is_err() {
                            ended = true;
                            continue;
                        }
                        if let Some(due) = last.map(|t| t + FEED_GAP)
                            && Instant::now() < due
                        {
                            tokio::time::sleep_until(due.into()).await;
                        }
                        let frame = mailbox.take();
                        if frame.is_empty() && !frame.reset && frame.tick == 0 {
                            continue;
                        }
                        if socket.send(Message::Binary(frame.encode().into())).await.is_err() {
                            break;
                        }
                        last = Some(Instant::now());
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
            .name("amoris-capture".into())
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
        let w = job
            .params
            .get("width")
            .and_then(Value::as_u64)
            .unwrap_or(1280)
            .clamp(16, 4096) as u32;
        let h = job
            .params
            .get("height")
            .and_then(Value::as_u64)
            .unwrap_or(720)
            .clamp(16, 4096) as u32;
        if renderer.is_none() {
            match Gpu::headless(amoris_render::host_backend()) {
                Ok(gpu) => {
                    let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, w, h);
                    r.set_asset_source(Box::new(FileAssets::new(root.clone())));
                    renderer = Some(r);
                }
                Err(e) => {
                    let _ = job.reply.send(Err(Problem::new(
                        "capture.no_gpu",
                        e.to_string(),
                        detail([]),
                    )));
                    continue;
                }
            }
        }
        let Some(r) = renderer.as_mut() else { continue };
        r.resize(w, h);
        r.apply(mailbox.take(), now);
        r.set_camera_override(camera_from(&job.params));
        // `frames`: first draw this many frames at 60 a second, each from the newest published
        // world, so what presentation gathers over time (spray, foam) is there in a running game.
        let lead = job
            .params
            .get("frames")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .min(1200);
        for k in 0..lead {
            let due = now + (k + 1) as f64 / 60.0;
            let wait = due - start.elapsed().as_secs_f64();
            if wait > 0.0 {
                std::thread::sleep(Duration::from_secs_f64(wait));
            }
            let t = start.elapsed().as_secs_f64();
            r.apply(mailbox.take(), t);
            let _ = r.capture_rgba(t);
        }
        let now = if lead > 0 {
            start.elapsed().as_secs_f64()
        } else {
            now
        };
        // A few frames so assets requested by this frame's scene can arrive (bounded wait).
        let mut frames = 0;
        let mut t = now;
        let (mut cw, mut ch, mut px) = r.capture_rgba(t);
        while r.last.pending_assets > 0 && frames < 120 {
            std::thread::sleep(Duration::from_millis(25));
            t = start.elapsed().as_secs_f64();
            (cw, ch, px) = r.capture_rgba(t);
            frames += 1;
        }
        // With TAA (an integrated GPU's default) one frame is a single jittered sample: draw a
        // whole jitter cycle from a reset instead.
        if r.antialiasing().taa() {
            (cw, ch, px) = r.capture_still(t);
        }
        n += 1;
        let dir = root.join(".amoris").join("captures");
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
                    // Each water region's spray pool after the shot: births and drops alive.
                    "spray": r.water_spray_stats().ok(),
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
                return Err(Problem::new(
                    "capture.unavailable",
                    "The capture thread stopped.",
                    detail([]),
                ));
            }
            rx.await.unwrap_or_else(|_| {
                Err(Problem::new(
                    "capture.unavailable",
                    "The capture thread stopped.",
                    detail([]),
                ))
            })
        })
    }
}

/// The script debugger behind the host's `debug.*` methods and the editor's debugging panel
/// (docs/spec/debugger.md): agents' calls go to the hub on a blocking thread (a step or a wait
/// blocks up to its `timeout_ms`); the host serves `debug.rewind` and resolves `debug.watch`'s
/// entity names itself (amoris-server `local.rs`).
pub struct DebugBridge {
    pub hub: amoris_debug::DebugHub,
}

/// The debugger's catalog entries (amoris-debug's methods), for the host's catalog and the CLI's
/// help without a host.
pub fn debug_methods() -> Vec<Value> {
    amoris_debug::methods()
        .iter()
        .map(amoris_debug::Method::entry)
        .collect()
}

impl amoris_server::DebugHub for DebugBridge {
    fn call(&self, method: &str, params: Value) -> BoxFuture<Result<Value, Problem>> {
        let hub = self.hub.clone();
        let method = method.to_owned();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || hub.call(&method, &params))
                .await
                .unwrap_or_else(|e| Err(Problem::new("debug.failed", e.to_string(), detail([]))))
        })
    }

    fn methods(&self) -> Vec<Value> {
        debug_methods()
    }

    fn pass(&self, on: bool) {
        self.hub.pass(on);
    }
}

/// Forwards the debugger's pauses, resumes, console lines and exceptions to editors (`debug` and
/// `log` topics).
pub fn forward_debug_events(hub: &amoris_debug::DebugHub, host: amoris_server::Host) {
    let rx = hub.subscribe();
    std::thread::Builder::new()
        .name("amoris-debug-events".into())
        .spawn(move || {
            for ev in rx {
                if let Some(j) = ev.json() {
                    let topic = if j["event"] == "debug" {
                        "debug"
                    } else {
                        "log"
                    };
                    host.push(topic, j["data"].clone());
                }
            }
        })
        .ok();
}
