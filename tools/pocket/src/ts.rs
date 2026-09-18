//! TypeScript transform and bundling.
//!
//! Every module is parsed and transformed with oxc (types stripped, TSX lowered), then its
//! import and export statements are rewritten by span into a tiny CommonJS-style module
//! registry so that the engine can evaluate one script. Live bindings are not preserved;
//! gameplay code does not need them. Bare specifiers `pocket` and `pocket/<name>` resolve
//! to the SDK runtime directory.

use anyhow::{anyhow, bail, Context, Result};
use indexmap::IndexMap;
use oxc::allocator::Allocator;
use oxc::ast::ast::{
    BindingPattern, Declaration, ExportDefaultDeclarationKind, ImportDeclarationSpecifier, ModuleExportName, Statement,
};
use oxc::codegen::Codegen;
use oxc::parser::Parser;
use oxc::semantic::SemanticBuilder;
use oxc::span::{GetSpan, SourceType, Span};
use oxc::transformer::{TransformOptions, Transformer};
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

/// Transform one TypeScript/TSX file to plain JavaScript (ES module syntax kept).
pub fn transform_to_js(path: &Path, source: &str) -> Result<String> {
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(path).unwrap_or_else(|_| SourceType::ts());
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if parsed.diagnostics.has_errors() {
        let msgs: Vec<String> = parsed.diagnostics.iter().map(|e| e.to_string()).collect();
        bail!("{}: parse errors:\n{}", path.display(), msgs.join("\n"));
    }
    let mut program = parsed.program;
    let scoping = SemanticBuilder::new().build(&program).semantic.into_scoping();
    let options = TransformOptions::default();
    let ret = Transformer::new(&allocator, path, &options).build_with_scoping(scoping, &mut program);
    if ret.diagnostics.has_errors() {
        let msgs: Vec<String> = ret.diagnostics.iter().map(|e| e.to_string()).collect();
        bail!("{}: transform errors:\n{}", path.display(), msgs.join("\n"));
    }
    Ok(Codegen::new().build(&program).code)
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
                let mut lines = vec![format!("const {var} = __pocket_require({});", js_string(&spec))];
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
                let mut lines = vec![format!("const {var} = __pocket_require({});", js_string(&spec))];
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
                    Some(name) => format!("const {var} = __pocket_require({}); __exports[{}] = {var};", js_string(&spec), js_string(&export_name(name))),
                    None => format!("const {var} = __pocket_require({}); Object.assign(__exports, {var});", js_string(&spec)),
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
        out.replace_range(s..e, &r.replacement);
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
        bail!("{}: cannot resolve bare import '{}' (only relative paths and pocket/* are supported)", from.display(), spec);
    };
    let tries = [candidate.clone(), candidate.with_extension("ts"), candidate.with_extension("tsx"), candidate.with_extension("js"), candidate.join("index.ts")];
    for t in &tries {
        if t.is_file() {
            return Ok(std::fs::canonicalize(t)?);
        }
    }
    let ext = candidate.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    if ["ts", "tsx", "js"].contains(&ext.as_str()) && candidate.is_file() {
        return Ok(std::fs::canonicalize(&candidate)?);
    }
    bail!("{}: cannot find module '{}' (tried {})", from.display(), spec, tries.iter().map(|t| t.display().to_string()).collect::<Vec<_>>().join(", "))
}

/// Bundle `entry` and everything it imports into `out`.
pub fn bundle(entry: &Path, sdk_dir: &Path, root: &Path, out: &Path) -> Result<BundleOutput> {
    let entry = std::fs::canonicalize(entry).with_context(|| format!("entry {}", entry.display()))?;
    let mut modules: IndexMap<PathBuf, String> = IndexMap::new(); // path -> rewritten body
    let mut ids: IndexMap<PathBuf, String> = IndexMap::new();
    let mut queue = vec![entry.clone()];
    while let Some(path) = queue.pop() {
        if modules.contains_key(&path) {
            continue;
        }
        let source = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let js = transform_to_js(&path, &source)?;
        let (body, imports) = rewrite_module(&path, &js)?;
        // Resolve imports now so the body can refer to canonical ids.
        let mut body_final = body;
        for spec in &imports {
            let target = resolve(&path, spec, sdk_dir)?;
            let id = module_id(&target, sdk_dir, root);
            body_final = body_final.replace(&format!("__pocket_require({})", js_string(spec)), &format!("__pocket_require({})", js_string(&id)));
            queue.push(target);
        }
        ids.insert(path.clone(), module_id(&path, sdk_dir, root));
        modules.insert(path, body_final);
    }
    let mut text = String::new();
    text.push_str("// generated by pocket ts; do not edit\n(function(){\n\"use strict\";\nconst __defs = Object.create(null); const __cache = Object.create(null);\n");
    text.push_str("function __pocket_require(id){ const c = __cache[id]; if (c) return c.exports; const d = __defs[id]; if (!d) throw new Error(\"module not found: \" + id); const m = { exports: {} }; __cache[id] = m; d(m.exports, __pocket_require); return m.exports; }\n");
    let mut names = vec![];
    for (path, body) in &modules {
        let id = &ids[path];
        names.push(id.clone());
        text.push_str(&format!("__defs[{}] = function(__exports, __pocket_require){{\n{}\n}};\n", js_string(id), body));
    }
    text.push_str(&format!("__pocket_require({});\n}})();\n", js_string(&ids[&entry])));
    std::fs::write(out, text).with_context(|| format!("writing {}", out.display()))?;
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
