//! The SDK as one line an export (docs/sdk.md, The API at a glance): every exported function and
//! every method of an exported object in sdk/runtime, with its signature as written and the first
//! sentence of its doc comment, and every exported interface with its fields. Written to
//! docs/generated/sdk.md for reading and to a table the runtime answers `help {sdk}` from, so an
//! agent learns what `timer.after` takes and returns without opening the SDK's sources.

use anyhow::{Context, Result};
use oxc::allocator::Allocator;
use oxc::ast::ast::{Declaration, Expression, ObjectPropertyKind, PropertyKind, Statement};
use oxc::parser::Parser;
use oxc::span::{GetSpan, SourceType};
use std::path::{Path, PathBuf};

pub struct Entry {
    pub name: String,
    pub signature: String,
    pub doc: String,
    pub file: String,
}

/// The SDK's source files, in name order (the generated components and the JSX plumbing left out).
pub fn sources(root: &Path) -> Result<Vec<PathBuf>> {
    let dir = root.join("sdk").join("runtime");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "ts") && p.file_stem().is_some_and(|s| s != "jsx-runtime"))
        .collect();
    files.sort();
    Ok(files)
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The first sentence of the doc comment that ends right before `start` (only blanks between).
fn doc_before(source: &str, comments: &[(u32, u32)], start: u32) -> String {
    let Some(&(a, b)) = comments.iter().rev().find(|&&(_, end)| end <= start) else { return String::new() };
    if !source[b as usize..start as usize].trim().is_empty() {
        return String::new();
    }
    let raw = &source[a as usize..b as usize];
    let text: String = if raw.starts_with("/*") {
        raw.trim_start_matches("/**").trim_start_matches("/*").trim_end_matches("*/").lines().map(|l| l.trim().trim_start_matches('*').trim()).collect::<Vec<_>>().join(" ")
    } else {
        raw.lines().map(|l| l.trim().trim_start_matches("//").trim()).collect::<Vec<_>>().join(" ")
    };
    let text = squash(&text);
    let end = text.find(". ").map(|i| i + 1).unwrap_or(text.len());
    let mut first = text[..end].to_string();
    if first.len() > 240 {
        let mut cut = 237;
        while !first.is_char_boundary(cut) {
            cut -= 1;
        }
        first.truncate(cut);
        first.push_str("...");
    }
    first
}

fn file_entries(path: &Path, module: &str) -> Result<Vec<Entry>> {
    let source = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, &source, SourceType::ts()).parse();
    if !parsed.diagnostics.is_empty() {
        anyhow::bail!("{}: {:?}", path.display(), parsed.diagnostics[0]);
    }
    let comments: Vec<(u32, u32)> = parsed.program.comments.iter().map(|c| (c.span.start, c.span.end)).collect();
    let text = |a: u32, b: u32| squash(&source[a as usize..b as usize]);
    let mut out = vec![];
    let file = format!("sdk/runtime/{module}.ts");
    for stmt in &parsed.program.body {
        let Statement::ExportDeclaration(export) = stmt else { continue };
        let decl = &export.declaration;
        let doc_at = export.span.start;
        match decl {
            Declaration::FunctionDeclaration(f) => {
                let (Some(id), Some(body)) = (&f.id, &f.body) else { continue };
                out.push(Entry { name: id.name.to_string(), signature: text(id.span.start, body.span.start), doc: doc_before(&source, &comments, doc_at), file: file.clone() });
            }
            Declaration::TSInterfaceDeclaration(i) => {
                // The fields, comments dropped: what an answer of that shape holds.
                let mut body = String::new();
                for sig in &i.body.body {
                    let s = sig.span();
                    body.push_str(&text(s.start, s.end));
                    if !body.ends_with(';') {
                        body.push(';');
                    }
                    body.push(' ');
                }
                let mut sig = format!("interface {} {{ {}}}", i.id.name, body);
                if sig.len() > 400 {
                    let mut cut = 397;
                    while !sig.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    sig.truncate(cut);
                    sig.push_str("...}");
                }
                out.push(Entry { name: i.id.name.to_string(), signature: sig, doc: doc_before(&source, &comments, doc_at), file: file.clone() });
            }
            Declaration::VariableDeclaration(v) => {
                for d in &v.declarations {
                    let Some(name) = d.id.get_identifier_name() else { continue };
                    let doc = doc_before(&source, &comments, doc_at);
                    match &d.init {
                        Some(Expression::ObjectExpression(o)) => object_entries(&source, &comments, name.as_str(), o, &file, &mut out),
                        Some(Expression::ArrowFunctionExpression(a)) => {
                            out.push(Entry { name: name.to_string(), signature: format!("{name}{}", text(a.span.start, a.body.span().start).trim_end_matches("=>").trim_end()), doc, file: file.clone() });
                        }
                        Some(Expression::FunctionExpression(f)) => {
                            if let Some(body) = &f.body {
                                out.push(Entry { name: name.to_string(), signature: format!("{name}{}", text(f.params.span.start, body.span.start).trim_start_matches("function")), doc, file: file.clone() });
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// The methods (and nested objects' methods) of an exported object, as `object.method(...)`.
fn object_entries(source: &str, comments: &[(u32, u32)], prefix: &str, o: &oxc::ast::ast::ObjectExpression, file: &str, out: &mut Vec<Entry>) {
    let text = |a: u32, b: u32| squash(&source[a as usize..b as usize]);
    for prop in &o.properties {
        let ObjectPropertyKind::ObjectProperty(p) = prop else { continue };
        let Some(key) = p.key.static_name() else { continue };
        let name = format!("{prefix}.{key}");
        let doc = doc_before(source, comments, p.span.start);
        match &p.value {
            Expression::FunctionExpression(f) => {
                let Some(body) = &f.body else { continue };
                let head = if p.method { text(p.key.span().end, body.span.start) } else { text(f.params.span.start, body.span.start) };
                let kind = match p.kind {
                    PropertyKind::Get => "get ",
                    PropertyKind::Set => "set ",
                    PropertyKind::Init => "",
                };
                out.push(Entry { name: name.clone(), signature: format!("{kind}{name}{head}"), doc, file: file.to_string() });
            }
            Expression::ArrowFunctionExpression(a) => {
                let head = text(a.span.start, a.body.span().start);
                out.push(Entry { name: name.clone(), signature: format!("{name}{}", head.trim_end_matches("=>").trim_end()), doc, file: file.to_string() });
            }
            Expression::ObjectExpression(inner) => object_entries(source, comments, &name, inner, file, out),
            _ => {}
        }
    }
}

pub fn entries(root: &Path) -> Result<Vec<Entry>> {
    let mut all = vec![];
    for path in sources(root)? {
        let module = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        all.extend(file_entries(&path, &module)?);
    }
    Ok(all)
}

pub fn markdown(entries: &[Entry]) -> String {
    let mut o = String::from("# The SDK at a glance\n\nGenerated by `pocket gen` from `sdk/runtime/*.ts`; do not edit. One line an export: its signature as written and the first sentence of its documentation; interfaces with their fields, which is what a function answering that type returns. Every name here is imported from `\"pocket\"`. `help {\"sdk\": \"timer.after\"}` answers the same from a running game; a part's design document says how it behaves.\n");
    let mut file = "";
    for e in entries {
        if e.file != file {
            file = &e.file;
            o.push_str(&format!("\n## `{file}`\n\n"));
        }
        if e.doc.is_empty() {
            o.push_str(&format!("- `{}`\n", e.signature));
        } else {
            o.push_str(&format!("- `{}`: {}\n", e.signature, e.doc));
        }
    }
    o
}

fn cpp_string(s: &str) -> String {
    // A raw literal unless the text could close one.
    if s.contains(")sdk\"") {
        format!("{:?}", s)
    } else {
        format!("R\"sdk({s})sdk\"")
    }
}

pub fn cpp(entries: &[Entry]) -> String {
    let mut o = String::from("// generated by `pocket gen` from sdk/runtime/*.ts (tools/pocket/src/sdkdoc.rs); do not edit\n#include \"../src/command_help.hpp\"\n\nnamespace pocket::app {\n\nnamespace {\nconstexpr SdkHelp kSdkHelps[] = {\n");
    for e in entries {
        o.push_str(&format!("    {{{}, {}, {}, {}}},\n", cpp_string(&e.name), cpp_string(&e.signature), cpp_string(&e.doc), cpp_string(&e.file)));
    }
    o.push_str("};\n}  // namespace\n\nstd::span<const SdkHelp> sdk_helps() { return kSdkHelps; }\n\n}  // namespace pocket::app\n");
    o
}
