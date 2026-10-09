//! The `wasm-bindgen` exports (docs/spec/architecture.md 4.14): [`GameWorker`] for the game worker
//! (`web/game.js`) and [`Presenter`] for the page (`web/pocket.js`), and the injected clock,
//! `performance.now()`, which exists in a window and in a worker alike (threads.md 3.1). Values
//! cross as JSON text and byte arrays, so the glue needs no generated types beyond these classes.

use pocket_contract::Problem;
use pocket_runtime::{Game, GameBuilder};
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;

use crate::Pacing;
use crate::package::Package;
use crate::presenter::PresenterCore;
use crate::worker::{Out, SLICE_MS, WorkerCore};

#[wasm_bindgen]
extern "C" {
    /// The page's or the worker's `performance.now()`.
    #[wasm_bindgen(js_namespace = performance, js_name = now)]
    fn performance_now() -> f64;
}

fn js_problem(p: &Problem) -> JsValue {
    JsValue::from_str(&serde_json::to_string(p).unwrap_or_default())
}

/// `init`'s options (threads.md 7.2), all optional: `seed` (the package's otherwise) and `pacing`
/// (`"stepped"`, the default, or `{"real_time": {"speed": s}}`).
fn options(text: &str) -> Result<(Option<u64>, Pacing), Problem> {
    let v: Value = if text.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(text).map_err(|e| {
            Problem::new(
                "request.malformed",
                format!("init's options are not JSON: {e}."),
                pocket_contract::detail([]),
            )
        })?
    };
    let seed = v.get("seed").and_then(Value::as_u64);
    let pacing = match v.get("pacing") {
        None | Some(Value::Null) => Pacing::Stepped,
        Some(p) => pocket_runtime::decode::<Pacing>(p, "init's pacing")?,
    };
    Ok((seed, pacing))
}

/// The game role in a Web Worker.
#[wasm_bindgen]
pub struct GameWorker {
    core: WorkerCore,
    /// The snapshots' bytes of the `snap`s [`GameWorker::messages`] returned, in order.
    bytes: std::collections::VecDeque<Vec<u8>>,
    /// The render feed this worker publishes to its page's viewport.
    render: (
        pocket_runtime::Extractor,
        pocket_assets::Feed,
        std::sync::Arc<pocket_assets::Mailbox>,
    ),
}

#[wasm_bindgen]
impl GameWorker {
    /// Builds the game of `package` (the web package's bytes) with `options` (JSON); the first
    /// messages (`ready` and the first `snap`) wait in [`GameWorker::messages`]. Throws the
    /// problem as JSON text.
    #[wasm_bindgen(constructor)]
    pub fn new(package: &[u8], options_json: &str) -> Result<GameWorker, JsValue> {
        // The source the native build of this tree reports too (versions.md V9), so the worker
        // reuses the compiled modules a native recording embeds (replay.md 2.4). A second game in
        // the same worker installs the same version again, which is a no-op.
        let _ = pocket_runtime::install_engine_version!();
        let build = || -> Result<WorkerCore, Problem> {
            let (seed, pacing) = options(options_json)?;
            let package = Package::from_bytes(package)?;
            let setup = std::sync::Arc::new(package.setup()?);
            let game: Game = GameBuilder::new(setup)
                .seed(seed.unwrap_or(package.seed))
                .build()?;
            WorkerCore::new(game, pacing, std::sync::Arc::new(performance_now))
        };
        build()
            .map(|core| {
                let feed = pocket_assets::Feed::new();
                let mailbox = feed.subscribe();
                GameWorker {
                    core,
                    bytes: Default::default(),
                    render: (pocket_runtime::Extractor::new(), feed, mailbox),
                }
            })
            .map_err(|p| js_problem(&p))
    }

    /// A `cmd` message as JSON text.
    pub fn push(&mut self, cmd_json: &str) {
        match serde_json::from_str::<Value>(cmd_json) {
            Ok(v) => self.core.push(&v),
            Err(_) => self.core.push(&json!({})),
        }
    }

    /// An `ack` of snapshot `version`.
    pub fn ack(&mut self, version: f64) {
        // Versions are counters from 1, exact in an f64.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        self.core.ack(version as u64);
    }

    /// One task of the loop; the answer as JSON: `{"next": "now" | "command" | "quit"}` or
    /// `{"next": "at", "at": ms}`.
    pub fn run(&mut self) -> String {
        self.core.run(SLICE_MS).to_json().to_string()
    }

    /// The render feed's frame since the last call (the bytes the page's viewport decodes), or
    /// `None` when nothing visual changed.
    pub fn render_frame(&mut self) -> Option<Vec<u8>> {
        let (ex, feed, mailbox) = &mut self.render;
        self.core.present(ex, feed);
        let f = mailbox.take();
        (!f.is_empty() || f.reset).then(|| f.encode())
    }

    /// `close`: what waits is answered `game.stopped`.
    pub fn close(&mut self) {
        self.core.close();
    }

    /// The messages to post, as a JSON array; for each `snap`, [`GameWorker::snap_bytes`] gives
    /// its snapshot's bytes, in the same order.
    pub fn messages(&mut self) -> String {
        let mut list = Vec::new();
        for o in self.core.take_out() {
            match o {
                Out::Message(m) => list.push(m),
                Out::Snap(m, b) => {
                    list.push(m);
                    self.bytes.push_back(b);
                }
            }
        }
        Value::Array(list).to_string()
    }

    /// The next `snap`'s snapshot bytes (a fresh `Uint8Array` the worker transfers).
    pub fn snap_bytes(&mut self) -> Option<Vec<u8>> {
        self.bytes.pop_front()
    }

    /// The check page's `perf`: every kept tick's cost, as JSON
    /// `{ticks, step_ms: [...], publish_ms: [...], posted: [...]}`.
    pub fn perf(&self) -> String {
        let t = self.core.timings();
        json!({
            "ticks": t.len(),
            "step_ms": t.iter().map(|x| x.step_ms).collect::<Vec<_>>(),
            "publish_ms": t.iter().map(|x| x.publish_ms).collect::<Vec<_>>(),
            "posted": t.iter().map(|x| x.posted).collect::<Vec<_>>(),
        })
        .to_string()
    }

    /// The check page's `verify`: a replay recorded natively, replayed here from its embedded
    /// compiled modules (checks.md 7.2, `replay`); `pocket replay --verify --json`'s summary, or
    /// `{"error": problem}`.
    pub fn verify_replay(&self, bytes: &[u8]) -> String {
        match pocket_check::verify_bytes(bytes) {
            Ok(s) => serde_json::to_string(&s).unwrap_or_default(),
            Err(p) => json!({"error": p}).to_string(),
        }
    }
}

/// The page's presenter role.
#[wasm_bindgen]
pub struct Presenter {
    core: PresenterCore,
}

impl Default for Presenter {
    fn default() -> Self {
        Presenter::new()
    }
}

#[wasm_bindgen]
impl Presenter {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Presenter {
        Presenter {
            core: PresenterCore::new(),
        }
    }

    /// Rebuilds a `snap` (its fields as JSON text, without the bytes, and the bytes): the rebuilt
    /// snapshot's `{version, tick, writes, world_hash}`, or `{"error": problem}`.
    pub fn receive(&mut self, meta_json: &str, bytes: &[u8]) -> String {
        let meta: Value = serde_json::from_str(meta_json).unwrap_or(Value::Null);
        match self.core.receive(&meta, bytes) {
            Ok(v) => v.to_string(),
            Err(p) => json!({"error": p}).to_string(),
        }
    }

    /// The hash stream since the last call: `[[tick, hash], ...]`.
    pub fn take_hashes(&mut self) -> String {
        let list: Vec<Value> = self
            .core
            .take_hashes()
            .into_iter()
            .map(|(t, h)| json!([t, h]))
            .collect();
        Value::Array(list).to_string()
    }

    /// What the checks of order, the hash stream and the rebuild found: problems as JSON.
    pub fn problems(&self) -> String {
        serde_json::to_string(self.core.problems()).unwrap_or_default()
    }

    pub fn received(&self) -> f64 {
        // A count of snapshots, far below 2^53.
        #[allow(clippy::cast_precision_loss)]
        let n = self.core.received() as f64;
        n
    }

    /// The named bodies of the latest snapshot, as JSON (or `{"error": problem}`).
    pub fn view(&self) -> String {
        match self.core.view() {
            Ok(v) => v.to_string(),
            Err(p) => json!({"error": p}).to_string(),
        }
    }
}
