//! Tests 9 and 11 of script-host.md 12 and script-sandbox.md 4.3: call depth is a deterministic
//! limit that fires before the stack runs out, through every frame shape, QuickJS-ng's
//! C-recursive builtins and chains of Proxies (P5); the stack the default limits need is measured;
//! a memory fault, whether memory runs out on a large allocation or a small one inside `try`, and a
//! panicking native poison the world and leave the process running.
#![cfg(feature = "transpile")]
mod common;

use pocket_script::{EngineFns, ScriptLimits, SystemOutcome};
use pocket_sim::registry::{
    ComponentOrigin, ComponentSchema, FieldType, FieldValue, ProjectValues,
};

fn body(code: &str) -> String {
    format!(
        r#"import {{ game, system }} from "pocket";
function down(n: number): number {{
    if (n <= 0) return 0;
    // A JS function, a host native (Math.sin coerces its argument), a JS callback.
    return Math.sin({{ valueOf: () => down(n - 1) }} as any) * 0 + 1;
}}
function plain(n: number): number {{ return n <= 0 ? 0 : plain(n - 1) + 1; }}
const probe = system({{
    name: "probe", phase: "update", doc: "Recurses.",
    run(ctx) {{
        {code}
    }},
}});
export default game({{ systems: [probe] }});
"#
    )
}

/// Runs `code` in a system on a thread with `thread_stack` bytes; the outcome's code, or "ok".
fn outcome(code: &str, limits: ScriptLimits, thread_stack: usize) -> String {
    let src = body(code);
    std::thread::Builder::new()
        .stack_size(thread_stack)
        .spawn(move || {
            let set = common::compiled(&[("scripts/main.ts", &src)]);
            let mut sim = common::sim_with(&set, limits, 1);
            match sim.step(&mut pocket_sim::NoHooks) {
                Err(p) => p.code,
                Ok(_) => match &pocket_script::scripts::last_tick(sim.world())[0].1 {
                    SystemOutcome::Ok(_) => "ok".into(),
                    SystemOutcome::Failed { error, .. } | SystemOutcome::Fault { error } => {
                        error.code.clone()
                    }
                },
            }
        })
        .unwrap()
        .join()
        .unwrap()
}

const BIG: usize = 64 << 20;

#[test]
fn depth_fails_at_exactly_the_limit() {
    let limits = ScriptLimits::default();
    // `run` is the first level; plain(n) is n + 1 nested calls, so plain(198) is the deepest.
    assert_eq!(outcome("plain(198);", limits, BIG), "ok");
    assert_eq!(outcome("plain(199);", limits, BIG), "script.call_depth");
    // Each `down` level is three calls (down, Math.sin, valueOf): down(n) is 3n + 1.
    assert_eq!(outcome("down(66);", limits, BIG), "ok");
    assert_eq!(outcome("down(67);", limits, BIG), "script.call_depth");
    // A script cannot catch it.
    assert_eq!(
        outcome(
            "try { plain(500); } catch (e) {} ctx.emit(\"game.caught\");",
            limits,
            BIG
        ),
        "script.call_depth"
    );
    // QuickJS-ng's C-recursive builtins count their nesting in the same counter.
    let nested = "let o: any = 0; for (let i = 0; i < 400; i++) o = { a: o };";
    assert_eq!(
        outcome(&format!("{nested} JSON.stringify(o);"), limits, BIG),
        "script.call_depth"
    );
    assert_eq!(
        outcome(
            "JSON.parse(\"[\".repeat(400) + \"]\".repeat(400));",
            limits,
            BIG
        ),
        "script.call_depth"
    );
    assert_eq!(
        outcome(
            "let a: any = [1]; for (let i = 0; i < 400; i++) a = [a]; a.flat(Infinity);",
            limits,
            BIG
        ),
        "script.call_depth"
    );
    assert_eq!(
        outcome(
            "new RegExp(\"(\".repeat(400) + \")\".repeat(400));",
            limits,
            BIG
        ),
        "script.call_depth"
    );
    assert_eq!(
        outcome(
            "new RegExp(\"(\".repeat(100) + \")\".repeat(100));",
            limits,
            BIG
        ),
        "ok"
    );
}

#[test]
fn proxy_chains_stop_by_depth() {
    let limits = ScriptLimits::default();
    // Each Proxy forwards to its target in C, without a call: P5 counts each level, so a chain past
    // the limit stops by depth, with a large stack and with the stack the host asks for alike.
    let target = "let p: any = {}; for (let i = 0; i < 2000; i++) p = new Proxy(p, {}); p.x;";
    let thread = pocket_script::host::THREAD_STACK_BYTES;
    assert_eq!(outcome(target, limits, BIG), "script.call_depth");
    assert_eq!(outcome(target, limits, thread), "script.call_depth");
    // A chain of handlers recurses the same way, through the trap lookup.
    let handler =
        "let h: any = {}; for (let i = 0; i < 2000; i++) h = new Proxy({}, h); new Proxy({}, h).x;";
    assert_eq!(outcome(handler, limits, thread), "script.call_depth");
    // So do the other internal methods.
    for op in [
        "\"x\" in p;",
        "Object.keys(p);",
        "Object.getPrototypeOf(p);",
        "Array.isArray(p);",
    ] {
        let src = format!(
            "let p: any = {{}}; for (let i = 0; i < 2000; i++) p = new Proxy(p, {{}}); {op}"
        );
        assert_eq!(outcome(&src, limits, thread), "script.call_depth", "{op}");
    }
    // A chain inside the limit runs.
    assert_eq!(
        outcome(
            "let p: any = {}; for (let i = 0; i < 150; i++) p = new Proxy(p, {}); p.x;",
            limits,
            thread
        ),
        "ok"
    );
}

/// The smallest stack limit at which the deepest frame shape still fails by depth rather than by
/// stack: what the call uses at `max_call_depth`.
fn measured_stack(limits: ScriptLimits) -> usize {
    let deepest = "down(1000);";
    let (mut lo, mut hi) = (16 * 1024usize, 32 << 20);
    while hi - lo > 4096 {
        let mid = (lo + hi) / 2;
        let o = outcome(
            deepest,
            ScriptLimits {
                stack_bytes: mid,
                ..limits
            },
            BIG,
        );
        if o == "script.call_depth" {
            hi = mid;
        } else {
            assert_eq!(o, "script.stack_overflow");
            lo = mid;
        }
    }
    hi
}

#[test]
fn the_default_stack_holds_the_depth_limit() {
    let limits = ScriptLimits::default();
    let used = measured_stack(limits);
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    println!(
        "{profile}: {used} bytes of stack at max_call_depth {}; stack_bytes {} (needs {})",
        limits.max_call_depth,
        limits.stack_bytes,
        used * 3 / 2
    );
    assert!(
        limits.stack_bytes >= used * 3 / 2,
        "stack_bytes {} < 1.5 x {used}",
        limits.stack_bytes
    );
    // On a thread of the size the host asks for, the deepest shape fails by depth.
    let thread = pocket_script::host::THREAD_STACK_BYTES;
    assert_eq!(outcome("down(1000);", limits, thread), "script.call_depth");
}

#[test]
fn a_memory_fault_poisons_the_world() {
    let limits = ScriptLimits {
        memory_bytes: 8 << 20,
        ..ScriptLimits::default()
    };
    let src = body(
        "const keep: number[][] = []; for (let i = 0; i < 1e6; i++) keep.push(new Array(1000).fill(i));",
    );
    let set = common::compiled(&[("scripts/main.ts", &src)]);
    let mut sim = common::sim_with(&set, limits, 1);
    let p = sim.step(&mut pocket_sim::NoHooks).unwrap_err();
    assert_eq!(p.code, "script.out_of_memory", "{p:#?}");
    assert!(sim.poisoned().is_some());
    assert_eq!(
        sim.step(&mut pocket_sim::NoHooks).unwrap_err().code,
        "sim.world_poisoned"
    );
    // P2: a script cannot catch running out of memory and carry on.
    let src = body(
        "try { const keep: number[][] = []; for (let i = 0; i < 1e6; i++) keep.push(new Array(1000).fill(i)); } catch (e) { ctx.emit(\"game.survived\"); }",
    );
    let set = common::compiled(&[("scripts/main.ts", &src)]);
    let mut sim = common::sim_with(&set, limits, 1);
    let p = sim.step(&mut pocket_sim::NoHooks).unwrap_err();
    assert_eq!(p.code, "script.out_of_memory", "{p:#?}");
}

#[test]
fn running_out_of_memory_on_small_allocations_faults_too() {
    // Small allocations fail where no error object can be made: P2 throws the context's own,
    // made in advance, so the call still faults instead of catching `null` and carrying on.
    let limits = ScriptLimits {
        memory_bytes: 4 << 20,
        steps_per_system: 100_000_000,
        steps_per_tick: 100_000_000,
        ..ScriptLimits::default()
    };
    for shape in [
        "let head: any = null; for (let i = 0; i < 1e8; i++) head = { next: head };",
        "let head: any = null; for (let i = 0; i < 1e8; i++) head = [head];",
        "let head: any = null; for (let i = 0; i < 1e8; i++) { const h = head; head = () => h; }",
        "let s = \"\"; const keep: string[] = []; for (let i = 0; i < 1e8; i++) keep.push(s + i);",
    ] {
        let src = body(&format!(
            "try {{ {shape} }} catch (e) {{ ctx.emit(\"game.survived\", {{ e: String(e) }}); }}"
        ));
        let set = common::compiled(&[("scripts/main.ts", &src)]);
        let mut sim = common::sim_with(&set, limits, 1);
        let p = sim.step(&mut pocket_sim::NoHooks).unwrap_err();
        assert_eq!(p.code, "script.out_of_memory", "{shape}: {p:#?}");
        assert!(sim.poisoned().is_some(), "{shape}");
    }
}

/// An engine component whose accessor panics: an engine bug a native meets.
#[derive(
    bevy_ecs::component::Component,
    Clone,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
struct Broken {
    v: f64,
}

impl pocket_sim::Persisted for Broken {
    const NAME: &'static str = "Broken";
    const VERSION: u32 = 1;
}

fn panics(_: &Broken) -> ProjectValues {
    panic!("the accessor is broken")
}

/// A world with `Broken` registered (its accessor reading fine), the scripts installed and `src`
/// swapped in, stepped once; then the accessor is broken.
fn broken_world(src: &str) -> pocket_sim::Sim {
    let set = common::compiled(&[("scripts/main.ts", src)]);
    let mut sim = pocket_sim::Sim::new(pocket_sim::SimConfig {
        rate: pocket_sim::TickRate::DEFAULT,
        seed: 1,
    })
    .unwrap();
    pocket_script::install(&mut sim, ScriptLimits::default()).unwrap();
    let schema = ComponentSchema::new(
        "Broken",
        ComponentOrigin::Engine,
        1,
        "Broken.",
        vec![("v", FieldType::F64, FieldValue::F64(0.0), "A value.")],
    )
    .unwrap();
    pocket_sim::ComponentRegistry::register::<Broken>(sim.world_mut(), Some(schema)).unwrap();
    pocket_script::register_engine::<Broken>(
        sim.world_mut(),
        EngineFns {
            read: |b| ProjectValues {
                nums: vec![b.v].into(),
                strs: Box::new([]),
            },
            write: |b, v| b.v = v.nums[0],
            make: |v| Broken { v: v.nums[0] },
        },
    )
    .unwrap();
    pocket_script::swap(sim.world_mut(), &set, false).unwrap();
    assert!(sim.step(&mut pocket_sim::NoHooks).is_ok());
    // Now break the accessor.
    pocket_script::register_engine::<Broken>(
        sim.world_mut(),
        EngineFns {
            read: panics,
            write: |b, v| b.v = v.nums[0],
            make: |v| Broken { v: v.nums[0] },
        },
    )
    .unwrap();
    sim
}

const SPAWN: &str = r#"const probe = system({
    name: "probe", phase: "update", doc: "Spawns a broken component.", when: "start",
    run(ctx) {
        ctx.world.spawn({ Broken: { v: 1 } });
    },
});
"#;

#[test]
fn a_panicking_native_poisons_the_world_and_the_process_lives() {
    // The panic happens inside a native (ctx.query reads the broken accessor): the error is
    // uncatchable, so the script's catch never runs.
    let src = format!(
        r#"import {{ game, system }} from "pocket";
{SPAWN}const read = system({{
    name: "read", phase: "update", doc: "Reads it.", when: {{ every: 2 }},
    run(ctx) {{
        try {{ ctx.query({{ with: ["Broken"] }}); }} catch (e) {{ ctx.emit("game.caught"); }}
    }},
}});
export default game({{ systems: [probe, read] }});
"#
    );
    let mut sim = broken_world(&src);
    let p = sim.step(&mut pocket_sim::NoHooks).unwrap_err();
    assert_eq!(p.code, "sim.internal", "{p:#?}");
    assert!(p.message.contains("the accessor is broken"), "{p:#?}");
    assert!(sim.poisoned().is_some());
    let scripts = sim.world().get_non_send::<pocket_script::Scripts>();
    let outcome = &scripts.expect("the scripts stay in the world").last_tick[0];
    assert!(
        matches!(outcome.1, SystemOutcome::Fault { .. }),
        "{outcome:#?}"
    );
    assert!(
        sim.world()
            .resource::<pocket_sim::event::EventInbox>()
            .events()
            .iter()
            .all(|e| e.kind.as_str() != "game.caught"),
        "the catch ran"
    );
}

#[test]
fn a_panic_outside_any_native_faults_and_keeps_the_scripts() {
    // The declared query is prepared in the host before `run`, outside any native: the update
    // system's own guard turns the panic into a fault, and the scripts go back into the world.
    let src = format!(
        r#"import {{ game, system }} from "pocket";
{SPAWN}const read = system({{
    name: "read", phase: "update", doc: "Reads it.", when: {{ every: 2 }},
    queries: {{ all: {{ with: ["Broken"] }} }},
    run(ctx, {{ all }}) {{
        ctx.emit("game.read", {{ n: all.len }});
    }},
}});
export default game({{ systems: [probe, read] }});
"#
    );
    let mut sim = broken_world(&src);
    let p = sim.step(&mut pocket_sim::NoHooks).unwrap_err();
    assert_eq!(p.code, "sim.internal", "{p:#?}");
    assert!(p.message.contains("the accessor is broken"), "{p:#?}");
    assert!(sim.poisoned().is_some());
    let scripts = sim.world().get_non_send::<pocket_script::Scripts>();
    assert!(
        scripts.is_some_and(|s| s.program.is_some()),
        "the scripts left the world"
    );
}
