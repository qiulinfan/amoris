//! The web tests (checks.md 7.2; script-host.md 12, tests 2, 3 and the cross-target determinism of
//! the script host): one report, produced natively by `tests/web.rs` and in WebAssembly by
//! `examples/web_workload.rs` under Node (`tests/web/run.mjs`), which must be the same bytes. It
//! holds the lockdown's verification walk, the replaced `Math` functions checked against the Rust
//! library, the canonical NaN of P6, hashes of the kept operations that reach a C library, the
//! depth limit through calls and through a chain of Proxies, and a script workload's digest and
//! steps at every tick.

use bevy_ecs::prelude::World;
use pocket_sim::entity::EntityIndex;
use pocket_sim::event::{EventCounter, EventInbox};
use pocket_sim::{
    ComponentRegistry, EntityAllocator, NoHooks, Sim, SimClock, SimConfig, TickRate, math,
};

use crate::access::ScriptAccess;
use crate::host::ScriptLimits;
use crate::sandbox::{js_hypot, js_pow};
use crate::source::CompiledSet;

/// The inputs of the `Math` sweeps, as JavaScript and as Rust build them alike.
const SWEEP_JS: &str = r#"(function () {
    const xs = [];
    for (let i = 0; i <= 4000; i++) xs.push(-10 + i * 0.005);
    xs.push(0, -0, NaN, Infinity, -Infinity, 5e-324, 1e-310, 1e308, -1e308, Math.PI, 1, -1, 0.5, 1e-9);
    const unary = ["sin", "cos", "tan", "asin", "acos", "atan", "sinh", "cosh", "tanh", "asinh",
                   "acosh", "atanh", "exp", "expm1", "log", "log1p", "log2", "log10", "cbrt"];
    const out = [];
    for (const f of unary) for (const x of xs) out.push(Math[f](x));
    for (const x of xs) out.push(Math.atan2(x, 0.75), Math.atan2(0.75, x), Math.pow(x, 1.5),
                                 Math.pow(2, x), Math.pow(x, 0.5), Math.hypot(x, 3), Math.hypot(x, 3, -4));
    out.push(Math.pow(1, NaN), Math.pow(-1, Infinity), Math.hypot(), Math.hypot(NaN, Infinity));
    const bytes = new Uint8Array(new Float64Array(out).buffer);
    let h = 0x811c9dc5;
    for (let i = 0; i < bytes.length; i++) h = Math.imul(h ^ bytes[i], 0x01000193) >>> 0;
    return h;
})()"#;

fn sweep_inputs() -> Vec<f64> {
    let mut xs: Vec<f64> = (0..=4000).map(|i| -10.0 + f64::from(i) * 0.005).collect();
    xs.extend([
        0.0,
        -0.0,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        5e-324,
        1e-310,
        1e308,
        -1e308,
        math::PI,
        1.0,
        -1.0,
        0.5,
        1e-9,
    ]);
    xs
}

fn fnv32(h: &mut u32, bytes: &[u8]) {
    for &b in bytes {
        *h = (*h ^ u32::from(b)).wrapping_mul(0x0100_0193);
    }
}

/// What the sweep must give: the Rust library's bits, NaNs canonical as a Float64Array stores
/// them (P6).
fn sweep_reference() -> u32 {
    let xs = sweep_inputs();
    let unary: [fn(f64) -> f64; 19] = [
        math::sin,
        math::cos,
        math::tan,
        math::asin,
        math::acos,
        math::atan,
        math::sinh,
        math::cosh,
        math::tanh,
        math::asinh,
        math::acosh,
        math::atanh,
        math::exp,
        math::expm1,
        math::ln,
        math::ln_1p,
        math::log2,
        math::log10,
        math::cbrt,
    ];
    let mut out = Vec::new();
    for f in unary {
        out.extend(xs.iter().map(|x| f(*x)));
    }
    for &x in &xs {
        out.extend([
            math::atan2(x, 0.75),
            math::atan2(0.75, x),
            js_pow(x, 1.5),
            js_pow(2.0, x),
            js_pow(x, 0.5),
            js_hypot(&[x, 3.0]),
            js_hypot(&[x, 3.0, -4.0]),
        ]);
    }
    out.extend([
        js_pow(1.0, f64::NAN),
        js_pow(-1.0, f64::INFINITY),
        js_hypot(&[]),
        js_hypot(&[f64::NAN, f64::INFINITY]),
    ]);
    let mut h = 0x811c_9dc5;
    for x in out {
        fnv32(&mut h, &math::canonical(x).to_le_bytes());
    }
    h
}

/// The kept operations that reach a C library (script-sandbox.md 2.2): `%`, `floor`, `ceil`,
/// `trunc`, `sqrt`, `round`, `fround`, `min`, `max` over edge operands, and number formatting and
/// parsing: one hash of the numbers and one of the strings.
const KEPT_JS: &str = r#"(function () {
    const xs = [0, -0, 5e-324, 2.2250738585072014e-308, 1e-310, 1, -1, 1.5, -1.5, 2.5, -2.5,
                4503599627370495.5, 1e300, -1e300, Infinity, -Infinity, NaN, 0.1, 7, -7, 3.75,
                1e21, 123.456, -0.000001234, 9007199254740992, 0.30000000000000004];
    const nums = [];
    let text = "";
    for (const a of xs) {
        nums.push(Math.floor(a), Math.ceil(a), Math.trunc(a), Math.sqrt(a), Math.round(a), Math.fround(a));
        for (const b of xs) nums.push(a % b, Math.min(a, b), Math.max(a, b));
        for (const r of [2, 3, 7, 16, 36]) text += a.toString(r) + ",";
        if (Number.isFinite(a)) {
            for (let d = 0; d <= 20; d++) text += a.toFixed(d) + "," + a.toExponential(d) + ",";
            for (let p = 1; p <= 21; p++) text += a.toPrecision(p) + ",";
        }
        text += String(a) + ";";
    }
    for (const s of ["  0x1f", "-077", "1e3", "zz", "123456789012345678901234567890", "0.5", "-0"]) {
        for (const r of [0, 2, 8, 10, 16, 36]) text += parseInt(s, r) + ",";
        text += parseFloat(s) + "," + Number(s) + ";";
    }
    const bytes = new Uint8Array(new Float64Array(nums).buffer);
    let h = 0x811c9dc5;
    for (let i = 0; i < bytes.length; i++) h = Math.imul(h ^ bytes[i], 0x01000193) >>> 0;
    let t = 0x811c9dc5;
    for (let i = 0; i < text.length; i++) t = Math.imul(t ^ text.charCodeAt(i), 0x01000193) >>> 0;
    return [h, t, text.length];
})()"#;

/// P6: a NaN stored into a Float64Array, a Float32Array and a DataView reads back canonical.
const NAN_JS: &str = r#"(function () {
    const f = new Float64Array(2); f[0] = 0 / 0; f[1] = -(0 / 0);
    const g = new Float32Array(1); g[0] = 0 / 0;
    const d = new DataView(new ArrayBuffer(8)); d.setFloat64(0, -(0 / 0), true);
    const h = new Float64Array(1); h.fill(-(0 / 0));
    return [Array.from(new Uint8Array(f.buffer)), Array.from(new Uint8Array(g.buffer)),
            Array.from(new Uint8Array(d.buffer)), Array.from(new Uint8Array(h.buffer))];
})()"#;

/// The deepest frame shape: a JS function, a host native, a JS callback, repeated `N` times
/// (the evaluated script and the function around it are two levels, so 65 of three calls fit).
const DEPTH_JS: &str = r#"(function () {
    function down(n) { return n <= 0 ? 0 : Math.sin({ valueOf: () => down(n - 1) }) * 0 + 1; }
    return down(N);
})()"#;

/// A chain of `N` Proxies read through: each level is one level of P5's counter.
const PROXY_JS: &str = r#"(function () {
    let p = {};
    for (let i = 0; i < N; i++) p = new Proxy(p, {});
    return p.x === undefined;
})()"#;

fn fnv64(h: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *h ^= u64::from(b);
        *h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

/// A digest of what scripts can change: every entity's script-visible components in id order (slot
/// bits and strings), the event inbox and counter, the allocator and the clock. A stand-in for
/// `pocket-persist`'s world hash, which this crate cannot depend on.
pub fn world_digest(world: &World) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let registry = world.resource::<ComponentRegistry>();
    let access = world.get_resource::<ScriptAccess>();
    for (id, e) in world.resource::<EntityIndex>().iter() {
        fnv64(&mut h, &id.get().to_le_bytes());
        let er = world.entity(e);
        for entry in registry.entries() {
            let Some(a) = access.and_then(|a| a.get(entry.id)) else {
                continue;
            };
            if let Some(v) = a.read(&er) {
                fnv64(&mut h, entry.name.as_bytes());
                for x in &v.nums {
                    fnv64(&mut h, &x.to_bits().to_le_bytes());
                }
                for s in &v.strs {
                    fnv64(&mut h, s.as_bytes());
                    fnv64(&mut h, &[0]);
                }
            }
        }
    }
    fnv64(
        &mut h,
        &serde_json::to_vec(world.resource::<EventInbox>()).unwrap_or_default(),
    );
    fnv64(
        &mut h,
        &world.resource::<EventCounter>().next().to_le_bytes(),
    );
    fnv64(
        &mut h,
        &world.resource::<EntityAllocator>().next().to_le_bytes(),
    );
    fnv64(&mut h, &world.resource::<SimClock>().tick.0.to_le_bytes());
    h
}

/// The report: identical bytes natively and in WebAssembly.
pub fn report(set_json: &str, ticks: u64) -> String {
    let mut out = String::new();
    let set = match CompiledSet::from_json(set_json) {
        Ok(s) => s,
        Err(e) => return format!("FAIL compiled set: {e}\n"),
    };
    let limits = ScriptLimits {
        steps_per_call: 50_000_000,
        ..ScriptLimits::default()
    };
    let Ok(mut sim) = Sim::new(SimConfig {
        rate: TickRate::DEFAULT,
        seed: 2026,
    }) else {
        return "FAIL sim\n".into();
    };
    if let Err(p) = crate::scripts::install(&mut sim, limits) {
        return format!("FAIL install: {}\n", p.message);
    }
    if let Err(e) = crate::scripts::swap(sim.world_mut(), &set, false) {
        return format!("FAIL swap: {}\n", e[0]);
    }
    {
        let scripts = sim.world().get_non_send::<crate::scripts::Scripts>();
        let (Some(scripts), true) = (scripts, true) else {
            return "FAIL no scripts\n".into();
        };
        let Some(program) = scripts.program.as_ref() else {
            return "FAIL no program\n".into();
        };
        // Test 1 (lockdown, web): the walk from globalThis and the hidden intrinsics finds nothing
        // writable, configurable or extensible; its counts must match the native report.
        match program.verify_lockdown(&scripts.host) {
            Ok(v) => {
                let problems = v["problems"].as_array().map_or(usize::MAX, Vec::len);
                let mark = if problems == 0 { "ok" } else { "FAIL" };
                out.push_str(&format!(
                    "{mark} lockdown problems {problems} setters {} objects {} roots {}\n",
                    v["setters"].as_array().map_or(0, Vec::len),
                    v["objects"],
                    v["roots"].as_array().map_or(0, Vec::len),
                ));
            }
            Err(e) => out.push_str(&format!("FAIL lockdown: {e}\n")),
        }
        let reference = sweep_reference();
        match program.eval_json(&scripts.host, SWEEP_JS) {
            Ok(v) if v.as_u64() == Some(u64::from(reference)) => {
                out.push_str(&format!("ok math.replaced {reference:08x}\n"))
            }
            other => out.push_str(&format!(
                "FAIL math.replaced: {other:?}, the Rust library gives {reference:08x}\n"
            )),
        }
        let canonical = serde_json::json!([
            [0, 0, 0, 0, 0, 0, 248, 127, 0, 0, 0, 0, 0, 0, 248, 127],
            [0, 0, 192, 127],
            [0, 0, 0, 0, 0, 0, 248, 127],
            [0, 0, 0, 0, 0, 0, 248, 127]
        ]);
        match program.eval_json(&scripts.host, NAN_JS) {
            Ok(v) if v == canonical => out.push_str("ok nan.canonical\n"),
            other => out.push_str(&format!("FAIL nan.canonical: {other:?}\n")),
        }
        match program.eval_json(&scripts.host, KEPT_JS) {
            Ok(v) => out.push_str(&format!("math.kept {v}\n")),
            Err(e) => out.push_str(&format!("FAIL math.kept: {e}\n")),
        }
        // P5 on this target: the deepest frame shape stops by depth within the stack limit, and a
        // nesting just inside the limit runs.
        for (n, want) in [("1000", "script.call_depth"), ("65", "ok")] {
            let got = match program.eval_json(&scripts.host, &DEPTH_JS.replace('N', n)) {
                Ok(_) => "ok".to_owned(),
                Err(e) => e.code,
            };
            let mark = if got == want { "ok" } else { "FAIL" };
            out.push_str(&format!("{mark} depth {n} {got}\n"));
        }
        // P5 through Proxies: a chain past the limit stops by depth on every target (natively the
        // stack check used to stop it first, on wasm32 nothing did), one inside it runs.
        for (n, want) in [("2000", "script.call_depth"), ("150", "ok")] {
            let got = match program.eval_json(&scripts.host, &PROXY_JS.replace('N', n)) {
                Ok(_) => "ok".to_owned(),
                Err(e) => e.code,
            };
            let mark = if got == want { "ok" } else { "FAIL" };
            out.push_str(&format!("{mark} proxies {n} {got}\n"));
        }
    }
    let mut total = 0u64;
    for _ in 0..ticks {
        match sim.step(&mut NoHooks) {
            Ok(r) => {
                let steps: u64 = crate::scripts::last_steps(sim.world())
                    .iter()
                    .map(|(_, s)| s)
                    .sum();
                total += steps;
                out.push_str(&format!(
                    "tick {} {:016x} steps {} events {} errors {}\n",
                    r.tick.0,
                    world_digest(sim.world()),
                    steps,
                    r.events.len(),
                    r.errors.len()
                ));
            }
            Err(p) => {
                out.push_str(&format!("FAIL step: {}\n", p.message));
                break;
            }
        }
    }
    out.push_str(&format!(
        "final {:016x} steps {total}\n",
        world_digest(sim.world())
    ));
    out
}
