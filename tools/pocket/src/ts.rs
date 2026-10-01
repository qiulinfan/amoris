//! TypeScript transform and bundling.
//!
//! Every module is parsed and transformed with oxc (types stripped, TSX lowered), then its
//! import and export statements are rewritten by span into a tiny CommonJS-style module
//! registry so that the engine can evaluate one script. Live bindings are not preserved;
//! gameplay code does not need them. Bare specifiers `pocket` and `pocket/<name>` resolve
//! to the SDK runtime directory; any other bare specifier is a package in a `node_modules`
//! beside the importing file or above it, entered through its package.json (`exports` under
//! the import, module, browser, default and require conditions, else `module`, `browser`,
//! `main`). A package's ES modules take the same path as the project's; its CommonJS files
//! are wrapped with `module`, `exports` and `require` (the specifiers it requires with string
//! literals are bundled; anything else, Node's built-ins among them, throws when required).
//! JSON files are modules whose export is their value.

use anyhow::{anyhow, bail, Context, Result};
use indexmap::IndexMap;
use oxc::allocator::Allocator;
use oxc::ast::ast::{
    Argument, BindingPattern, CallExpression, Declaration, ExportDefaultDeclarationKind, Expression, ImportDeclarationSpecifier, ModuleExportName, Statement,
};
use oxc::ast_visit::{walk, Visit};
use oxc::codegen::{Codegen, CodegenOptions};
use oxc::parser::Parser;
use oxc::semantic::SemanticBuilder;
use oxc::span::{GetSpan, SourceType, Span};
use oxc::transformer::{JsxRuntime, TransformOptions, Transformer};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct ProjectFile {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub entry: Option<String>,
    #[serde(default)]
    pub scene: Option<String>,
    #[serde(default)]
    pub window: Option<toml::Value>,
    #[serde(flatten)]
    pub other: IndexMap<String, toml::Value>,
}

pub struct BundleOutput {
    pub modules: Vec<String>,
}

struct Rewrite {
    span: Span,
    replacement: String,
}

/// Transform one TypeScript/TSX file to plain JavaScript (ES module syntax kept), with the line of
/// the source each line of it came from (1-based; a line nothing maps keeps the line before's).
pub fn transform_to_js(path: &Path, source: &str) -> Result<(String, Vec<u32>)> {
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(path).unwrap_or_else(|_| SourceType::ts());
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if parsed.diagnostics.has_errors() {
        let msgs: Vec<String> = parsed.diagnostics.iter().map(|e| e.to_string()).collect();
        bail!("{}: parse errors:\n{}", path.display(), msgs.join("\n"));
    }
    let mut program = parsed.program;
    let scoping = SemanticBuilder::new().build(&program).semantic.into_scoping();
    let mut options = TransformOptions::default();
    // JSX compiles to calls into the Pocket UI SDK (`pocket/jsx-runtime`), never React.
    options.jsx.runtime = JsxRuntime::Automatic;
    options.jsx.import_source = Some("pocket".into());
    options.jsx.jsx_plugin = true;
    let ret = Transformer::new(&allocator, path, &options).build_with_scoping(scoping, &mut program);
    if ret.diagnostics.has_errors() {
        let msgs: Vec<String> = ret.diagnostics.iter().map(|e| e.to_string()).collect();
        bail!("{}: transform errors:\n{}", path.display(), msgs.join("\n"));
    }
    let options = CodegenOptions { source_map_path: Some(path.to_path_buf()), ..CodegenOptions::default() };
    let out = Codegen::new().with_options(options).build(&program);
    let mut lines = vec![0u32; out.code.matches('\n').count() + 1];
    if let Some(map) = &out.map {
        // The first token on a line is its leftmost: where the statement there starts.
        for t in map.get_tokens() {
            let at = t.get_dst_line() as usize;
            if at < lines.len() && lines[at] == 0 {
                lines[at] = t.get_src_line() + 1;
            }
        }
    }
    let mut last = 1;
    for l in lines.iter_mut() {
        if *l == 0 {
            *l = last;
        } else {
            last = *l;
        }
    }
    Ok((out.code, lines))
}

/// Lines 1..=n of a file kept as it is.
fn same_lines(text: &str) -> Vec<u32> {
    (1..=text.matches('\n').count() as u32 + 1).collect()
}

fn export_name(n: &ModuleExportName) -> String {
    n.name().to_string()
}

fn binding_names(pat: &BindingPattern, out: &mut Vec<String>) {
    match pat {
        BindingPattern::BindingIdentifier(id) => out.push(id.name.to_string()),
        BindingPattern::ObjectPattern(p) => {
            for prop in &p.properties {
                binding_names(&prop.value, out);
            }
            if let Some(rest) = &p.rest {
                binding_names(&rest.argument, out);
            }
        }
        BindingPattern::ArrayPattern(p) => {
            for el in p.elements.iter().flatten() {
                binding_names(el, out);
            }
            if let Some(rest) = &p.rest {
                binding_names(&rest.argument, out);
            }
        }
        BindingPattern::AssignmentPattern(p) => binding_names(&p.left, out),
    }
}

/// Rewrite ES module syntax in plain JavaScript into registry calls. Returns the module body
/// and the list of specifiers it imports.
pub fn rewrite_module(path: &Path, js: &str) -> Result<(String, Vec<String>)> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, js, SourceType::mjs()).parse();
    if parsed.diagnostics.has_errors() {
        let msgs: Vec<String> = parsed.diagnostics.iter().map(|e| e.to_string()).collect();
        bail!("{}: internal parse errors after transform:\n{}", path.display(), msgs.join("\n"));
    }
    let mut rewrites: Vec<Rewrite> = vec![];
    let mut imports: Vec<String> = vec![];
    let mut exports_tail: Vec<String> = vec![];
    let mut counter = 0usize;
    for stmt in &parsed.program.body {
        match stmt {
            Statement::ImportDeclaration(decl) => {
                let spec = decl.source.value.to_string();
                imports.push(spec.clone());
                counter += 1;
                let var = format!("__m{counter}");
                let mut lines = vec![format!("const {var} = __pocket_import({});", js_string(&spec))];
                if let Some(specs) = &decl.specifiers {
                    for s in specs {
                        match s {
                            ImportDeclarationSpecifier::ImportSpecifier(s) => {
                                lines.push(format!("const {} = {var}[{}];", s.local.name, js_string(&export_name(&s.imported))));
                            }
                            ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                                lines.push(format!("const {} = {var}.default;", s.local.name));
                            }
                            ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                                lines.push(format!("const {} = {var};", s.local.name));
                            }
                        }
                    }
                }
                rewrites.push(Rewrite { span: decl.span, replacement: lines.join(" ") });
            }
            Statement::ExportDeclaration(decl) => {
                // `export const x = ...` -> keep declaration, export at the end.
                let d = &decl.declaration;
                let mut names = vec![];
                match d {
                    Declaration::VariableDeclaration(v) => {
                        for dd in &v.declarations {
                            binding_names(&dd.id, &mut names);
                        }
                    }
                    Declaration::FunctionDeclaration(f) => {
                        if let Some(id) = &f.id {
                            names.push(id.name.to_string());
                        }
                    }
                    Declaration::ClassDeclaration(c) => {
                        if let Some(id) = &c.id {
                            names.push(id.name.to_string());
                        }
                    }
                    _ => {}
                }
                for n in names {
                    exports_tail.push(format!("__exports.{n} = {n};"));
                }
                let d_span = d.span();
                rewrites.push(Rewrite { span: Span::new(decl.span.start, d_span.start), replacement: String::new() });
            }
            Statement::ExportFromDeclaration(decl) => {
                let spec = decl.source.value.to_string();
                imports.push(spec.clone());
                counter += 1;
                let var = format!("__m{counter}");
                let mut lines = vec![format!("const {var} = __pocket_import({});", js_string(&spec))];
                for s in &decl.specifiers {
                    lines.push(format!("__exports[{}] = {var}[{}];", js_string(&export_name(&s.exported)), js_string(&export_name(&s.local))));
                }
                rewrites.push(Rewrite { span: decl.span, replacement: lines.join(" ") });
            }
            Statement::ExportNamedDeclaration(decl) => {
                let mut lines = vec![];
                for s in &decl.specifiers {
                    lines.push(format!("__exports[{}] = {};", js_string(&export_name(&s.exported)), export_name(&s.local)));
                }
                rewrites.push(Rewrite { span: decl.span, replacement: lines.join(" ") });
            }
            Statement::ExportDefaultDeclaration(decl) => match &decl.declaration {
                ExportDefaultDeclarationKind::FunctionDeclaration(f) => {
                    let inner = f.span;
                    if let Some(id) = &f.id {
                        exports_tail.push(format!("__exports.default = {};", id.name));
                        rewrites.push(Rewrite { span: Span::new(decl.span.start, inner.start), replacement: String::new() });
                    } else {
                        rewrites.push(Rewrite { span: Span::new(decl.span.start, inner.start), replacement: "__exports.default = ".into() });
                    }
                }
                ExportDefaultDeclarationKind::ClassDeclaration(c) => {
                    let inner = c.span;
                    if let Some(id) = &c.id {
                        exports_tail.push(format!("__exports.default = {};", id.name));
                        rewrites.push(Rewrite { span: Span::new(decl.span.start, inner.start), replacement: String::new() });
                    } else {
                        rewrites.push(Rewrite { span: Span::new(decl.span.start, inner.start), replacement: "__exports.default = ".into() });
                    }
                }
                other => {
                    let inner = other.span();
                    rewrites.push(Rewrite { span: Span::new(decl.span.start, inner.start), replacement: "__exports.default = ".into() });
                }
            },
            Statement::ExportAllDeclaration(decl) => {
                let spec = decl.source.value.to_string();
                imports.push(spec.clone());
                counter += 1;
                let var = format!("__m{counter}");
                let repl = match &decl.exported {
                    Some(name) => format!("const {var} = __pocket_import({}); __exports[{}] = {var};", js_string(&spec), js_string(&export_name(name))),
                    None => format!("const {var} = __pocket_import({}); for (const k in {var}) if (k !== \"default\") __exports[k] = {var}[k];", js_string(&spec)),
                };
                rewrites.push(Rewrite { span: decl.span, replacement: repl });
            }
            _ => {}
        }
    }
    rewrites.sort_by_key(|r| std::cmp::Reverse(r.span.start));
    let mut out = js.to_string();
    for r in rewrites {
        let (s, e) = (r.span.start as usize, r.span.end as usize);
        // As many lines as what it replaces, so the body's lines stay the transformed file's.
        let breaks = out[s..e].matches('\n').count();
        out.replace_range(s..e, &(r.replacement + &"\n".repeat(breaks)));
    }
    if !exports_tail.is_empty() {
        out.push('\n');
        out.push_str(&exports_tail.join("\n"));
        out.push('\n');
    }
    Ok((out, imports))
}

fn js_string(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

fn resolve(from: &Path, spec: &str, sdk_dir: &Path) -> Result<PathBuf> {
    let candidate = if spec == "pocket" {
        sdk_dir.join("pocket.ts")
    } else if let Some(rest) = spec.strip_prefix("pocket/") {
        sdk_dir.join(format!("{rest}.ts"))
    } else if spec.starts_with('.') || spec.starts_with('/') {
        from.parent().unwrap_or(Path::new(".")).join(spec)
    } else {
        return resolve_package(from, spec);
    };
    probe(&candidate).ok_or_else(|| anyhow!("{}: cannot find module '{}' (tried {} with .ts, .tsx, .js, .mjs, .cjs, .json and /index.*)", from.display(), spec, candidate.display()))
}

/// The file a path names, as written or with a module extension or an index file.
fn probe(candidate: &Path) -> Option<PathBuf> {
    if candidate.is_file() {
        return std::fs::canonicalize(candidate).ok();
    }
    let name = candidate.file_name()?.to_string_lossy().into_owned();
    let with = |ext: &str| candidate.with_file_name(format!("{name}.{ext}"));
    let tries = [with("ts"), with("tsx"), with("js"), with("mjs"), with("cjs"), with("json"), candidate.join("index.ts"), candidate.join("index.tsx"), candidate.join("index.js"), candidate.join("index.mjs"), candidate.join("index.cjs")];
    tries.iter().find(|t| t.is_file()).and_then(|t| std::fs::canonicalize(t).ok())
}

/// Node's own modules: a game has none of them.
fn is_node_builtin(spec: &str) -> bool {
    const BUILTINS: &[&str] = &[
        "assert", "async_hooks", "buffer", "child_process", "cluster", "console", "constants", "crypto", "dgram", "diagnostics_channel", "dns", "domain", "events", "fs", "http", "http2", "https", "inspector", "module", "net", "os", "path", "perf_hooks", "process", "punycode", "querystring", "readline", "repl", "stream", "string_decoder", "sys", "timers", "tls", "trace_events", "tty", "url", "util", "v8", "vm", "wasi", "worker_threads", "zlib",
    ];
    spec.starts_with("node:") || BUILTINS.contains(&spec.split('/').next().unwrap_or(spec))
}

/// `@scope/name/sub` is package `@scope/name`, subpath `./sub`; `name` alone is subpath `.`.
fn split_package(spec: &str) -> (&str, String) {
    let parts = if spec.starts_with('@') { 2 } else { 1 };
    let end = spec.match_indices('/').nth(parts - 1).map(|(i, _)| i).unwrap_or(spec.len());
    let (name, rest) = spec.split_at(end);
    (name, if rest.is_empty() { ".".to_string() } else { format!(".{rest}") })
}

/// A package.json's `exports`, keeping its key order (conditions are tried in order).
#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum Exports {
    Path(String),
    List(Vec<Exports>),
    Map(IndexMap<String, Exports>),
    Other(serde::de::IgnoredAny),
}

#[derive(Deserialize, Debug, Default)]
struct PackageJson {
    #[serde(default)]
    exports: Option<Exports>,
    #[serde(default)]
    module: Option<String>,
    #[serde(default)]
    browser: Option<serde_json::Value>,
    #[serde(default)]
    main: Option<String>,
}

/// The conditions a game resolves under: it imports, runs like a browser, and can wrap CommonJS.
const CONDITIONS: &[&str] = &["import", "module", "browser", "default", "require"];

fn pick_target(v: &Exports) -> Option<String> {
    match v {
        Exports::Path(s) => Some(s.clone()),
        Exports::List(l) => l.iter().find_map(pick_target),
        Exports::Map(m) => m.iter().filter(|(k, _)| CONDITIONS.contains(&k.as_str())).find_map(|(_, v)| pick_target(v)),
        Exports::Other(_) => None,
    }
}

fn exports_target(exports: &Exports, subpath: &str) -> Option<String> {
    match exports {
        Exports::Map(m) if m.keys().next().is_some_and(|k| k.starts_with('.')) => {
            if let Some(v) = m.get(subpath) {
                return pick_target(v);
            }
            // A pattern ("./*", "./features/*.js"): the one with the longest prefix that fits.
            let mut best: Option<(usize, &Exports, &str)> = None;
            for (k, v) in m {
                let Some(star) = k.find('*') else { continue };
                let (pre, post) = (&k[..star], &k[star + 1..]);
                if subpath.len() >= pre.len() + post.len() && subpath.starts_with(pre) && subpath.ends_with(post) && best.is_none_or(|(len, _, _)| pre.len() > len) {
                    best = Some((pre.len(), v, &subpath[pre.len()..subpath.len() - post.len()]));
                }
            }
            best.and_then(|(_, v, mid)| pick_target(v).map(|t| t.replace('*', mid)))
        }
        other if subpath == "." => pick_target(other),
        _ => None,
    }
}

/// A bare specifier: a package in the nearest `node_modules` that has it.
fn resolve_package(from: &Path, spec: &str) -> Result<PathBuf> {
    let (name, subpath) = split_package(spec);
    let mut dir = from.parent();
    let mut package = None;
    while let Some(d) = dir {
        let p = d.join("node_modules").join(name);
        if !spec.starts_with("node:") && p.join("package.json").is_file() {
            package = Some(p);
            break;
        }
        dir = d.parent();
    }
    let Some(package) = package else {
        if is_node_builtin(spec) {
            bail!("{}: '{}' is part of Node, which a game does not run on (no file system or process): the SDK has saves (save.*) and assets, or use a package made for browsers", from.display(), spec);
        }
        bail!("{}: cannot find package '{}' in a node_modules beside the project or above it (npm install {} in the project directory)", from.display(), spec, name);
    };
    let text = std::fs::read_to_string(package.join("package.json"))?;
    let pj: PackageJson = serde_json::from_str(&text).with_context(|| format!("{}", package.join("package.json").display()))?;
    let target = if let Some(exports) = &pj.exports {
        exports_target(exports, &subpath).ok_or_else(|| anyhow!("{}: package '{}' does not export '{}' (its package.json `exports`)", from.display(), name, subpath))?
    } else if subpath == "." {
        let browser = pj.browser.as_ref().and_then(|b| b.as_str()).map(str::to_string);
        pj.module.clone().or(browser).or(pj.main.clone()).unwrap_or_else(|| "index.js".into())
    } else {
        subpath.clone()
    };
    probe(&package.join(&target)).ok_or_else(|| anyhow!("{}: package '{}' names '{}', which is not there", from.display(), name, target))
}

/// Whether the package a file is in turns a specifier off for browsers (`"browser": {"crypto":
/// false}` in its package.json): the module is then an empty object.
fn browser_disabled(from: &Path, spec: &str) -> bool {
    let mut dir = from.parent();
    while let Some(d) = dir {
        let pj = d.join("package.json");
        if pj.is_file() {
            let browser = std::fs::read_to_string(&pj).ok().and_then(|t| serde_json::from_str::<PackageJson>(&t).ok()).and_then(|p| p.browser);
            return browser.as_ref().and_then(|b| b.get(spec)).is_some_and(|v| v == &serde_json::Value::Bool(false));
        }
        if d.file_name().is_some_and(|n| n == "node_modules") {
            return false;
        }
        dir = d.parent();
    }
    false
}

/// The `require("...")` calls with a string literal in a CommonJS file.
struct Requires(Vec<String>);

impl<'a> Visit<'a> for Requires {
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if let Expression::Identifier(id) = &it.callee {
            if id.name == "require" && it.arguments.len() == 1 {
                if let Some(Argument::StringLiteral(s)) = it.arguments.first() {
                    self.0.push(s.value.to_string());
                }
            }
        }
        walk::walk_call_expression(self, it);
    }
}

/// How a file becomes a module: the project's TypeScript and a package's ES modules are
/// rewritten, a package's CommonJS is wrapped, JSON is its value.
enum Module {
    // `lines`: the source line of each line of the body.
    Esm { body: String, imports: Vec<String>, lines: Vec<u32> },
    Cjs { source: String, requires: Vec<String> },
}

fn load_module(path: &Path) -> Result<Module> {
    let source = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    if ext == "json" {
        serde_json::from_str::<serde_json::Value>(&source).with_context(|| format!("{} is not JSON", path.display()))?;
        return Ok(Module::Cjs { source: format!("module.exports = {};", source.trim()), requires: vec![] });
    }
    let in_package = path.components().any(|c| c.as_os_str() == "node_modules");
    if !(in_package && matches!(ext.as_str(), "js" | "mjs" | "cjs")) {
        let (js, lines) = transform_to_js(path, &source)?;
        let (body, imports) = rewrite_module(path, &js)?;
        return Ok(Module::Esm { body, imports, lines });
    }
    // A package's JavaScript: an ES module when it says so (import or export, or .mjs), else CommonJS.
    if ext != "cjs" {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, &source, SourceType::mjs()).parse();
        let esm = !parsed.diagnostics.has_errors() && (ext == "mjs" || parsed.program.body.iter().any(|s| s.is_module_declaration()));
        if esm {
            let (body, imports) = rewrite_module(path, &source)?;
            return Ok(Module::Esm { body, imports, lines: same_lines(&source) });
        }
    }
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, &source, SourceType::cjs()).parse();
    if parsed.diagnostics.has_errors() {
        let msgs: Vec<String> = parsed.diagnostics.iter().map(|e| e.to_string()).collect();
        bail!("{}: parse errors:\n{}", path.display(), msgs.join("\n"));
    }
    let mut requires = Requires(vec![]);
    requires.visit_program(&parsed.program);
    requires.0.dedup();
    Ok(Module::Cjs { source, requires: requires.0 })
}

/// Bundle `entry` and everything it imports into `out`.
pub fn bundle(entry: &Path, sdk_dir: &Path, root: &Path, out: &Path) -> Result<BundleOutput> {
    let entry = std::fs::canonicalize(entry).with_context(|| format!("entry {}", entry.display()))?;
    // path -> the module's definition, the source line of each line of its body, and the line of
    // the definition its body starts on (0-based)
    let mut modules: IndexMap<PathBuf, (String, Vec<u32>, usize)> = IndexMap::new();
    let mut ids: IndexMap<PathBuf, String> = IndexMap::new();
    let mut cjs: Vec<String> = vec![];
    let mut queue = vec![entry.clone()];
    while let Some(path) = queue.pop() {
        if modules.contains_key(&path) {
            continue;
        }
        let id = module_id(&path, sdk_dir, root);
        let def = match load_module(&path)? {
            Module::Esm { body, imports, lines } => {
                // Resolve imports now so the body can refer to canonical ids.
                let mut body_final = body;
                for spec in &imports {
                    if browser_disabled(&path, spec) {
                        body_final = body_final.replace(&format!("__pocket_import({})", js_string(spec)), "__pocket_import(\"pocket:empty\")");
                        continue;
                    }
                    let target = resolve(&path, spec, sdk_dir)?;
                    let tid = module_id(&target, sdk_dir, root);
                    body_final = body_final.replace(&format!("__pocket_import({})", js_string(spec)), &format!("__pocket_import({})", js_string(&tid)));
                    queue.push(target);
                }
                (format!("function(__exports, __pocket_require){{\n\"use strict\";\n{body_final}\n}}"), lines, 2)
            }
            Module::Cjs { source, requires } => {
                // What it requires by name maps to bundled ids; what cannot be found throws when (if) it is required.
                let mut found = serde_json::Map::new();
                let mut missing = serde_json::Map::new();
                for spec in &requires {
                    if browser_disabled(&path, spec) {
                        found.insert(spec.clone(), "pocket:empty".into());
                        continue;
                    }
                    match resolve(&path, spec, sdk_dir) {
                        Ok(target) => {
                            found.insert(spec.clone(), module_id(&target, sdk_dir, root).into());
                            queue.push(target);
                        }
                        Err(e) => {
                            missing.insert(spec.clone(), e.to_string().into());
                        }
                    }
                }
                cjs.push(id.clone());
                let lines = same_lines(&source);
                let def = format!(
                    "function(e, r, m){{ (function(exports, require, module, process, global){{\n{source}\n}}).call(e, e, __pocket_cjs_require({}, {}), m, __pocket_process, globalThis); }}",
                    serde_json::Value::Object(found),
                    serde_json::Value::Object(missing)
                );
                (def, lines, 1)
            }
        };
        ids.insert(path.clone(), id);
        modules.insert(path, def);
    }
    let mut text = String::new();
    text.push_str("// generated by pocket ts; do not edit\n(function(){\nconst __defs = Object.create(null); const __cache = Object.create(null); const __ns = Object.create(null);\n");
    text.push_str("__defs[\"pocket:empty\"] = function(){};\n");
    text.push_str("function __pocket_require(id){ const c = __cache[id]; if (c) return c.exports; const d = __defs[id]; if (!d) throw new Error(\"module not found: \" + id); const m = { exports: {} }; __cache[id] = m; d(m.exports, __pocket_require, m); return m.exports; }\n");
    if !cjs.is_empty() {
        // An ES module importing CommonJS sees its properties by name and module.exports as the default.
        text.push_str(&format!("const __cjs = {};\n", serde_json::to_string(&cjs.iter().map(|id| (id.clone(), serde_json::Value::from(1))).collect::<serde_json::Map<_, _>>())?));
        text.push_str("function __pocket_import(id){ const e = __pocket_require(id); if (!__cjs[id] || (e && e.__esModule)) return e; let n = __ns[id]; if (!n) { n = Object.create(null); if (e !== null && (typeof e === \"object\" || typeof e === \"function\")) for (const k of Object.keys(e)) n[k] = e[k]; n.default = e; __ns[id] = n; } return n; }\n");
        text.push_str("function __pocket_cjs_require(found, missing){ return function require(s){ const id = found[s]; if (id !== undefined) return __pocket_require(id); throw new Error(missing[s] || (\"require('\" + s + \"'): only requires of a string literal are bundled\")); }; }\n");
        text.push_str("const __pocket_process = { env: { NODE_ENV: \"production\" }, platform: \"browser\", browser: true, nextTick: (f, ...a) => Promise.resolve().then(() => f(...a)) };\n");
    } else {
        text.push_str("const __pocket_import = __pocket_require;\n");
    }
    let mut names = vec![];
    // Where each module's lines are in the bundle, for the runtime to say where a script error or
    // a handler is in the files that were written (docs/sdk.md, Errors and cost).
    let mut map = vec![];
    let mut line = text.matches('\n').count() + 1;
    for (path, (def, lines, offset)) in &modules {
        let id = &ids[path];
        names.push(id.clone());
        map.push(serde_json::json!({"source": path.to_string_lossy(), "first": line + offset, "lines": lines}));
        let entry = format!("__defs[{}] = {};\n", js_string(id), def);
        line += entry.matches('\n').count();
        text.push_str(&entry);
    }
    text.push_str(&format!("__pocket_require({});\n}})();\n", js_string(&ids[&entry])));
    std::fs::write(out, text).with_context(|| format!("writing {}", out.display()))?;
    let lines_out = PathBuf::from(format!("{}.lines.json", out.display()));
    std::fs::write(&lines_out, serde_json::to_string(&serde_json::json!({"modules": map}))?).with_context(|| format!("writing {}", lines_out.display()))?;
    Ok(BundleOutput { modules: names })
}

fn module_id(path: &Path, sdk_dir: &Path, root: &Path) -> String {
    if let Ok(rel) = path.strip_prefix(sdk_dir) {
        return format!("pocket/{}", rel.with_extension("").to_string_lossy().replace('\\', "/"));
    }
    if let Ok(rel) = path.strip_prefix(root) {
        return rel.to_string_lossy().replace('\\', "/");
    }
    path.to_string_lossy().replace('\\', "/")
}

#[allow(dead_code)]
pub fn anyhow_wrap<T>(r: std::result::Result<T, String>) -> Result<T> {
    r.map_err(|e| anyhow!(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_names_and_subpaths() {
        assert_eq!(split_package("inkjs"), ("inkjs", ".".to_string()));
        assert_eq!(split_package("inkjs/full"), ("inkjs", "./full".to_string()));
        assert_eq!(split_package("@scope/pkg"), ("@scope/pkg", ".".to_string()));
        assert_eq!(split_package("@scope/pkg/a/b"), ("@scope/pkg", "./a/b".to_string()));
        assert!(is_node_builtin("fs") && is_node_builtin("node:fs") && is_node_builtin("fs/promises") && !is_node_builtin("fsm"));
    }

    #[test]
    fn exports_follow_conditions_in_order_and_patterns() {
        let e: Exports = serde_json::from_str(r#"{".": {"types": "./x.d.ts", "node": "./node.js", "import": "./esm.mjs", "default": "./cjs.js"}, "./engine/*": {"default": "./engine/*.js"}, "./engine/special": null}"#).unwrap();
        assert_eq!(exports_target(&e, ".").as_deref(), Some("./esm.mjs"));
        assert_eq!(exports_target(&e, "./engine/Story").as_deref(), Some("./engine/Story.js"));
        assert_eq!(exports_target(&e, "./engine/special"), None);
        assert_eq!(exports_target(&e, "./missing"), None);
        let s: Exports = serde_json::from_str(r#"{"require": "./c.js", "import": "./m.js"}"#).unwrap();
        assert_eq!(exports_target(&s, ".").as_deref(), Some("./c.js"));   // the first condition a game has wins
        let p: Exports = serde_json::from_str(r#""./main.js""#).unwrap();
        assert_eq!(exports_target(&p, ".").as_deref(), Some("./main.js"));
        assert_eq!(exports_target(&p, "./sub"), None);
    }

    #[test]
    fn bundles_es_modules_commonjs_and_json_from_node_modules() {
        let dir = std::env::temp_dir().join(format!("pocket-ts-packages-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap();   // module ids are relative to a canonical root
        let write = |rel: &str, text: &str| {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        write("game/main.ts", "import { add } from \"esm-pkg\";\nimport legacy from \"cjs-pkg\";\nimport { twice } from \"cjs-pkg\";\nexport const v = add(1, 2) + legacy.twice(3) + twice(4);\n");
        write("game/node_modules/esm-pkg/package.json", r#"{"exports": {".": {"import": "./esm/index.js", "require": "./cjs/index.js"}}}"#);
        write("game/node_modules/esm-pkg/esm/index.js", "export function add(a, b) { return a + b; }\n");
        write("game/node_modules/cjs-pkg/package.json", r#"{"main": "lib/index", "browser": {"crypto": false}}"#);
        write("game/node_modules/cjs-pkg/lib/index.js", "var data = require('./data.json');\nvar crypto = require('crypto');\nvar later = function () { return require('fs'); };\nexports.twice = function (x) { return x * data.factor; };\n");
        write("game/node_modules/cjs-pkg/lib/data.json", r#"{"factor": 2}"#);
        let out = dir.join("out.js");
        let sdk = dir.join("sdk");
        std::fs::create_dir_all(&sdk).unwrap();
        let b = bundle(&dir.join("game/main.ts"), &sdk, &dir, &out).unwrap();
        assert_eq!(b.modules.len(), 4, "{:?}", b.modules);
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.contains("__pocket_cjs_require({\"./data.json\":\"game/node_modules/cjs-pkg/lib/data.json\""), "{text}");
        assert!(text.contains("\"crypto\":\"pocket:empty\""));   // the package turned it off for browsers
        assert!(text.contains("'fs' is part of Node"));          // required only when called: a message, not a bundle error
        assert!(text.contains("module.exports = {\"factor\": 2};"));
        // A static import of Node's own module is refused when bundling.
        write("game/bad.ts", "import { readFileSync } from \"fs\";\nexport const x = readFileSync;\n");
        let err = bundle(&dir.join("game/bad.ts"), &sdk, &dir, &out).map(|_| ()).unwrap_err().to_string();
        assert!(err.contains("'fs' is part of Node"), "{err}");
        let err = bundle(&dir.join("game/missing.ts"), &sdk, &dir, &out).map(|_| ()).unwrap_err().to_string();
        assert!(err.contains("missing.ts"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
