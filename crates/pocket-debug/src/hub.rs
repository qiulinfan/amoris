//! The debugger's shared state (docs/spec/debugger.md 2): clients, breakpoints, data watches, the
//! scripts, the current pause and the events. Every frontend (CDP sessions, agents through
//! [`DebugHub::call`]) edits it from its own thread; the game thread's hook reads a snapshot of it
//! ([`Settings`]) whenever its generation changes, and is sent [`Command`]s while it is stopped.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::Value as Json;

use crate::game::GameHook;
use crate::model::{Command, DebugEvent, ExceptionMode, Location, Pause, StepKind};
use crate::scripts::{Registry, Script, SourceRoot};

/// A frontend of the hub.
pub type ClientId = u64;

/// The agents' client (the JSON API): always present.
pub const AGENT: ClientId = 0;

/// How long a frontend waits for the stopped game thread to answer.
pub(crate) const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

/// How a hub is set up.
#[derive(Clone, Debug)]
pub struct HubOptions {
    /// Where the source maps send clients for the TypeScript (`pocket:///` beside the scripts by
    /// default; a project directory gives `file://` URLs).
    pub source_root: SourceRoot,
    /// The game's name, for the CDP target's title.
    pub title: String,
}

impl Default for HubOptions {
    fn default() -> HubOptions {
        HubOptions {
            source_root: SourceRoot::Relative,
            title: "Amoris game".into(),
        }
    }
}

/// What a breakpoint applies to.
#[derive(Clone, Debug)]
pub(crate) enum Target {
    /// A script URL (`pocket:///scripts/rules.js`); `line` is the JavaScript's.
    Url(String),
    /// Script URLs matching (js-debug sends these).
    Regex(Regex),
    /// One script by id.
    Script(String),
    /// A module by path (`scripts/rules.ts`); `line` is the TypeScript's (agents).
    Module(String),
}

#[derive(Clone, Debug)]
pub(crate) struct Breakpoint {
    pub id: String,
    pub owner: ClientId,
    pub target: Target,
    /// 0-based, in the target's coordinates.
    pub line: u32,
    pub condition: Option<String>,
    /// A logpoint: this template literal is logged and the game does not stop.
    pub log: Option<String>,
}

/// A data breakpoint (docs/spec/debugger.md 6).
#[derive(Clone, Debug)]
pub(crate) struct Watch {
    pub id: String,
    pub owner: ClientId,
    pub entity: u64,
    pub component: String,
    pub field: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct Client {
    pub kind: &'static str,
    /// Wants the game instrumented (`Debugger.enable`, `debug.attach`).
    pub enabled: bool,
    pub exceptions: ExceptionMode,
    /// Its breakpoints apply (`Debugger.setBreakpointsActive`).
    pub active: bool,
    /// `Debugger.setSkipAllPauses`.
    pub skip: bool,
}

/// Where the game is (the last system call the hook heard of).
#[derive(Clone, Debug, Default)]
pub(crate) struct Running {
    pub tick: u64,
    pub system: Option<String>,
    pub instrumented: bool,
}

pub(crate) struct State {
    pub clients: BTreeMap<ClientId, Client>,
    next_client: u64,
    pub breakpoints: Vec<Breakpoint>,
    next_bp: u64,
    pub watches: Vec<Watch>,
    next_watch: u64,
    pub registry: Registry,
    pub pause: Option<Arc<Pause>>,
    /// Pauses so far: the next pause's serial is this plus one.
    pub pauses: u64,
    pub running: Running,
}

/// A breakpoint resolved in the running program: the generated lines it stops on, and where to
/// show it.
pub(crate) struct Resolved {
    pub script: Arc<Script>,
    pub js_lines: Vec<u32>,
    pub shown: Location,
}

/// A breakpoint on one generated line of one script.
pub(crate) struct LineBreakpoint {
    pub id: String,
    pub script: Arc<Script>,
    pub line: u32,
    pub condition: Option<String>,
    /// A logpoint's template.
    pub log: Option<String>,
}

/// What the game thread's hook works from; rebuilt when the generation changes.
pub(crate) struct Settings {
    pub scripts: Vec<Arc<Script>>,
    pub breakpoints: Vec<LineBreakpoint>,
    pub watches: Vec<Watch>,
    pub exceptions: ExceptionMode,
    pub debugger_statements: bool,
}

pub(crate) struct Inner {
    /// Some frontend wants the game instrumented: the hook acts on trace calls.
    pub attached: AtomicBool,
    /// Bumped on every change the game thread must see.
    pub generation: AtomicU64,
    pub pause_requested: AtomicBool,
    pub paused: AtomicBool,
    /// How many callers want every pause passed over ([`DebugHub::pass`]): while above zero the
    /// game thread does not stop.
    pub passing: AtomicUsize,
    /// Subscribers: without any, console lines are not even located.
    pub listeners: AtomicUsize,
    /// Statements the hook has looked at while attached.
    pub traced: AtomicU64,
    pub state: Mutex<State>,
    /// Notified when the game pauses or resumes.
    pub changed: Condvar,
    pub commands: Mutex<Option<Sender<Command>>>,
    pub subscribers: Mutex<Vec<Sender<DebugEvent>>>,
    /// The `--inspect-brk` gate: true while something waits for a debugger.
    pub gate: Mutex<bool>,
    pub gate_cv: Condvar,
    pub title: String,
    /// The game loop's state, set to `Breakpoint` while paused (threads.md 3.5).
    pub loop_state: Mutex<Option<pocket_link::StateHandle>>,
    /// Where the CDP endpoint listens, while it does (`debug.state`'s `cdp`).
    pub cdp: Mutex<Option<std::net::SocketAddr>>,
}

/// The debugger: one per game. Clone it to share it between the game thread setup, the CDP
/// endpoint and the server that answers agents; it is `Send + Sync`.
///
/// Wiring (docs/spec/debugger.md 8):
///
/// ```ignore
/// let hub = pocket_debug::DebugHub::new(pocket_debug::HubOptions::default());
/// let cdp = hub.serve_cdp(pocket_debug::CdpOptions::default())?;   // 127.0.0.1:9229
/// let h = hub.clone();
/// let game = GameThread::spawn(move || {
///     let mut g = Game::new(setup, seed)?;
///     g.set_script_debugger(Some(h.hook()));                       // on the game thread
///     Ok(g)
/// }, options)?;
/// // agents: hub.call("debug.breakpoints.set", &json!({"file": "scripts/rules.ts", "line": 30}))
/// ```
#[derive(Clone)]
pub struct DebugHub {
    pub(crate) inner: Arc<Inner>,
}

impl DebugHub {
    pub fn new(options: HubOptions) -> DebugHub {
        let mut clients = BTreeMap::new();
        clients.insert(
            AGENT,
            Client {
                kind: "agent",
                enabled: false,
                exceptions: ExceptionMode::None,
                active: true,
                skip: false,
            },
        );
        DebugHub {
            inner: Arc::new(Inner {
                attached: AtomicBool::new(false),
                generation: AtomicU64::new(1),
                pause_requested: AtomicBool::new(false),
                paused: AtomicBool::new(false),
                passing: AtomicUsize::new(0),
                listeners: AtomicUsize::new(0),
                traced: AtomicU64::new(0),
                state: Mutex::new(State {
                    clients,
                    next_client: 1,
                    breakpoints: Vec::new(),
                    next_bp: 0,
                    watches: Vec::new(),
                    next_watch: 0,
                    registry: Registry::new(options.source_root),
                    pause: None,
                    pauses: 0,
                    running: Running::default(),
                }),
                changed: Condvar::new(),
                commands: Mutex::new(None),
                subscribers: Mutex::new(Vec::new()),
                gate: Mutex::new(false),
                gate_cv: Condvar::new(),
                title: options.title,
                loop_state: Mutex::new(None),
                cdp: Mutex::new(None),
            }),
        }
    }

    /// The hook to attach to the game's script host, on the game thread
    /// (`pocket_runtime::Game::set_script_debugger`). A new hook replaces the previous one's
    /// command channel: attach one game at a time.
    pub fn hook(&self) -> Rc<dyn pocket_script::debug::DebugHook> {
        let (tx, rx) = mpsc::channel();
        *lock(&self.inner.commands) = Some(tx);
        Rc::new(GameHook::new(self.inner.clone(), rx))
    }

    /// Gives the hub the game loop's state (`pocket_runtime::thread::GameHandle::loop_state`): a
    /// pause sets it to `Breakpoint` and a resume back to `Ticking`, so presenters reading the
    /// status see why the game thread is still in its tick (threads.md 3.5).
    pub fn set_loop_state(&self, handle: pocket_link::StateHandle) {
        *lock(&self.inner.loop_state) = Some(handle);
    }

    /// Events for one frontend, from now on (the `debug` and `log` topics through
    /// [`DebugEvent::json`]). Dropping the receiver unsubscribes.
    pub fn subscribe(&self) -> Receiver<DebugEvent> {
        let (tx, rx) = mpsc::channel();
        lock(&self.inner.subscribers).push(tx);
        self.inner.listeners.fetch_add(1, Ordering::AcqRel);
        rx
    }

    /// Statements the game thread's hook has looked at while a frontend was attached (each trace
    /// call of an instrumented program): what the debugger's cost scales with.
    pub fn traced_statements(&self) -> u64 {
        self.inner.traced.load(Ordering::Relaxed)
    }

    /// Whether the game thread is stopped.
    pub fn is_paused(&self) -> bool {
        self.inner.paused.load(Ordering::Acquire)
    }

    /// The current pause, if the game is stopped.
    pub fn pause_info(&self) -> Option<Arc<Pause>> {
        self.state().pause.clone()
    }

    /// Blocks until a frontend releases the game (`Runtime.runIfWaitingForDebugger`, or
    /// `debug.continue`) or `timeout` passes; true when released. The `--inspect-brk` model: call
    /// it before the game's first tick.
    pub fn wait_for_debugger(&self, timeout: Option<Duration>) -> bool {
        let deadline = timeout.map(|t| Instant::now() + t);
        let mut waiting = lock(&self.inner.gate);
        *waiting = true;
        while *waiting {
            let left = match deadline {
                Some(d) => match d.checked_duration_since(Instant::now()) {
                    Some(l) => l,
                    None => {
                        *waiting = false;
                        return false;
                    }
                },
                None => Duration::from_secs(3600),
            };
            waiting = self
                .inner
                .gate_cv
                .wait_timeout(waiting, left)
                .map(|(g, _)| g)
                .unwrap_or_else(|e| e.into_inner().0);
        }
        true
    }

    /// Releases whatever waits in [`DebugHub::wait_for_debugger`].
    pub fn release(&self) {
        *lock(&self.inner.gate) = false;
        self.inner.gate_cv.notify_all();
    }

    /// Passes over every pause while on (`pass(true)` and `pass(false)` come in pairs; callers
    /// count): a current pause resumes, and breakpoints, data breakpoints, `debugger;` statements,
    /// exceptions, steps and pause requests do not stop the game thread until every caller has
    /// turned it off. Breakpoints and watches stay set. The host's `play.stop` while the debugger
    /// holds Play (docs/spec/server.md 3.4): Stop drops the held tick's world, so the rest of the
    /// tick runs to the boundary where the Stop lands.
    pub fn pass(&self, on: bool) {
        if on {
            self.inner.passing.fetch_add(1, Ordering::AcqRel);
            self.inner.pause_requested.store(false, Ordering::Release);
            let _ = self.send(Command::Resume);
        } else {
            let _ = self
                .inner
                .passing
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1));
        }
    }

    /// Whether some caller has the debugger pass over every pause ([`DebugHub::pass`]).
    pub fn is_passing(&self) -> bool {
        self.inner.passing.load(Ordering::Acquire) > 0
    }

    /// Detaches every frontend: breakpoints and watches go, a pause resumes, the next tick runs
    /// uninstrumented. For shutting a game down while it may be stopped.
    pub fn shutdown(&self) {
        {
            let mut st = self.state();
            st.breakpoints.clear();
            st.watches.clear();
            for c in st.clients.values_mut() {
                c.enabled = false;
                c.exceptions = ExceptionMode::None;
            }
            self.recompute(&mut st);
        }
        self.inner.pause_requested.store(false, Ordering::Release);
        self.release();
        let _ = self.send(Command::Resume);
    }

    pub(crate) fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.inner.state)
    }

    /// Recomputes whether the game is to be instrumented and tells the game thread.
    pub(crate) fn recompute(&self, st: &mut State) {
        recompute(&self.inner, st);
    }

    pub(crate) fn open_client(&self, kind: &'static str) -> ClientId {
        let mut st = self.state();
        let id = st.next_client;
        st.next_client += 1;
        st.clients.insert(
            id,
            Client {
                kind,
                enabled: false,
                exceptions: ExceptionMode::None,
                active: true,
                skip: false,
            },
        );
        id
    }

    /// Forgets a client: its breakpoints and watches go; when no frontend wants the game any more,
    /// a pause resumes (V8 drops a session's breakpoints with the session).
    pub(crate) fn close_client(&self, id: ClientId) {
        let resume = {
            let mut st = self.state();
            st.clients.remove(&id);
            st.breakpoints.retain(|b| b.owner != id);
            st.watches.retain(|w| w.owner != id);
            self.recompute(&mut st);
            !self.inner.attached.load(Ordering::Acquire)
        };
        if resume {
            self.inner.pause_requested.store(false, Ordering::Release);
            let _ = self.send(Command::Resume);
            self.release();
        }
    }

    pub(crate) fn update_client(&self, id: ClientId, f: impl FnOnce(&mut Client)) {
        let mut st = self.state();
        if let Some(c) = st.clients.get_mut(&id) {
            f(c);
        }
        self.recompute(&mut st);
    }

    /// Adds a breakpoint; returns its id and where it resolves now.
    pub(crate) fn add_breakpoint(
        &self,
        owner: ClientId,
        target: Target,
        line: u32,
        condition: Option<String>,
        log: Option<String>,
    ) -> (String, Vec<Resolved>) {
        let mut st = self.state();
        st.next_bp += 1;
        let id = format!("bp{}", st.next_bp);
        let bp = Breakpoint {
            id: id.clone(),
            owner,
            target,
            line,
            condition,
            log,
        };
        let resolved = resolve(&bp, &st.registry);
        st.breakpoints.push(bp);
        self.recompute(&mut st);
        (id, resolved)
    }

    /// Removes a breakpoint of `owner` (any owner's when `None`); true if there was one.
    pub(crate) fn remove_breakpoint(&self, owner: Option<ClientId>, id: &str) -> bool {
        let mut st = self.state();
        let before = st.breakpoints.len();
        st.breakpoints
            .retain(|b| !(b.id == id && owner.is_none_or(|o| o == b.owner)));
        let removed = st.breakpoints.len() != before;
        self.recompute(&mut st);
        removed
    }

    pub(crate) fn add_watch(
        &self,
        owner: ClientId,
        entity: u64,
        component: &str,
        field: Option<&str>,
    ) -> String {
        let mut st = self.state();
        st.next_watch += 1;
        let id = format!("w{}", st.next_watch);
        st.watches.push(Watch {
            id: id.clone(),
            owner,
            entity,
            component: component.to_owned(),
            field: field.map(str::to_owned),
        });
        self.recompute(&mut st);
        id
    }

    /// Asks the game to stop at its next statement (instrumenting it first if needed).
    pub(crate) fn request_pause(&self) {
        self.inner.pause_requested.store(true, Ordering::Release);
        let mut st = self.state();
        self.recompute(&mut st);
    }

    /// Sends a command to the game thread; only while it is stopped.
    pub(crate) fn send(&self, cmd: Command) -> Result<(), String> {
        if !self.is_paused() {
            return Err("The game is not paused.".into());
        }
        let tx = lock(&self.inner.commands).clone();
        tx.ok_or("No game is attached.")?
            .send(cmd)
            .map_err(|_| "The game thread is gone.".to_owned())
    }

    /// Asks the stopped game thread and waits for its answer.
    pub(crate) fn ask(
        &self,
        make: impl FnOnce(crate::model::Reply) -> Command,
    ) -> Result<Json, String> {
        let (tx, rx) = mpsc::channel();
        self.send(make(tx))?;
        rx.recv_timeout(ANSWER_TIMEOUT)
            .map_err(|_| "The game thread did not answer.".to_owned())?
    }

    /// Resumes, or steps.
    pub(crate) fn resume(&self, step: Option<StepKind>) -> Result<(), String> {
        self.send(match step {
            Some(k) => Command::Step(k),
            None => Command::Resume,
        })
    }

    /// Waits until the game has left the pause numbered `serial` (resumed, or stopped again), at
    /// most `timeout`.
    pub(crate) fn wait_left(&self, serial: u64, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        let mut st = self.state();
        while st.pause.as_ref().is_some_and(|p| p.serial == serial) {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return;
            };
            st = self
                .inner
                .changed
                .wait_timeout(st, left)
                .map(|(g, _)| g)
                .unwrap_or_else(|e| e.into_inner().0);
        }
    }

    /// Waits until the game is stopped in a pause later than `after` (a serial), at most
    /// `timeout`.
    pub(crate) fn wait_pause(&self, after: u64, timeout: Duration) -> Option<Arc<Pause>> {
        let deadline = Instant::now() + timeout;
        let mut st = self.state();
        loop {
            if let Some(p) = &st.pause
                && p.serial > after
            {
                return Some(p.clone());
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            st = self
                .inner
                .changed
                .wait_timeout(st, left)
                .map(|(g, _)| g)
                .unwrap_or_else(|e| e.into_inner().0);
        }
    }
}

/// Locks, ignoring poisoning (a frontend that panicked leaves consistent data behind: every
/// change is made under one lock).
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn emit(inner: &Inner, event: DebugEvent) {
    let mut subs = lock(&inner.subscribers);
    let before = subs.len();
    subs.retain(|s| s.send(event.clone()).is_ok());
    let gone = before - subs.len();
    if gone > 0 {
        inner.listeners.fetch_sub(gone, Ordering::AcqRel);
    }
}

pub(crate) fn recompute(inner: &Inner, st: &mut State) {
    let wanted = st.clients.values().any(|c| c.enabled)
        || !st.breakpoints.is_empty()
        || !st.watches.is_empty()
        || inner.pause_requested.load(Ordering::Acquire);
    inner.attached.store(wanted, Ordering::Release);
    inner.generation.fetch_add(1, Ordering::AcqRel);
}

/// Where a breakpoint applies in the running program.
pub(crate) fn resolve(bp: &Breakpoint, reg: &Registry) -> Vec<Resolved> {
    let js_bp = |s: &Arc<Script>| -> Option<Resolved> {
        let l = s.snap_js(bp.line)?;
        Some(Resolved {
            script: s.clone(),
            js_lines: vec![l],
            shown: Location::new(s.clone(), (l, s.js_first_col(l))),
        })
    };
    match &bp.target {
        Target::Url(u) => reg
            .current()
            .filter(|s| same_url(&s.url, u))
            .filter_map(js_bp)
            .collect(),
        Target::Regex(r) => reg
            .current()
            .filter(|s| r.is_match(&s.url))
            .filter_map(js_bp)
            .collect(),
        Target::Script(id) => reg.by_id(id).and_then(js_bp).into_iter().collect(),
        Target::Module(m) => reg
            .module(m)
            .and_then(|s| {
                let (ts_line, js) = s.snap_ts(bp.line)?;
                Some(Resolved {
                    script: s.clone(),
                    js_lines: s.js_lines_of(ts_line),
                    shown: Location {
                        script: s.clone(),
                        js,
                        ts: Some((ts_line, 0)),
                    },
                })
            })
            .into_iter()
            .collect(),
    }
}

/// URLs compared without case and with `%3A` as a colon (Windows paths through js-debug).
pub(crate) fn same_url(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.replace("%3A", ":").replace("%3a", ":").to_lowercase();
    norm(a) == norm(b)
}

impl State {
    /// The game thread's working set.
    pub(crate) fn settings(&self) -> Settings {
        let active = |owner: ClientId| self.clients.get(&owner).is_some_and(|c| c.active);
        let mut breakpoints = Vec::new();
        for bp in self.breakpoints.iter().filter(|b| active(b.owner)) {
            for r in resolve(bp, &self.registry) {
                for line in r.js_lines {
                    breakpoints.push(LineBreakpoint {
                        id: bp.id.clone(),
                        script: r.script.clone(),
                        line,
                        condition: bp.condition.clone(),
                        log: bp.log.clone(),
                    });
                }
            }
        }
        let listening = self.clients.values().filter(|c| c.enabled && !c.skip);
        Settings {
            scripts: self.registry.current().cloned().collect(),
            breakpoints,
            watches: self.watches.clone(),
            exceptions: listening
                .clone()
                .map(|c| c.exceptions)
                .max()
                .unwrap_or(ExceptionMode::None),
            debugger_statements: listening.count() > 0,
        }
    }
}
