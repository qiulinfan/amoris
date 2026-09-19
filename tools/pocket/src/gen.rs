//! Code generation from component metadata (`engine/*/meta/*.toml`).
//!
//! One metadata file feeds every derived artifact: C++ structs with JSON conversion and a
//! reflection table, TypeScript interfaces with defaults, and Markdown docs. Files are rewritten
//! only when their content changes so incremental builds stay incremental.
//!
//! Besides components, a metadata file may declare records: plain value types that components
//! hold in `list:<Record>` fields (a JSON array, `std::vector` in C++, `Record[]` in TypeScript).
//! A record's fields are scalar or vector types; records do not nest.

use crate::manifest::Workspace;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct MetaFile {
    #[serde(default)]
    pub record: Vec<Record>,
    #[serde(default)]
    pub component: Vec<Component>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub name: String,
    #[serde(default)]
    pub doc: String,
    #[serde(default)]
    pub fields: Vec<Field>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Component {
    pub name: String,
    #[serde(default)]
    pub doc: String,
    #[serde(default = "default_true")]
    pub serialized: bool,
    #[serde(default)]
    pub fields: Vec<Field>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub default: Option<toml::Value>,
    #[serde(default)]
    pub doc: String,
}

fn default_true() -> bool {
    true
}

struct TypeInfo {
    cpp: String,
    ts: String,
    components: &'static [&'static str], // sub-fields for vector types
    list: Option<String>,                 // the record name of a `list:<Record>` field
}

fn type_info(ty: &str, records: &[Record]) -> Result<TypeInfo> {
    if let Some(rec) = ty.strip_prefix("list:") {
        if !records.iter().any(|r| r.name == rec) {
            bail!("unknown record '{rec}' in field type '{ty}'");
        }
        return Ok(TypeInfo { cpp: format!("std::vector<{rec}>"), ts: format!("{rec}[]"), components: &[], list: Some(rec.to_string()) });
    }
    let (cpp, ts, components): (&str, &str, &'static [&'static str]) = match ty {
        "f32" => ("float", "number", &[]),
        "f64" => ("double", "number", &[]),
        "i32" => ("std::int32_t", "number", &[]),
        "u32" => ("std::uint32_t", "number", &[]),
        "i64" => ("std::int64_t", "number", &[]),
        "bool" => ("bool", "boolean", &[]),
        "entity" => ("std::uint64_t", "number", &[]),
        "vec2" => ("Vec2", "Vec2", &["x", "y"]),
        "vec3" => ("Vec3", "Vec3", &["x", "y", "z"]),
        "vec4" => ("Vec4", "Vec4", &["x", "y", "z", "w"]),
        "quat" => ("Quat", "Quat", &["x", "y", "z", "w"]),
        "color" => ("Color4", "Color", &["r", "g", "b", "a"]),
        "string" => ("std::string", "string", &[]),
        other => bail!("unknown field type '{other}'"),
    };
    Ok(TypeInfo { cpp: cpp.into(), ts: ts.into(), components, list: None })
}

fn toml_num(v: &toml::Value) -> Result<f64> {
    match v {
        toml::Value::Float(f) => Ok(*f),
        toml::Value::Integer(i) => Ok(*i as f64),
        toml::Value::Boolean(b) => Ok(if *b { 1.0 } else { 0.0 }),
        other => bail!("expected a number, got {other}"),
    }
}

fn fmt_cpp_num(v: f64, ty: &str) -> String {
    match ty {
        "f32" | "vec2" | "vec3" | "vec4" | "quat" | "color" => {
            if v.fract() == 0.0 && v.abs() < 1e15 { format!("{v:.1}f") } else { format!("{v}f") }
        }
        "f64" => { if v.fract() == 0.0 { format!("{v:.1}") } else { format!("{v}") } }
        _ => format!("{}", v as i64),
    }
}

fn fmt_ts_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 { format!("{}", v as i64) } else { format!("{v}") }
}

fn cpp_default(f: &Field, records: &[Record]) -> Result<String> {
    let info = type_info(&f.ty, records)?;
    if info.list.is_some() {
        if f.default.is_some() {
            bail!("{} is a list and starts empty; it cannot have a default", f.name);
        }
        return Ok("{}".into());
    }
    let Some(d) = &f.default else {
        return Ok("{}".into());
    };
    if f.ty == "bool" {
        return Ok(match d { toml::Value::Boolean(b) => b.to_string(), other => bail!("bool default for {} must be true/false, got {other}", f.name) });
    }
    if f.ty == "string" {
        return Ok(match d { toml::Value::String(s) => format!("\"{}\"", cpp_escape(s)), other => bail!("string default for {} must be a string, got {other}", f.name) });
    }
    if info.components.is_empty() {
        return Ok(fmt_cpp_num(toml_num(d)?, &f.ty));
    }
    let arr = d.as_array().with_context(|| format!("{} default must be an array", f.name))?;
    if arr.len() != info.components.len() {
        bail!("{} default needs {} values", f.name, info.components.len());
    }
    let parts: Result<Vec<String>> = arr.iter().map(|x| Ok(fmt_cpp_num(toml_num(x)?, &f.ty))).collect();
    Ok(format!("{{{}}}", parts?.join(", ")))
}

fn ts_default(f: &Field, records: &[Record]) -> Result<String> {
    let info = type_info(&f.ty, records)?;
    if info.list.is_some() {
        return Ok("[]".into());
    }
    let Some(d) = &f.default else {
        if f.ty == "string" { return Ok("\"\"".into()); }
        return Ok(if info.components.is_empty() { "0".into() } else { format!("{{ {} }}", info.components.iter().map(|c| format!("{c}: 0")).collect::<Vec<_>>().join(", ")) });
    };
    if f.ty == "bool" {
        return Ok(d.as_bool().unwrap_or(false).to_string());
    }
    if f.ty == "string" {
        return Ok(serde_json::to_string(d.as_str().unwrap_or("")).unwrap_or_else(|_| "\"\"".into()));
    }
    if info.components.is_empty() {
        return Ok(fmt_ts_num(toml_num(d)?));
    }
    let arr = d.as_array().with_context(|| format!("{} default must be an array", f.name))?;
    let parts: Result<Vec<String>> = arr.iter().zip(info.components.iter()).map(|(x, c)| Ok(format!("{c}: {}", fmt_ts_num(toml_num(x)?)))).collect();
    Ok(format!("{{ {} }}", parts?.join(", ")))
}

fn default_text(f: &Field) -> String {
    if f.ty.starts_with("list:") {
        return "[]".into();
    }
    match &f.default {
        None => "-".into(),
        Some(toml::Value::Array(a)) => format!("[{}]", a.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", ")),
        Some(v) => v.to_string(),
    }
}

pub fn load_meta(ws: &Workspace) -> Result<(Vec<PathBuf>, Vec<Record>, Vec<Component>)> {
    let mut files = vec![];
    let mut parsed: Vec<(PathBuf, MetaFile)> = vec![];
    for m in ws.modules.values() {
        let meta_dir = m.dir.join("meta");
        if !meta_dir.is_dir() {
            continue;
        }
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&meta_dir)?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().map(|e| e == "toml").unwrap_or(false)).collect();
        entries.sort();
        for p in entries {
            let text = std::fs::read_to_string(&p)?;
            let mf: MetaFile = toml::from_str(&text).with_context(|| format!("parsing {}", p.display()))?;
            files.push(p.clone());
            parsed.push((p, mf));
        }
    }
    // Records first: a list field in any file may name a record from any file.
    let mut records: Vec<Record> = vec![];
    for (p, mf) in &parsed {
        for r in &mf.record {
            if records.iter().any(|x| x.name == r.name) {
                bail!("duplicate record {} in {}", r.name, p.display());
            }
            for f in &r.fields {
                type_info(&f.ty, &[]).with_context(|| format!("record {}.{} in {} (records hold scalar and vector fields only)", r.name, f.name, p.display()))?;
            }
            records.push(r.clone());
        }
    }
    let mut comps: Vec<Component> = vec![];
    for (p, mf) in &parsed {
        for c in &mf.component {
            if comps.iter().any(|x| x.name == c.name) {
                bail!("duplicate component {} in {}", c.name, p.display());
            }
            if records.iter().any(|r| r.name == c.name) {
                bail!("{} in {} is both a record and a component", c.name, p.display());
            }
            for f in &c.fields {
                type_info(&f.ty, &records).with_context(|| format!("{}.{} in {}", c.name, f.name, p.display()))?;
            }
            comps.push(c.clone());
        }
    }
    Ok((files, records, comps))
}

fn cpp_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn emit_struct_hpp(o: &mut String, name: &str, doc: &str, fields: &[Field], records: &[Record]) -> Result<()> {
    o.push_str(&format!("/// {}\nstruct {} {{\n", doc, name));
    for f in fields {
        let info = type_info(&f.ty, records)?;
        o.push_str(&format!("    {} {}{};\n", info.cpp, f.name, if info.components.is_empty() { format!(" = {}", cpp_default(f, records)?) } else { cpp_default(f, records)? }));
    }
    o.push_str(&format!("    constexpr bool operator==(const {}&) const = default;\n}};\n", name));
    o.push_str(&format!("void to_json(Json& j, const {0}& v);\nvoid from_json(const Json& j, {0}& v);\n// Floats behind a numeric field path (\"position\", \"position.x\", \"color\", \"layers.0.weight\"); 0 when the path is not numeric.\nstd::size_t numeric_span({0}& v, std::string_view path, float** out);\n", name));
    Ok(())
}

fn gen_hpp(records: &[Record], comps: &[Component]) -> Result<String> {
    let mut o = String::new();
    o.push_str("// generated by `pocket gen` from engine/*/meta/*.toml; do not edit\n#pragma once\n\n#include <pocket/core/json.hpp>\n#include <pocket/core/math.hpp>\n#include <pocket/core/result.hpp>\n\n#include <cstdint>\n#include <span>\n#include <string>\n#include <string_view>\n#include <vector>\n\nnamespace pocket::world {\n\n");
    o.push_str("struct Color4 {\n    float r = 1, g = 1, b = 1, a = 1;\n    constexpr bool operator==(const Color4&) const = default;\n};\n\n");
    if !records.is_empty() {
        o.push_str("// Records: the values held by list fields of components.\n");
        for r in records {
            emit_struct_hpp(&mut o, &r.name, &r.doc, &r.fields, &[])?;
            o.push_str(&format!("void hash_record(struct StateHasherRef& h, const {}& v);\n\n", r.name));
        }
    }
    for c in comps {
        emit_struct_hpp(&mut o, &c.name, &c.doc, &c.fields, records)?;
        o.push('\n');
    }
    o.push_str("struct FieldInfo {\n    std::string_view name;\n    std::string_view type;\n    std::string_view doc;\n};\n\nstruct ComponentInfo {\n    std::string_view name;\n    std::string_view doc;\n    bool serialized;\n    std::span<const FieldInfo> fields;\n};\n\n");
    o.push_str("/// Every component known to the engine, in metadata order.\nstd::span<const ComponentInfo> component_infos();\n\n");
    o.push_str("/// Hash a component value into a state hasher (all fields, in metadata order).\n");
    for c in comps {
        o.push_str(&format!("void hash_component(struct StateHasherRef& h, const {}& v);\n", c.name));
    }
    o.push_str("\n}  // namespace pocket::world\n");
    Ok(o)
}

fn emit_struct_cpp(o: &mut String, name: &str, fields: &[Field], records: &[Record], hash_fn: &str) -> Result<()> {
    o.push_str(&format!("void to_json(Json& j, const {}& v) {{\n    j = Json::object();\n", name));
    for f in fields {
        let info = type_info(&f.ty, records)?;
        if info.list.is_some() {
            o.push_str(&format!("    j[\"{0}\"] = Json::array();\n    for (const auto& x : v.{0}) {{ Json e; to_json(e, x); j[\"{0}\"].push_back(std::move(e)); }}\n", f.name));
        } else if info.components.is_empty() {
            o.push_str(&format!("    j[\"{0}\"] = v.{0};\n", f.name));
        } else {
            o.push_str(&format!("    vec_to_json(j[\"{0}\"], v.{0});\n", f.name));
        }
    }
    o.push_str("}\n\n");
    o.push_str(&format!("void from_json(const Json& j, {}& v) {{\n", name));
    for f in fields {
        let info = type_info(&f.ty, records)?;
        if let Some(rec) = &info.list {
            o.push_str(&format!("    if (j.is_object() && j.contains(\"{0}\") && j[\"{0}\"].is_array()) {{\n        v.{0}.clear();\n        for (const Json& e : j[\"{0}\"]) {{ {1} x; from_json(e, x); v.{0}.push_back(std::move(x)); }}\n    }}\n", f.name, rec));
        } else if info.components.is_empty() {
            o.push_str(&format!("    scalar_from_json(j, \"{0}\", v.{0});\n", f.name));
        } else {
            o.push_str(&format!("    if (j.is_object() && j.contains(\"{0}\")) vec_from_json(j[\"{0}\"], v.{0});\n", f.name));
        }
    }
    o.push_str("}\n\n");
    o.push_str(&format!("void {}(StateHasherRef& h, const {}& v) {{\n", hash_fn, name));
    for f in fields {
        let info = type_info(&f.ty, records)?;
        match f.ty.as_str() {
            "f32" => o.push_str(&format!("    h.f32(v.{});\n", f.name)),
            "f64" => o.push_str(&format!("    h.f64(v.{});\n", f.name)),
            "i32" | "u32" | "i64" | "entity" => o.push_str(&format!("    h.i64(static_cast<std::int64_t>(v.{}));\n", f.name)),
            "bool" => o.push_str(&format!("    h.u8(v.{} ? 1 : 0);\n", f.name)),
            "string" => o.push_str(&format!("    h.str(v.{});\n", f.name)),
            _ if info.list.is_some() => o.push_str(&format!("    h.i64(static_cast<std::int64_t>(v.{0}.size()));\n    for (const auto& x : v.{0}) hash_record(h, x);\n", f.name)),
            _ => {
                for comp in info.components {
                    o.push_str(&format!("    h.f32(v.{}.{});\n", f.name, comp));
                }
            }
        }
    }
    o.push_str("}\n\n");
    // numeric_span: typed-array packing addresses float fields by path without JSON.
    o.push_str(&format!("std::size_t numeric_span({}& v, std::string_view path, float** out) {{\n    (void)v;\n", name));
    for f in fields {
        let info = type_info(&f.ty, records)?;
        if f.ty == "f32" {
            o.push_str(&format!("    if (path == \"{0}\") {{ *out = &v.{0}; return 1; }}\n", f.name));
        } else if info.list.is_some() {
            o.push_str(&format!("    if (path.starts_with(\"{0}.\")) {{\n        std::string_view rest = path.substr({1});\n        std::size_t index = 0;\n        if (list_index(rest, index) && index < v.{0}.size()) return numeric_span(v.{0}[index], rest, out);\n    }}\n", f.name, f.name.len() + 1));
        } else if !info.components.is_empty() {
            o.push_str(&format!("    if (path == \"{0}\") {{ *out = &v.{0}.{1}; return {2}; }}\n", f.name, info.components[0], info.components.len()));
            for comp in info.components {
                o.push_str(&format!("    if (path == \"{0}.{1}\") {{ *out = &v.{0}.{1}; return 1; }}\n", f.name, comp));
            }
        }
    }
    o.push_str("    return 0;\n}\n\n");
    Ok(())
}

fn gen_cpp(records: &[Record], comps: &[Component]) -> Result<String> {
    let mut o = String::new();
    o.push_str("// generated by `pocket gen` from engine/*/meta/*.toml; do not edit\n#include <pocket/world/components.gen.hpp>\n#include <pocket/world/hashing.hpp>\n\n#include <array>\n\nnamespace pocket::world {\n\nnamespace {\n\n");
    o.push_str("[[maybe_unused]] void vec_to_json(Json& j, const Vec2& v) { j = Json{{\"x\", v.x}, {\"y\", v.y}}; }\n[[maybe_unused]] void vec_to_json(Json& j, const Vec3& v) { j = Json{{\"x\", v.x}, {\"y\", v.y}, {\"z\", v.z}}; }\n[[maybe_unused]] void vec_to_json(Json& j, const Vec4& v) { j = Json{{\"x\", v.x}, {\"y\", v.y}, {\"z\", v.z}, {\"w\", v.w}}; }\n[[maybe_unused]] void vec_to_json(Json& j, const Quat& v) { j = Json{{\"x\", v.x}, {\"y\", v.y}, {\"z\", v.z}, {\"w\", v.w}}; }\n[[maybe_unused]] void vec_to_json(Json& j, const Color4& v) { j = Json{{\"r\", v.r}, {\"g\", v.g}, {\"b\", v.b}, {\"a\", v.a}}; }\n\n");
    o.push_str("float num(const Json& j, const char* key, float fallback) {\n    if (!j.is_object() || !j.contains(key) || !j[key].is_number()) return fallback;\n    return j[key].get<float>();\n}\n");
    o.push_str("float idx(const Json& j, std::size_t i, float fallback) {\n    if (!j.is_array() || j.size() <= i || !j[i].is_number()) return fallback;\n    return j[i].get<float>();\n}\n\n");
    o.push_str("[[maybe_unused]] void vec_from_json(const Json& j, Vec2& v) { v = {num(j, \"x\", idx(j, 0, v.x)), num(j, \"y\", idx(j, 1, v.y))}; }\n[[maybe_unused]] void vec_from_json(const Json& j, Vec3& v) { v = {num(j, \"x\", idx(j, 0, v.x)), num(j, \"y\", idx(j, 1, v.y)), num(j, \"z\", idx(j, 2, v.z))}; }\n[[maybe_unused]] void vec_from_json(const Json& j, Vec4& v) { v = {num(j, \"x\", idx(j, 0, v.x)), num(j, \"y\", idx(j, 1, v.y)), num(j, \"z\", idx(j, 2, v.z)), num(j, \"w\", idx(j, 3, v.w))}; }\n[[maybe_unused]] void vec_from_json(const Json& j, Quat& v) { v = {num(j, \"x\", idx(j, 0, v.x)), num(j, \"y\", idx(j, 1, v.y)), num(j, \"z\", idx(j, 2, v.z)), num(j, \"w\", idx(j, 3, v.w))}; }\n[[maybe_unused]] void vec_from_json(const Json& j, Color4& v) { v = {num(j, \"r\", idx(j, 0, v.r)), num(j, \"g\", idx(j, 1, v.g)), num(j, \"b\", idx(j, 2, v.b)), num(j, \"a\", idx(j, 3, v.a))}; }\n\n");
    o.push_str("template <class T>\nvoid scalar_from_json(const Json& j, const char* key, T& v) {\n    if (!j.is_object() || !j.contains(key)) return;\n    const Json& x = j[key];\n    if constexpr (std::is_same_v<T, bool>) { if (x.is_boolean()) v = x.get<bool>(); else if (x.is_number()) v = x.get<double>() != 0; }\n    else if constexpr (std::is_same_v<T, std::string>) { if (x.is_string()) v = x.get<std::string>(); else if (x.is_number()) v = std::to_string(x.get<long long>()); }\n    else { if (x.is_number()) v = x.get<T>(); else if (x.is_boolean()) v = static_cast<T>(x.get<bool>()); }\n}\n\n");
    o.push_str("// \"3.weight\" -> index 3 and the rest \"weight\"; false unless the path starts with digits and a dot.\n[[maybe_unused]] bool list_index(std::string_view& rest, std::size_t& index) {\n    std::size_t dot = rest.find('.');\n    if (dot == std::string_view::npos || dot == 0) return false;\n    index = 0;\n    for (std::size_t i = 0; i < dot; ++i) {\n        char c = rest[i];\n        if (c < '0' || c > '9') return false;\n        index = index * 10 + static_cast<std::size_t>(c - '0');\n    }\n    rest = rest.substr(dot + 1);\n    return true;\n}\n\n}  // namespace\n\n");
    for r in records {
        emit_struct_cpp(&mut o, &r.name, &r.fields, &[], "hash_record")?;
    }
    for c in comps {
        emit_struct_cpp(&mut o, &c.name, &c.fields, records, "hash_component")?;
    }
    o.push_str("namespace {\n\n");
    for c in comps {
        o.push_str(&format!("constexpr std::array<FieldInfo, {}> k{}Fields = {{{{\n", c.fields.len(), c.name));
        for f in &c.fields {
            o.push_str(&format!("    FieldInfo{{\"{}\", \"{}\", \"{}\"}},\n", f.name, f.ty, cpp_escape(&f.doc)));
        }
        o.push_str("}};\n");
    }
    o.push_str(&format!("\nconstexpr std::array<ComponentInfo, {}> kComponents = {{{{\n", comps.len()));
    for c in comps {
        o.push_str(&format!("    ComponentInfo{{\"{}\", \"{}\", {}, k{}Fields}},\n", c.name, cpp_escape(&c.doc), c.serialized, c.name));
    }
    o.push_str("}};\n\n}  // namespace\n\nstd::span<const ComponentInfo> component_infos() { return kComponents; }\n\n}  // namespace pocket::world\n");
    Ok(o)
}

fn gen_registry_hpp(comps: &[Component]) -> String {
    // X-macro list so the world can instantiate templates per component without a hand-written list.
    let mut o = String::new();
    o.push_str("// generated by `pocket gen`; do not edit\n#pragma once\n\n#define POCKET_COMPONENT_LIST(X) \\\n");
    for (i, c) in comps.iter().enumerate() {
        o.push_str(&format!("    X({}){}\n", c.name, if i + 1 < comps.len() { " \\" } else { "" }));
    }
    o
}

fn gen_ts(records: &[Record], comps: &[Component]) -> Result<String> {
    let mut o = String::new();
    o.push_str("// generated by `pocket gen` from engine/*/meta/*.toml; do not edit\n\nexport interface Vec2 { x: number; y: number }\nexport interface Vec3 { x: number; y: number; z: number }\nexport interface Vec4 { x: number; y: number; z: number; w: number }\nexport interface Quat { x: number; y: number; z: number; w: number }\nexport interface Color { r: number; g: number; b: number; a: number }\n\n");
    for r in records {
        o.push_str(&format!("/** {} */\nexport interface {} {{\n", r.doc, r.name));
        for f in &r.fields {
            let info = type_info(&f.ty, &[])?;
            o.push_str(&format!("    /** {} */\n    {}: {};\n", f.doc, f.name, info.ts));
        }
        o.push_str("}\n\n");
    }
    for c in comps {
        o.push_str(&format!("/** {} */\nexport interface {} {{\n", c.doc, c.name));
        for f in &c.fields {
            let info = type_info(&f.ty, records)?;
            o.push_str(&format!("    /** {} */\n    {}: {};\n", f.doc, f.name, info.ts));
        }
        o.push_str("}\n\n");
    }
    o.push_str("export interface Components {\n");
    for c in comps {
        o.push_str(&format!("    {0}: {0};\n", c.name));
    }
    o.push_str("}\n\nexport type ComponentName = keyof Components;\n\n");
    o.push_str("export const componentNames: readonly ComponentName[] = [");
    o.push_str(&comps.iter().map(|c| format!("\"{}\"", c.name)).collect::<Vec<_>>().join(", "));
    o.push_str("];\n\n");
    o.push_str("/** Default value of every component, as the engine initializes it. */\nexport const componentDefaults: { readonly [K in ComponentName]: Components[K] } = {\n");
    for c in comps {
        o.push_str(&format!("    {}: {{ ", c.name));
        let parts: Result<Vec<String>> = c.fields.iter().map(|f| Ok(format!("{}: {}", f.name, ts_default(f, records)?))).collect();
        o.push_str(&parts?.join(", "));
        o.push_str(" },\n");
    }
    o.push_str("};\n\n/** Records: the values inside list fields, with their defaults. */\nexport interface Records {\n");
    for r in records {
        o.push_str(&format!("    {0}: {0};\n", r.name));
    }
    o.push_str("}\n\nexport const recordDefaults: { readonly [K in keyof Records]: Records[K] } = {\n");
    for r in records {
        o.push_str(&format!("    {}: {{ ", r.name));
        let parts: Result<Vec<String>> = r.fields.iter().map(|f| Ok(format!("{}: {}", f.name, ts_default(f, &[])?))).collect();
        o.push_str(&parts?.join(", "));
        o.push_str(" },\n");
    }
    o.push_str("};\n\n/** Components that are computed by the engine and never written to scene files. */\nexport const derivedComponents: readonly ComponentName[] = [");
    o.push_str(&comps.iter().filter(|c| !c.serialized).map(|c| format!("\"{}\"", c.name)).collect::<Vec<_>>().join(", "));
    o.push_str("];\n");
    Ok(o)
}

fn gen_md(records: &[Record], comps: &[Component]) -> String {
    let mut o = String::new();
    o.push_str("# Components\n\nGenerated by `pocket gen` from `engine/*/meta/*.toml`. Every component is a plain record; `world.set(entity, name, partial)` merges the given fields onto the current value (or the default when the component is new). A `list:<Record>` field is an array of the record documented at the end; a patch replaces the whole array.\n\n");
    for c in comps {
        o.push_str(&format!("## {}\n\n{}{}\n\n| Field | Type | Default | Meaning |\n|---|---|---|---|\n", c.name, c.doc, if c.serialized { "" } else { " Derived: computed by the engine, not stored in scenes." }));
        for f in &c.fields {
            o.push_str(&format!("| `{}` | {} | {} | {} |\n", f.name, f.ty, default_text(f), f.doc));
        }
        o.push('\n');
    }
    if !records.is_empty() {
        o.push_str("# Records\n\nThe values held by list fields. An element takes these defaults for the fields a patch leaves out.\n\n");
        for r in records {
            o.push_str(&format!("## {}\n\n{}\n\n| Field | Type | Default | Meaning |\n|---|---|---|---|\n", r.name, r.doc));
            for f in &r.fields {
                o.push_str(&format!("| `{}` | {} | {} | {} |\n", f.name, f.ty, default_text(f), f.doc));
            }
            o.push('\n');
        }
    }
    o
}

fn write_if_changed(path: &Path, content: &str) -> Result<bool> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        if existing == content {
            return Ok(false);
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content).with_context(|| format!("writing {}", path.display()))?;
    Ok(true)
}

pub struct GenOutput {
    pub inputs: Vec<PathBuf>,
    pub outputs: Vec<PathBuf>,
    pub changed: Vec<PathBuf>,
    pub components: usize,
}

pub fn generate(ws: &Workspace, check_only: bool) -> Result<GenOutput> {
    let (inputs, records, comps) = load_meta(ws)?;
    let world_dir = ws.root.join("engine").join("world");
    let targets: Vec<(PathBuf, String)> = vec![
        (world_dir.join("include/pocket/world/components.gen.hpp"), gen_hpp(&records, &comps)?),
        (world_dir.join("include/pocket/world/component_list.gen.hpp"), gen_registry_hpp(&comps)),
        (world_dir.join("generated/components.gen.cpp"), gen_cpp(&records, &comps)?),
        (ws.root.join("sdk/runtime/generated/components.ts"), gen_ts(&records, &comps)?),
        (ws.root.join("docs/generated/components.md"), gen_md(&records, &comps)),
    ];
    let mut outputs = vec![];
    let mut changed = vec![];
    for (path, content) in targets {
        if check_only {
            let existing = std::fs::read_to_string(&path).unwrap_or_default();
            if existing != content {
                changed.push(path.clone());
            }
        } else if write_if_changed(&path, &content)? {
            changed.push(path.clone());
        }
        outputs.push(path);
    }
    Ok(GenOutput { inputs, outputs, changed, components: comps.len() })
}
