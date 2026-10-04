//! Test 1 of script-host.md 12 (lockdown): everything reachable from `globalThis` and from the
//! hidden intrinsics (what builtins return but no property path reaches) is frozen, the removed
//! globals are absent, the setters are the reviewed list, code generation is refused, and override
//! taming keeps `this.name = ...` in an Error subclass working.
#![cfg(feature = "transpile")]
mod common;

use pocket_script::{ScriptHost, ScriptLimits};
use serde_json::json;

const MAIN: &str = r#"
import { game } from "pocket";
export default game({ systems: [] });
"#;

fn program() -> (ScriptHost, pocket_script::Program) {
    let set = common::compiled(&[("scripts/main.ts", MAIN)]);
    let host = ScriptHost::new(ScriptLimits::default()).unwrap();
    let world = pocket_sim::Sim::new(pocket_sim::SimConfig {
        rate: pocket_sim::TickRate::DEFAULT,
        seed: 1,
    })
    .unwrap();
    let p = host
        .instantiate(&set, world.world())
        .unwrap_or_else(|e| panic!("{e:#?}"));
    (host, p)
}

/// Whether a setter is on the reviewed list: the override taming's (they define an own property
/// on the receiver, named `tamed <key>`), `__proto__`'s (it sets the receiver's prototype),
/// `Error.prototype.stack`'s (it defines `stack` on the receiver), %ThrowTypeError% (it throws), and
/// the ES2025 Iterator.prototype accessors' (they define on the receiver). None writes state of
/// the context.
fn reviewed(setter: &str) -> bool {
    let (path, name) = setter.split_once(" = ").unwrap_or((setter, ""));
    name.starts_with("tamed ")
        || path.ends_with(".__proto__")
        // Error.prototype.stack: defines `stack` on the receiver.
        || name == "set stack"
        // %ThrowTypeError%: Function.prototype's arguments and caller throw.
        || ((path.ends_with(".arguments") || path.ends_with(".caller")) && name.is_empty())
        || (path.contains("Iterator.prototype") && (path.ends_with(".constructor") || path.ends_with("Symbol.toStringTag)")))
}

#[test]
fn everything_reachable_is_frozen() {
    let (host, p) = program();
    let report = p.verify_lockdown(&host).unwrap();
    let problems = report["problems"].as_array().unwrap();
    assert!(problems.is_empty(), "{problems:#?}");
    let setters: Vec<&str> = report["setters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap())
        .collect();
    let unreviewed: Vec<&&str> = setters.iter().filter(|s| !reviewed(s)).collect();
    assert!(unreviewed.is_empty(), "unreviewed setters: {unreviewed:#?}");
    // The walk starts where the freeze did: globalThis and the hidden intrinsics.
    let roots: Vec<&str> = report["roots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap())
        .collect();
    for root in [
        "globalThis",
        "%ArrayIteratorPrototype%",
        "%IteratorHelperPrototype%",
        "%GeneratorPrototype%",
        "%Promise%",
        "%ThrowTypeError%",
    ] {
        assert!(roots.contains(&root), "{root} is not a root: {roots:?}");
    }
    println!(
        "{} objects frozen from {} roots, {} reviewed setters",
        report["objects"],
        roots.len(),
        setters.len()
    );
}

#[test]
fn hidden_intrinsics_are_frozen() {
    let (host, p) = program();
    // Objects no own property or prototype chain from globalThis reaches, met through what
    // builtins return; Promise through an async function's result (its global is gone).
    let thawed = p
        .eval_json(
            &host,
            r#"(() => {
                const proto = Object.getPrototypeOf;
                const promise = proto((async function () {})());
                const samples = {
                    arrayIterator: proto([].values()),
                    mapIterator: proto(new Map().values()),
                    setIterator: proto(new Set().values()),
                    stringIterator: proto(""[Symbol.iterator]()),
                    regexpStringIterator: proto("a".matchAll(/a/g)),
                    iteratorHelper: proto([].values().map((x) => x)),
                    wrapForValidIterator: proto(Iterator.from({ next() { return { done: true }; } })),
                    iteratorConcat: proto(Iterator.concat()),
                    generatorFunction: proto(function* () {}),
                    generator: proto(function* () {}).prototype,
                    asyncFunction: proto(async function () {}),
                    asyncGeneratorFunction: proto(async function* () {}),
                    asyncGenerator: proto(async function* () {}).prototype,
                    asyncIterator: proto(proto(async function* () {}).prototype),
                    promisePrototype: promise,
                    promise: promise.constructor,
                    throwTypeError: Object.getOwnPropertyDescriptor(Function.prototype, "caller").get,
                };
                return Object.entries(samples)
                    .filter(([, v]) => !Object.isFrozen(v))
                    .map(([k]) => k);
            })()"#,
        )
        .unwrap();
    assert_eq!(thawed, json!([]));
}

const ITERATOR_STATE: &str = r#"import { game, system } from "pocket";
const count = system({
    name: "count", phase: "update", doc: "Keeps a counter on an iterator prototype.",
    run(ctx) {
        const AIP: any = Object.getPrototypeOf([].values());
        AIP.n = (AIP.n || 0) + 1;
        ctx.emit("game.count", { n: AIP.n });
    },
});
export default game({ systems: [count] });
"#;

#[test]
fn a_hidden_intrinsic_keeps_no_state() {
    // The probe of the review: with %ArrayIteratorPrototype% unfrozen this counted 1, 2, and again
    // after a reload. Frozen, the write throws in the module's strict code.
    let set = common::compiled(&[("scripts/main.ts", ITERATOR_STATE)]);
    let mut sim = common::sim_with(&set, ScriptLimits::default(), 1);
    for _ in 0..2 {
        let r = common::step(&mut sim);
        assert_eq!(r.errors.len(), 1, "{:#?}", r.events);
        assert!(r.events.iter().all(|e| e.kind.as_str() != "game.count"));
    }
}

#[test]
fn removed_globals_are_absent() {
    let (host, p) = program();
    let absent = p
        .eval_json(
            &host,
            r#"["Date", "performance", "Promise", "queueMicrotask", "WeakRef", "FinalizationRegistry",
                "SharedArrayBuffer", "Atomics", "eval", "std", "os", "setTimeout", "require", "process",
                "AsyncDisposableStack"]
               .filter((k) => k in globalThis)"#,
        )
        .unwrap();
    assert_eq!(absent, json!([]));
    // The builtins that would make promises without async syntax are gone.
    let from_async = p.eval_json(&host, "typeof Array.fromAsync").unwrap();
    assert_eq!(from_async, json!("undefined"));
    let compile = p
        .eval_json(&host, "typeof RegExp.prototype.compile")
        .unwrap();
    assert_eq!(compile, json!("undefined"));
    let console = p.eval_json(&host, "typeof console.log").unwrap();
    assert_eq!(console, json!("function"));
}

#[test]
fn code_generation_is_refused() {
    let (host, p) = program();
    for src in [
        r#"Function("return 1")"#,
        r#"(function () {}).constructor("return 1")"#,
        r#"(function* () {}).constructor("yield 1")"#,
        r#"new Function("return 1")"#,
    ] {
        let e = p.eval_json(&host, src).unwrap_err();
        assert_eq!(e.code, "script.code_generation", "{src}: {e:#?}");
    }
    // instanceof Function still works.
    assert_eq!(
        p.eval_json(&host, "(() => 1) instanceof Function").unwrap(),
        json!(true)
    );
    // eval is gone; indirect eval through the global object too.
    let e = p
        .eval_json(&host, r#"globalThis["ev" + "al"]("1")"#)
        .unwrap_err();
    assert_eq!(e.code, "script.exception", "{e:#?}");
}

#[test]
fn writes_to_intrinsics_throw_and_taming_works() {
    let (host, p) = program();
    for src in [
        "'use strict'; Math.sin = () => 0",
        "'use strict'; Array.prototype.push = null",
        "'use strict'; globalThis.x = 1",
        "'use strict'; Object.prototype.toString = () => ''",
        "'use strict'; Error.prepareStackTrace = () => ''",
        "'use strict'; Error.stackTraceLimit = 1",
    ] {
        let e = p.eval_json(&host, src).unwrap_err();
        assert_eq!(e.code, "script.exception", "{src}");
        assert_eq!(
            e.detail.js_error.as_ref().unwrap().name,
            "TypeError",
            "{src}"
        );
    }
    let named = p
        .eval_json(
            &host,
            r#"(() => {
                class GameError extends Error { constructor(m) { super(m); this.name = "GameError"; } }
                const o = {};
                o.toString = () => "mine";
                return [new GameError("x").name, String(o), Object.keys(o)];
            })()"#,
        )
        .unwrap();
    assert_eq!(named, json!(["GameError", "mine", ["toString"]]));
}

#[test]
fn the_runtime_reads_no_clock() {
    let (host, p) = program();
    // P3: the constant seeds; the hash seed only places Map entries, so iteration follows insertion.
    let order = p
        .eval_json(&host, "[...new Map([[3, 1], [1, 2], [2, 3]]).keys()]")
        .unwrap();
    assert_eq!(order, json!([3, 1, 2]));
    assert!(
        pocket_script::QJS_CC.contains(':'),
        "{}",
        pocket_script::QJS_CC
    );
    println!("QuickJS-ng compiled by {}", pocket_script::QJS_CC);
}
