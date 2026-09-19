//! Module graph resolution: transitive includes, defines and link closure.

use crate::manifest::{Dependency, Workspace};
use anyhow::{anyhow, bail, Result};
use indexmap::{IndexMap, IndexSet};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Resolved {
    pub name: String,
    pub kind: String,
    pub dir: PathBuf,
    pub sources: Vec<PathBuf>,
    pub include_dirs: Vec<PathBuf>,
    pub defines: Vec<String>,
    pub cxx_flags: Vec<String>,
    pub c_flags: Vec<String>,
    pub third_party: bool,
    pub output: String,
    /// Static-library modules to link, dependencies first.
    pub link_modules: Vec<String>,
    /// External dependency names in the link closure.
    pub link_deps: Vec<String>,
    pub frameworks: Vec<String>,
    pub link_flags: Vec<String>,
}

pub struct Graph {
    pub modules: IndexMap<String, Resolved>,
}

fn expand_sources(ws: &Workspace, dir: &PathBuf, patterns: &[String]) -> Result<Vec<PathBuf>> {
    let mut builder = globset::GlobSetBuilder::new();
    for p in patterns {
        builder.add(globset::Glob::new(p).map_err(|e| anyhow!("bad source glob {p}: {e}"))?);
    }
    let set = builder.build()?;
    let mut out = vec![];
    for entry in walkdir::WalkDir::new(dir).sort_by_file_name().into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry.path().strip_prefix(dir).unwrap();
        if set.is_match(rel) {
            out.push(entry.path().to_path_buf());
        }
    }
    let _ = ws;
    Ok(out)
}

/// Public surface a module exposes to its dependents.
struct Surface {
    include_dirs: Vec<PathBuf>,
    defines: Vec<String>,
}

impl Graph {
    pub fn resolve(ws: &Workspace) -> Result<Graph> {
        Self::resolve_for(ws, "native")
    }

    pub fn resolve_for(ws: &Workspace, target: &str) -> Result<Graph> {
        // Validate references first.
        for (name, m) in &ws.modules {
            for d in m.file.public_deps.iter().chain(m.file.private_deps.iter()) {
                if !ws.modules.contains_key(d) && ws.dependency(d).is_none() {
                    bail!("module {name} depends on unknown '{d}' (not a module, not a dependency in pocket.toml)");
                }
            }
        }
        let order = topo_order(ws)?;
        let mut surfaces: IndexMap<String, Surface> = IndexMap::new();
        let mut modules = IndexMap::new();
        for name in &order {
            let m = &ws.modules[name];
            if m.file.exclude_targets.iter().any(|t| t == target) || (target == "wasm" && m.file.kind == "test") {
                continue;
            }
            // Own public surface = own public includes/defines + surfaces of public deps (transitively).
            let mut pub_inc: IndexSet<PathBuf> = IndexSet::new();
            let mut pub_def: IndexSet<String> = IndexSet::new();
            for inc in &m.file.public_include {
                pub_inc.insert(m.dir.join(inc));
            }
            for d in &m.file.public_defines {
                pub_def.insert(d.clone());
            }
            for d in &m.file.public_deps {
                merge_dep_surface(ws, &surfaces, d, &mut pub_inc, &mut pub_def, target);
            }
            // Compile surface = public surface + private includes + private deps' surfaces.
            let mut inc: IndexSet<PathBuf> = pub_inc.clone();
            let mut def: IndexSet<String> = pub_def.clone();
            for i in &m.file.private_include {
                inc.insert(m.dir.join(i));
            }
            for d in &m.file.defines {
                def.insert(d.clone());
            }
            for d in &m.file.private_deps {
                merge_dep_surface(ws, &surfaces, d, &mut inc, &mut def, target);
            }
            let sources = expand_sources(ws, &m.dir, &m.file.sources)?;
            if sources.is_empty() && m.file.kind != "static_library" {
                bail!("module {name} has no sources");
            }
            // Link closure.
            let (link_modules, link_deps) = link_closure(ws, name);
            let mut frameworks: IndexSet<String> = IndexSet::new();
            let mut link_flags: IndexSet<String> = IndexSet::new();
            for lm in link_modules.iter().chain(std::iter::once(name)) {
                let mf = &ws.modules[lm].file;
                for f in &mf.frameworks {
                    frameworks.insert(f.clone());
                }
                for f in &mf.link_flags {
                    link_flags.insert(f.clone());
                }
            }
            surfaces.insert(name.clone(), Surface { include_dirs: pub_inc.into_iter().collect(), defines: pub_def.into_iter().collect() });
            modules.insert(
                name.clone(),
                Resolved {
                    name: name.clone(),
                    kind: m.file.kind.clone(),
                    dir: m.dir.clone(),
                    sources,
                    include_dirs: inc.into_iter().collect(),
                    defines: def.into_iter().collect(),
                    cxx_flags: m.file.cxx_flags.clone(),
                    c_flags: m.file.c_flags.clone(),
                    third_party: m.file.third_party,
                    output: m.file.output.clone().unwrap_or_else(|| name.clone()),
                    link_modules,
                    link_deps,
                    frameworks: frameworks.into_iter().collect(),
                    link_flags: link_flags.into_iter().collect(),
                },
            );
        }
        Ok(Graph { modules })
    }

    pub fn dependency_link_info<'a>(&self, ws: &'a Workspace, name: &str) -> Option<&'a Dependency> {
        ws.dependency(name)
    }
}

fn merge_dep_surface(ws: &Workspace, surfaces: &IndexMap<String, Surface>, dep: &str, inc: &mut IndexSet<PathBuf>, def: &mut IndexSet<String>, target: &str) {
    if let Some(s) = surfaces.get(dep) {
        for i in &s.include_dirs {
            inc.insert(i.clone());
        }
        for d in &s.defines {
            def.insert(d.clone());
        }
    } else if let Some(d) = ws.dependency(dep) {
        if !crate::deps::applies(d, target) {
            return;
        }
        let pfx = crate::deps::prefix_for(ws, d, target);
        for i in &d.include_dirs {
            inc.insert(pfx.join(i));
        }
        for x in &d.defines {
            def.insert(x.clone());
        }
    }
}

/// Modules in dependency order (a module appears after everything it depends on).
fn topo_order(ws: &Workspace) -> Result<Vec<String>> {
    let mut order = vec![];
    let mut state: IndexMap<String, u8> = IndexMap::new(); // 1 = visiting, 2 = done
    fn visit(ws: &Workspace, n: &str, state: &mut IndexMap<String, u8>, order: &mut Vec<String>, stack: &mut Vec<String>) -> Result<()> {
        match state.get(n) {
            Some(2) => return Ok(()),
            Some(1) => bail!("dependency cycle: {} -> {}", stack.join(" -> "), n),
            _ => {}
        }
        state.insert(n.to_string(), 1);
        stack.push(n.to_string());
        let m = &ws.modules[n];
        for d in m.file.public_deps.iter().chain(m.file.private_deps.iter()) {
            if ws.modules.contains_key(d) {
                visit(ws, d, state, order, stack)?;
            }
        }
        stack.pop();
        state.insert(n.to_string(), 2);
        order.push(n.to_string());
        Ok(())
    }
    for name in ws.modules.keys() {
        visit(ws, name, &mut state, &mut order, &mut vec![])?;
    }
    Ok(order)
}

/// Static libraries (dependencies first) and external dependencies needed to link `name`.
fn link_closure(ws: &Workspace, name: &str) -> (Vec<String>, Vec<String>) {
    let mut mods: IndexSet<String> = IndexSet::new();
    let mut deps: IndexSet<String> = IndexSet::new();
    fn walk(ws: &Workspace, n: &str, mods: &mut IndexSet<String>, deps: &mut IndexSet<String>) {
        let m = &ws.modules[n];
        for d in m.file.public_deps.iter().chain(m.file.private_deps.iter()) {
            if ws.modules.contains_key(d) {
                if !mods.contains(d) {
                    walk(ws, d, mods, deps);
                    mods.insert(d.clone());
                }
            } else {
                deps.insert(d.clone());
            }
        }
    }
    walk(ws, name, &mut mods, &mut deps);
    // Link order: dependents before dependencies (reverse of the post-order collected above).
    let mut v: Vec<String> = mods.into_iter().collect();
    v.reverse();
    (v, deps.into_iter().collect())
}
