//! The step profiler (docs/spec/debugger.md 14): the work counter (P1) as a deterministic sampling
//! clock. While a profile records, every budgeted call arms the counter at the smaller of the steps
//! to the next sample and the steps its budget has left; when the counter runs out, the interrupt
//! handler reads the stack through P10's `JS_GetStackFrameInfo` (P13 makes the innermost frame's
//! position the polling jump's), re-arms the counter and lets the call go on. The budget is kept to
//! the step: a call uses as many steps and stops at the same step as without the profiler, so steps,
//! budget errors and world hashes do not change, and the same run gives the same profile on every
//! target.
//!
//! The gaps between samples are drawn from a fixed-seed generator, uniform in
//! `[1, 2 * interval - 1]` (mean `interval`), so a loop whose iterations take a fixed number of
//! steps cannot alias with the sampling period; `interval` 1 samples every step.
//!
//! The handler has a second caller: libregexp calls it directly, without the counter, every
//! 10,000 of a regular expression's own polls (`lre_check_timeout`), and without a profile that
//! call stops the call as over budget. The interpreter's poll sets the counter to
//! [`POLL_RESET`] before it calls the handler, and a profile never arms the counter above
//! [`MAX_ARM`], so the counter tells the two apart: only the interpreter's poll is a sample point,
//! and libregexp's call stops the call as it would without the profiler.
//!
//! The handler allocates nothing in QuickJS-ng's heap: it keeps the atoms the frames name (one
//! reference each) and turns them into text, and positions into TypeScript, only when the profile is
//! taken, outside any call ([`ScriptProfile`]).

use std::collections::{BTreeMap, BTreeSet};
use std::os::raw::c_int;
use std::rc::Rc;

use rquickjs::qjs::{self, JSAtom, JSContext, JSRuntime};
use serde::{Deserialize, Serialize};

use crate::ffi;
use crate::host::{Maps, Shared};

/// The largest sampling interval, in steps.
pub const MAX_INTERVAL: u32 = 1_000_000;

/// What the interpreter's poll sets the counter to before it calls the interrupt handler
/// (QuickJS-ng's `JS_INTERRUPT_COUNTER_INIT`, quickjs.c; P1).
const POLL_RESET: i32 = 10_000;

/// The most steps a profile arms the counter with: below [`POLL_RESET`], so a counter at
/// [`POLL_RESET`] in the handler is the interpreter's poll, never a value left from an arming.
/// A longer gap or budget is armed in parts, the handler re-arming without a sample.
const MAX_ARM: u32 = 9_999;

/// What kind of frame a sample saw.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Script,
    /// A native function's frame (a builtin that polls, or one that called back into the script).
    Native,
    /// No frame at all: the step the host's own call into the script takes on entry, before the
    /// callee's frame exists (a system's `run`, the prelude's `__context` and `__query`, a job).
    Host,
}

/// A frame as the interrupt handler read it: the atoms it holds a reference to, the JavaScript
/// position (1-based; 0 when unknown) and the source maps of the program it ran in.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct RawFrame {
    kind: Kind,
    file: JSAtom,
    function: JSAtom,
    line: u32,
    column: u32,
    maps: u32,
}

/// The budgeted call being profiled: its budget, the steps it used before the counter was last
/// armed, what the counter was armed with, and what it is counted under.
#[derive(Clone, Copy)]
struct Armed {
    budget: u32,
    base: u32,
    armed: u32,
    label: usize,
    maps: u32,
}

/// What a label (a system, or `(load)` and `(eval)` for the other calls) took.
struct Label {
    name: String,
    calls: u64,
    steps: u64,
}

/// A profile being recorded, in the host's shared state.
pub(crate) struct Recorder {
    interval: u32,
    rng: u64,
    /// Steps to the next sample, at least 1.
    until: u32,
    call: Option<Armed>,
    labels: Vec<Label>,
    maps: Vec<Option<Rc<Maps>>>,
    frames: Vec<RawFrame>,
    frame_ids: BTreeMap<RawFrame, u32>,
    /// Samples by stack: the label, then the frames' ids, innermost first.
    stacks: BTreeMap<Vec<u32>, u64>,
    /// The atoms the recorded frames name, one reference each.
    atoms: BTreeSet<JSAtom>,
    scratch: Vec<u32>,
    samples: u64,
}

impl RawFrame {
    /// A frame with no position: a native one, or the host's.
    fn outside(kind: Kind) -> RawFrame {
        RawFrame {
            kind,
            file: 0,
            function: 0,
            line: 0,
            column: 0,
            maps: 0,
        }
    }
}

/// The fixed seed of the gaps between samples.
const SEED: u64 = 0x5eed_0f5c_41b7_9d13;

impl Recorder {
    fn new(interval: u32) -> Recorder {
        let mut r = Recorder {
            interval,
            rng: SEED,
            until: 1,
            call: None,
            labels: Vec::new(),
            maps: Vec::new(),
            frames: Vec::new(),
            frame_ids: BTreeMap::new(),
            stacks: BTreeMap::new(),
            atoms: BTreeSet::new(),
            scratch: Vec::new(),
            samples: 0,
        };
        r.until = r.gap();
        r
    }

    /// Steps to the next sample: uniform in `[1, 2 * interval - 1]` (splitmix64).
    fn gap(&mut self) -> u32 {
        if self.interval <= 1 {
            return 1;
        }
        self.rng = self.rng.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.rng;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        let span = u64::from(self.interval) * 2 - 1;
        // At most 2 * MAX_INTERVAL - 1: it fits.
        u32::try_from(z % span + 1).unwrap_or(1)
    }

    fn label(&mut self, name: &str) -> usize {
        if let Some(i) = self.labels.iter().position(|l| l.name == name) {
            return i;
        }
        self.labels.push(Label {
            name: name.to_owned(),
            calls: 0,
            steps: 0,
        });
        self.labels.len() - 1
    }

    fn maps_index(&mut self, maps: Option<Rc<Maps>>) -> u32 {
        let found = self.maps.iter().position(|m| match (m, &maps) {
            (Some(a), Some(b)) => Rc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        });
        let i = found.unwrap_or_else(|| {
            self.maps.push(maps);
            self.maps.len() - 1
        });
        u32::try_from(i).unwrap_or(u32::MAX)
    }

    /// Keeps one reference to `atom`, dropping the one the caller hands over if one is kept already.
    ///
    /// # Safety
    /// `ctx` must be live and the reference to `atom` the caller's to give.
    unsafe fn hold(&mut self, ctx: *mut JSContext, atom: JSAtom) {
        if atom == 0 {
            return;
        }
        if !self.atoms.insert(atom) {
            unsafe { qjs::JS_FreeAtom(ctx, atom) };
        }
    }

    /// Records the stack of `ctx` as one sample under `label`.
    ///
    /// # Safety
    /// `ctx` must be the live context of the call being profiled, inside its interrupt handler.
    unsafe fn sample(&mut self, ctx: *mut JSContext, label: usize, maps: u32) {
        let mut stack = std::mem::take(&mut self.scratch);
        stack.clear();
        stack.push(u32::try_from(label).unwrap_or(u32::MAX));
        let mut level: c_int = 0;
        loop {
            let (mut file, mut function, mut line, mut column) = (0, 0, 0, 0);
            let r = unsafe {
                ffi::JS_GetStackFrameInfo(
                    ctx,
                    level,
                    &mut file,
                    &mut function,
                    &mut line,
                    &mut column,
                )
            };
            if r < 0 {
                break;
            }
            let frame = if r == 0 {
                unsafe {
                    self.hold(ctx, file);
                    self.hold(ctx, function);
                }
                RawFrame {
                    kind: Kind::Script,
                    file,
                    function,
                    line: u32::try_from(line).unwrap_or(0),
                    column: u32::try_from(column).unwrap_or(0),
                    maps,
                }
            } else {
                RawFrame::outside(Kind::Native)
            };
            stack.push(self.intern(frame));
            level += 1;
        }
        if level == 0 {
            stack.push(self.intern(RawFrame::outside(Kind::Host)));
        }
        match self.stacks.get_mut(stack.as_slice()) {
            Some(n) => *n += 1,
            None => {
                self.stacks.insert(stack.clone(), 1);
            }
        }
        self.samples += 1;
        self.scratch = stack;
    }

    fn intern(&mut self, frame: RawFrame) -> u32 {
        let next = u32::try_from(self.frames.len()).unwrap_or(u32::MAX);
        let id = *self.frame_ids.entry(frame).or_insert(next);
        if id == next {
            self.frames.push(frame);
        }
        id
    }

    /// Drops every atom reference held.
    ///
    /// # Safety
    /// `rt` must be the live runtime the atoms belong to.
    unsafe fn release(&mut self, rt: *mut JSRuntime) {
        for atom in std::mem::take(&mut self.atoms) {
            unsafe { qjs::JS_FreeAtomRT(rt, atom) };
        }
    }
}

/// The label of the budgeted call starting: its system, else `(load)` while a program loads, else
/// `(eval)`.
fn call_label(sh: &Shared) -> String {
    if let Some(call) = sh.call.borrow().as_ref() {
        return call.name.clone();
    }
    if sh.loading.borrow().is_some() {
        "(load)".to_owned()
    } else {
        "(eval)".to_owned()
    }
}

/// Starts recording in `sh` (a profile already recording starts over).
pub(crate) fn start(sh: &Shared, interval: u32) {
    let old = sh.profile.replace(Some(Recorder::new(interval)));
    if let Some(mut old) = old {
        let rt = sh.rt.get();
        if !rt.is_null() {
            unsafe { old.release(rt) };
        }
    }
}

/// Whether a profile records.
pub(crate) fn recording(sh: &Shared) -> bool {
    sh.profile.borrow().is_some()
}

/// A budgeted call of `budget` steps starts: what to arm the counter with (`budget` when no
/// profile records).
pub(crate) fn begin(sh: &Shared, budget: u32) -> u32 {
    if !recording(sh) {
        return budget;
    }
    let label = call_label(sh);
    let maps = sh.maps.borrow().clone();
    let mut guard = sh.profile.borrow_mut();
    let Some(rec) = guard.as_mut() else {
        return budget;
    };
    let label = rec.label(&label);
    let maps = rec.maps_index(maps);
    let armed = rec.until.min(budget).min(MAX_ARM);
    rec.call = Some(Armed {
        budget,
        base: 0,
        armed,
        label,
        maps,
    });
    armed
}

/// The interrupt handler's part in a budgeted call: when the counter ran out (the interpreter's
/// poll), a sample if one is due, then `Some(counter)` to re-arm with when the call's budget is not
/// spent, or `None` when it is (the call overran), as it would be without the profiler. `None`
/// too for libregexp's direct call, which stops the call without a profile as well.
pub(crate) fn interrupted(sh: &Shared) -> Option<u32> {
    let mut guard = sh.profile.borrow_mut();
    let rec = guard.as_mut()?;
    let call = rec.call?;
    // SAFETY: a profiled call runs in `call_ctx` (begin_budget), which is live during it.
    if unsafe { ffi::JS_GetInterruptCounter(sh.call_ctx.get()) } != POLL_RESET {
        return None;
    }
    // This poll is the call's `pos`-th step (with no budget the first poll stops the call).
    let pos = call.base + call.armed;
    if call.armed > 0 && call.armed == rec.until {
        unsafe { rec.sample(sh.call_ctx.get(), call.label, call.maps) };
        rec.until = rec.gap();
    } else {
        rec.until -= call.armed;
    }
    if pos >= call.budget {
        return None;
    }
    let armed = rec.until.min(call.budget - pos).min(MAX_ARM);
    rec.call = Some(Armed {
        base: pos,
        armed,
        ..call
    });
    Some(armed)
}

/// The budgeted call ends with `left` on the counter: the steps it used when a profile records it,
/// exactly what the host counts without one (`None`: not profiled).
pub(crate) fn end(sh: &Shared, left: i32, exceeded: bool, oom: bool) -> Option<u64> {
    let mut guard = sh.profile.borrow_mut();
    let rec = guard.as_mut()?;
    let call = rec.call.take()?;
    let left = left.max(0).unsigned_abs();
    let used = if exceeded {
        call.budget
    } else if oom {
        // P2 zeroed the counter and the handler reset it: the host's own reading, kept as is.
        call.budget.saturating_sub(left)
    } else {
        let spent = call.armed.saturating_sub(left);
        rec.until = rec.until.saturating_sub(spent).max(1);
        call.base + spent
    };
    let label = &mut rec.labels[call.label];
    label.calls += 1;
    label.steps += u64::from(used);
    Some(u64::from(used))
}

/// A profiled call set aside by [`suspend`], for [`resume`].
pub(crate) struct Suspended(Option<Armed>);

/// Sets the profiled call aside while a debugger's own JavaScript runs under its own budget
/// (`debug::guarded`); [`resume`] puts it back. The caller keeps what was set aside, so guarded
/// evaluations nest (an evaluation inside a debugger's `callFunctionOn`).
pub(crate) fn suspend(sh: &Shared) -> Suspended {
    Suspended(
        sh.profile
            .borrow_mut()
            .as_mut()
            .and_then(|rec| rec.call.take()),
    )
}

/// Puts back what [`suspend`] set aside.
pub(crate) fn resume(sh: &Shared, suspended: Suspended) {
    if let Some(rec) = sh.profile.borrow_mut().as_mut() {
        rec.call = suspended.0;
    }
}

/// Forgets the call a panic cut short.
pub(crate) fn abandon(sh: &Shared) {
    if let Some(rec) = sh.profile.borrow_mut().as_mut() {
        rec.call = None;
    }
}

/// Stops recording and drops what was recorded (the host is going away, or a caller stops
/// without taking the profile).
pub(crate) fn discard(sh: &Shared, rt: *mut JSRuntime) {
    if let Some(mut rec) = sh.profile.borrow_mut().take()
        && !rt.is_null()
    {
        unsafe { rec.release(rt) };
    }
}

/// Stops recording and resolves the profile: atoms into text through `ctx` (any live context of
/// the host's runtime), positions into TypeScript through the programs' source maps.
///
/// # Safety
/// `ctx` must be a live context of the runtime the profile recorded in, on its thread, outside any
/// call.
pub(crate) unsafe fn take(sh: &Shared, ctx: *mut JSContext) -> Option<ScriptProfile> {
    let mut rec = sh.profile.borrow_mut().take()?;
    let mut text: BTreeMap<JSAtom, String> = BTreeMap::new();
    for &atom in &rec.atoms {
        text.insert(atom, unsafe { crate::debug::atom_text(ctx, atom) });
    }
    let frames: Vec<Frame> = rec
        .frames
        .iter()
        .map(|f| {
            match f.kind {
                Kind::Native => return Frame::outside("native"),
                Kind::Host => return Frame::outside("host"),
                Kind::Script => {}
            }
            let file = text.get(&f.file).cloned().unwrap_or_default();
            let function = text.get(&f.function).cloned().unwrap_or_default();
            let (line, _) = match rec.maps.get(f.maps as usize).and_then(Option::as_ref) {
                Some(m) if f.line > 0 => m.map(&file, f.line, f.column.max(1)),
                _ => (f.line, f.column),
            };
            Frame {
                function,
                file,
                line,
            }
        })
        .collect();
    let rt = unsafe { qjs::JS_GetRuntime(ctx) };
    unsafe { rec.release(rt) };
    Some(ScriptProfile::build(&rec, &frames))
}

/// A frame resolved: its function's name (empty for an anonymous one), its module (`amoris` for
/// the prelude, `native` for a native function, `host` for the host's call into the script) and
/// its TypeScript line (0 when unknown).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Frame {
    function: String,
    file: String,
    line: u32,
}

impl Frame {
    fn outside(file: &str) -> Frame {
        Frame {
            function: String::new(),
            file: file.to_owned(),
            line: 0,
        }
    }

    fn name(&self) -> &str {
        match self.file.as_str() {
            "native" => "(native)",
            "host" => "(host)",
            _ if self.function.is_empty() => "(anonymous)",
            _ => &self.function,
        }
    }

    /// The frame in a folded stack: `name (file:line)`, or `(native)` and `(host)`.
    fn folded(&self) -> String {
        match self.file.as_str() {
            "native" | "host" => self.name().to_owned(),
            _ => format!("{} ({}:{})", self.name(), self.file, self.line),
        }
    }
}

/// A profile (docs/spec/debugger.md 14). Steps are exact; samples are counts, each standing for
/// `interval` steps on average.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptProfile {
    /// The mean steps between two samples.
    pub interval: u32,
    /// Steps of every budgeted call while the profile recorded, exact.
    pub steps: u64,
    pub samples: u64,
    /// By system (or `(load)`, `(eval)`), in the order they first ran.
    pub systems: Vec<SystemProfile>,
}

/// One system's share of a profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemProfile {
    pub system: String,
    pub calls: u64,
    /// Exact.
    pub steps: u64,
    pub samples: u64,
    /// By function, the most self samples first.
    pub functions: Vec<FunctionCost>,
    /// By TypeScript line, the most self samples first.
    pub lines: Vec<LineCost>,
    /// Every sampled stack, root first, the most samples first.
    pub stacks: Vec<FoldedStack>,
}

/// A function's samples: where it was the innermost frame (`self_samples`) and where it was
/// anywhere on the stack, once per sample (`total_samples`). Functions are told apart by module
/// and name, as stack traces name them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionCost {
    pub function: String,
    pub file: String,
    pub self_samples: u64,
    pub total_samples: u64,
}

/// A line's samples: where the innermost frame was at it (`self_samples`) and where any frame was,
/// a caller at a call on it included, once per sample (`total_samples`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineCost {
    pub file: String,
    pub line: u32,
    pub function: String,
    pub self_samples: u64,
    pub total_samples: u64,
}

/// A sampled stack, root first, as `name (file:line)` frames.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoldedStack {
    pub frames: Vec<String>,
    pub samples: u64,
}

#[derive(Default)]
struct Cost {
    own: u64,
    total: u64,
}

impl ScriptProfile {
    fn build(rec: &Recorder, frames: &[Frame]) -> ScriptProfile {
        let mut systems: Vec<SystemProfile> = rec
            .labels
            .iter()
            .map(|l| SystemProfile {
                system: l.name.clone(),
                calls: l.calls,
                steps: l.steps,
                samples: 0,
                functions: Vec::new(),
                lines: Vec::new(),
                stacks: Vec::new(),
            })
            .collect();
        let mut functions: Vec<BTreeMap<(String, String), Cost>> =
            systems.iter().map(|_| BTreeMap::new()).collect();
        let mut lines: Vec<BTreeMap<(String, u32, String), Cost>> =
            systems.iter().map(|_| BTreeMap::new()).collect();
        let mut stacks: Vec<BTreeMap<Vec<String>, u64>> =
            systems.iter().map(|_| BTreeMap::new()).collect();
        for (key, &n) in &rec.stacks {
            let Some((&label, ids)) = key.split_first() else {
                continue;
            };
            let s = label as usize;
            let Some(system) = systems.get_mut(s) else {
                continue;
            };
            system.samples += n;
            let stack: Vec<&Frame> = ids.iter().map(|&i| &frames[i as usize]).collect();
            let mut seen_f = BTreeSet::new();
            let mut seen_l = BTreeSet::new();
            for (depth, f) in stack.iter().enumerate() {
                let fk = (f.file.clone(), f.name().to_owned());
                let lk = (f.file.clone(), f.line, f.name().to_owned());
                if seen_f.insert(fk.clone()) {
                    let c = functions[s].entry(fk.clone()).or_default();
                    c.total += n;
                    if depth == 0 {
                        c.own += n;
                    }
                }
                if seen_l.insert(lk.clone()) {
                    let c = lines[s].entry(lk.clone()).or_default();
                    c.total += n;
                    if depth == 0 {
                        c.own += n;
                    }
                }
            }
            let folded: Vec<String> = stack.iter().rev().map(|f| f.folded()).collect();
            *stacks[s].entry(folded).or_default() += n;
        }
        for (i, system) in systems.iter_mut().enumerate() {
            let mut f: Vec<FunctionCost> = std::mem::take(&mut functions[i])
                .into_iter()
                .map(|((file, function), c)| FunctionCost {
                    function,
                    file,
                    self_samples: c.own,
                    total_samples: c.total,
                })
                .collect();
            f.sort_by(|a, b| {
                (b.self_samples, b.total_samples)
                    .cmp(&(a.self_samples, a.total_samples))
                    .then_with(|| (&a.file, &a.function).cmp(&(&b.file, &b.function)))
            });
            system.functions = f;
            let mut l: Vec<LineCost> = std::mem::take(&mut lines[i])
                .into_iter()
                .map(|((file, line, function), c)| LineCost {
                    file,
                    line,
                    function,
                    self_samples: c.own,
                    total_samples: c.total,
                })
                .collect();
            l.sort_by(|a, b| {
                (b.self_samples, b.total_samples)
                    .cmp(&(a.self_samples, a.total_samples))
                    .then_with(|| {
                        (&a.file, a.line, &a.function).cmp(&(&b.file, b.line, &b.function))
                    })
            });
            system.lines = l;
            let mut st: Vec<FoldedStack> = std::mem::take(&mut stacks[i])
                .into_iter()
                .map(|(frames, samples)| FoldedStack { frames, samples })
                .collect();
            st.sort_by(|a, b| {
                b.samples
                    .cmp(&a.samples)
                    .then_with(|| a.frames.cmp(&b.frames))
            });
            system.stacks = st;
        }
        ScriptProfile {
            interval: rec.interval,
            steps: rec.labels.iter().map(|l| l.steps).sum(),
            samples: rec.samples,
            systems,
        }
    }

    /// The stacks in the folded format flame graph tools read (speedscope, inferno,
    /// `flamegraph.pl`): one line per stack, `system;frame;...;frame weight`, root first, the
    /// weight in steps (samples times the interval).
    pub fn folded(&self) -> String {
        let mut out = String::new();
        for s in &self.systems {
            for st in &s.stacks {
                out.push_str(&s.system);
                for f in &st.frames {
                    out.push(';');
                    out.push_str(&f.replace(';', ","));
                }
                out.push(' ');
                out.push_str(&(st.samples * u64::from(self.interval)).to_string());
                out.push('\n');
            }
        }
        out
    }
}
