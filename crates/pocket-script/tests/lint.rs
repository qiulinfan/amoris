//! The stateless-script lint (script-sandbox.md 3; script-host.md 12, test 6): every rule has a
//! refused and an accepted fixture, among them the cases the spikes' lints let through.
#![cfg(feature = "transpile")]
mod common;

use pocket_script::{CompileOptions, ScriptSource, compile};

const HEAD: &str = "import { game, system, component, field, freeze } from \"pocket\";\n";
const TAIL: &str = "\nexport default game({ systems: [] });\n";

/// The lint's findings for a module body (line 2 onward), as `(code, line)`.
fn lint(body: &str) -> Vec<(String, u32)> {
    let src = ScriptSource::new().with("scripts/main.ts", &format!("{HEAD}{body}{TAIL}"));
    match compile(&src, &CompileOptions::default()) {
        Ok(_) => Vec::new(),
        Err(e) => e
            .into_iter()
            .map(|e| (e.code, e.detail.location.map_or(0, |l| l.line)))
            .collect(),
    }
}

fn refused(body: &str, code: &str, line: u32) {
    let found = lint(body);
    assert!(
        found.iter().any(|(c, l)| c == code && *l == line),
        "{body}\nexpected {code} at {line}, got {found:?}"
    );
}

fn accepted(body: &str) {
    let found = lint(body);
    assert!(found.is_empty(), "{body}\nrefused: {found:?}");
}

#[test]
fn module_let() {
    refused("let count = 0;", "lint.module_let", 2);
    refused("var seen = [];", "lint.module_let", 2);
    refused("export let total = 1;", "lint.module_let", 2);
    accepted("const LIMIT = 10;\nfunction f() { let local = 0; local++; return local; }");
}

#[test]
fn module_reassign() {
    refused(
        "function f() {}\nfunction g() { f = () => 1; }",
        "lint.module_reassign",
        3,
    );
    refused(
        "class C {}\nexport function g() { (C as any) = null; }",
        "lint.module_reassign",
        3,
    );
    refused(
        "const xs = [1];\nexport function g() { for (xs of [[2]]) {} }",
        "lint.module_reassign",
        3,
    );
    accepted("const xs = [1, 2];\nexport function g() { for (const x of xs) { void x; } }");
}

#[test]
fn module_statement() {
    refused("const t = { n: 0 };\nif (t) {}", "lint.module_statement", 3);
    refused("for (;;) { break; }", "lint.module_statement", 2);
    refused("import fs = require(\"fs\");", "lint.module_statement", 2);
    accepted(
        "export const A = 1;\nexport function f() { return A; }\nexport class K {}\nexport enum E { A, B }\nexport type T = number;\ninterface I { x: number }",
    );
}

#[test]
fn load_time_code() {
    refused(
        "function make() { return {}; }\nconst t = make();",
        "lint.load_time_code",
        3,
    );
    refused("const m = new Map();", "lint.load_time_code", 2);
    refused("const s = String.raw`x`;", "lint.load_time_code", 2);
    refused(
        "class C { static n = make(); }\nfunction make() { return 1; }",
        "lint.load_time_code",
        2,
    );
    refused("class C { static { } }", "lint.load_time_code", 2);
    refused(
        "class C { static get n() { return 1; } }",
        "lint.load_time_code",
        2,
    );
    refused(
        "class C {}\nconst p = C.prototype;",
        "lint.load_time_code",
        3,
    );
    refused(
        "const o = { get x() { return 1; } };",
        "lint.load_time_code",
        2,
    );
    refused("const r = /a/g;", "lint.load_time_code", 2);
    refused(
        "const t = { n: 1 };\nconst d = delete (t as any).n;",
        "lint.load_time_code",
        3,
    );
    refused(
        "enum E { A = make() }\nfunction make() { return 1; }",
        "lint.load_time_code",
        2,
    );
    refused(
        "const { a = make() } = { a: 1 };\nfunction make() { return 1; }",
        "lint.load_time_code",
        2,
    );
    accepted(
        "const A = [1, 2, 3];\nconst B = { a: 1, f() { return 2; }, ...{ c: 3 } };\nconst C = `x${A.length}`;\n\
         const D = Math.sqrt(2) + Number.parseFloat(\"1.5\") + Object.keys(B).length;\nconst E = freeze({ x: 1 });\n\
         const F = A.length > 2 ? \"many\" : \"few\";\nconst G = /a/i;\nconst H = field.f64(0, \"doc\");\n\
         const S = system({ name: \"s\", phase: \"update\", doc: \"d\", run(ctx) {} });",
    );
}

#[test]
fn load_time_hook() {
    // A load-time spread of an object whose iterator is project code.
    refused(
        "const it = { *[Symbol.iterator]() { yield 1; } };\nconst xs = [...(it as any)];",
        "lint.load_time_hook",
        2,
    );
    // `x + 1` on an object with valueOf.
    refused(
        "const v = { valueOf() { return 1; } };\nconst w = (v as any) + 1;",
        "lint.load_time_hook",
        2,
    );
    refused(
        "const j = { toJSON() { return 1; } };",
        "lint.load_time_hook",
        2,
    );
    // instanceof against a class with a static [Symbol.hasInstance].
    refused(
        "class K { static [Symbol.hasInstance](x: unknown) { return true; } }",
        "lint.load_time_hook",
        2,
    );
    refused(
        "class K { static toString() { return \"k\"; } }",
        "lint.load_time_hook",
        2,
    );
    accepted(
        "class K { toString() { return \"k\"; } [Symbol.iterator]() { return [][Symbol.iterator](); } }",
    );
}

#[test]
fn static_private() {
    refused(
        "class Spawner { static #n = 0; static next() { return ++Spawner.#n; } }",
        "lint.static_private",
        2,
    );
    refused(
        "class Cache { static #cache = {}; }",
        "lint.static_private",
        2,
    );
    accepted("class Point { #x = 0; get x() { return this.#x; } }");
}

#[test]
fn async_code() {
    refused("export async function f() {}", "lint.async", 2);
    refused("export const f = async () => 1;", "lint.async", 2);
    refused(
        "export function f() { return import(\"./x\"); }",
        "lint.async",
        2,
    );
    refused("export const u = import.meta;", "lint.async", 2);
    accepted("export function* count() { yield 1; yield 2; }");
}

#[test]
fn removed_global() {
    refused(
        "export function f() { return Date.now(); }",
        "lint.removed_global",
        2,
    );
    refused(
        "export function f() { return performance.now(); }",
        "lint.removed_global",
        2,
    );
    refused(
        "export function f() { return new Promise(() => {}); }",
        "lint.removed_global",
        2,
    );
    refused(
        "export function f() { return eval(\"1\"); }",
        "lint.removed_global",
        2,
    );
    refused(
        "export function f() { (globalThis as any).x = 1; }",
        "lint.removed_global",
        2,
    );
    refused(
        "const RE = /a/;\nexport function f(n: number) { try { (RE as any).compile(String(n + 1)); } catch {} }",
        "lint.removed_global",
        3,
    );
    // The builtins that make promises without async syntax.
    refused(
        "export function f() { return Array.fromAsync([1]); }",
        "lint.removed_global",
        2,
    );
    refused(
        "export function f() { return new AsyncDisposableStack(); }",
        "lint.removed_global",
        2,
    );
    // A local binding of the same name is not the global.
    accepted("export function f() { const Date = 1; return Date; }");
    accepted("export function f() { return Array.from([1]); }");
}

#[test]
fn reserved_name() {
    // The harden epilogue's own bindings: a top-level one would collide with them, and a reference
    // would reach the private native.
    refused("const __pocket_harden = 1;", "lint.reserved_name", 2);
    refused(
        "export function __pocket_self() {}",
        "lint.reserved_name",
        2,
    );
    refused(
        "export function f() {\n    return (__pocket_harden as any)(1);\n}",
        "lint.reserved_name",
        3,
    );
    refused(
        "export function f(__pocket_x: number) { return __pocket_x; }",
        "lint.reserved_name",
        2,
    );
    accepted("const pocket_harden = 1;\nexport const __proto__ = { n: pocket_harden };");
}

#[test]
fn decorator_namespace_private_module() {
    refused(
        "function dec(x: any) { return x; }\n@dec class K {}",
        "lint.decorator",
        3,
    );
    refused("namespace NS { export const x = 1; }", "lint.namespace", 2);
    accepted(
        "namespace Types { export interface P { x: number } }\ndeclare namespace Ambient { const y: number; }",
    );
    refused(
        "import * as h from \"pocket:host\";",
        "lint.private_module",
        2,
    );
}

#[test]
fn an_import_mutated_by_a_method_call_passes_the_lint_and_fails_at_run_time() {
    // The lint cannot see `list.push()` through an import (the script-native spike's let it
    // through); the harden epilogue froze the value, so the call throws at the mutation
    // (tests/errors.rs runs it).
    let src = ScriptSource::new()
        .with("scripts/list.ts", "export const list: number[] = [];\n")
        .with("scripts/main.ts", "import { game, system } from \"pocket\";\nimport { list } from \"./list\";\nexport default game({ systems: [system({ name: \"s\", phase: \"update\", doc: \"d\", run(ctx) { list.push(1); } })] });\n");
    assert!(compile(&src, &CompileOptions::default()).is_ok());
}

#[test]
fn lint_off_is_only_for_the_negative_control() {
    let src = ScriptSource::new().with("scripts/main.ts", &format!("{HEAD}let n = 0;{TAIL}"));
    assert!(compile(&src, &CompileOptions::default()).is_err());
    assert!(compile(&src, &CompileOptions { lint_off: true }).is_ok());
}
