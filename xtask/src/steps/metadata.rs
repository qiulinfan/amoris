//! The part of `cargo metadata --format-version 1` the checks read.
//!
//! The resolve graph's edges come from cargo's dependency resolver, which lists an optional
//! dependency that the feature resolver never turns on: `bevy_ecs` 0.19.1 with only `std` shows an
//! edge to `bevy_reflect`, and through it to `wgpu-types` and `web-sys` on wasm32, though
//! `cargo tree` builds none of them. So an edge counts only when the dependency is not optional or
//! one of the package's resolved features turns it on.

use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Debug, Default, Deserialize)]
pub struct Metadata {
    pub packages: Vec<Package>,
    pub workspace_members: Vec<String>,
    pub resolve: Option<Resolve>,
    #[serde(default)]
    pub target_directory: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct Package {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    #[serde(default)]
    pub features: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Dependency {
    pub name: String,
    pub kind: Option<String>,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub rename: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Resolve {
    pub nodes: Vec<Node>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Node {
    pub id: String,
    #[serde(default)]
    pub deps: Vec<NodeDep>,
    #[serde(default)]
    pub features: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct NodeDep {
    pub pkg: String,
    #[serde(default)]
    pub dep_kinds: Vec<DepKind>,
}

#[derive(Debug, Default, Deserialize)]
pub struct DepKind {
    pub kind: Option<String>,
}

impl NodeDep {
    fn normal(&self) -> bool {
        self.dep_kinds.is_empty() || self.dep_kinds.iter().any(|k| k.kind.is_none())
    }
}

impl Metadata {
    pub fn name_of<'a>(&'a self, id: &'a str) -> &'a str {
        self.package(id).map_or(id, |p| p.name.as_str())
    }

    fn package(&self, id: &str) -> Option<&Package> {
        self.packages.iter().find(|p| p.id == id)
    }

    /// The workspace members, sorted by name.
    pub fn members(&self) -> Vec<&Package> {
        let mut members: Vec<&Package> = self
            .packages
            .iter()
            .filter(|p| self.workspace_members.contains(&p.id))
            .collect();
        members.sort_by(|a, b| a.name.cmp(&b.name));
        members
    }

    fn node(&self, id: &str) -> Option<&Node> {
        self.resolve.as_ref()?.nodes.iter().find(|n| n.id == id)
    }

    /// Whether the edge from `from` to `to` is built: the dependency is declared without
    /// `optional`, or a resolved feature of `from` names it (`dep:x`, `x/feature`, or the implicit
    /// feature `x`; a weak `x?/feature` does not turn it on).
    fn active(&self, from: &Node, to: &str, normal_only: bool) -> bool {
        let Some(p) = self.package(&from.id) else {
            return true;
        };
        let name = self.name_of(to);
        let entries: Vec<&Dependency> = p
            .dependencies
            .iter()
            .filter(|d| d.name == name && (!normal_only || d.kind.is_none()))
            .collect();
        if entries.is_empty() {
            return true;
        }
        entries.iter().any(|d| {
            let key = d.rename.as_deref().unwrap_or(&d.name);
            !d.optional
                || from.features.iter().any(|f| {
                    f == key
                        || p.features.get(f).is_some_and(|list| {
                            list.iter().any(|x| {
                                x.strip_prefix("dep:") == Some(key)
                                    || x.strip_prefix(key).is_some_and(|r| r.starts_with('/'))
                            })
                        })
                })
        })
    }

    /// Every package in the normal (not dev, not build) dependencies of `from`, transitively, with
    /// the chain of names that reaches it from `from`.
    pub fn normal_closure(&self, from: &str) -> BTreeMap<String, Vec<String>> {
        let mut chains: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut seen: BTreeSet<&str> = BTreeSet::from([from]);
        let mut queue: VecDeque<(&str, Vec<String>)> =
            VecDeque::from([(from, vec![self.name_of(from).to_string()])]);
        while let Some((id, chain)) = queue.pop_front() {
            let Some(node) = self.node(id) else { continue };
            for dep in &node.deps {
                if !dep.normal() || !self.active(node, &dep.pkg, true) || !seen.insert(&dep.pkg) {
                    continue;
                }
                let mut next = chain.clone();
                next.push(self.name_of(&dep.pkg).to_string());
                chains
                    .entry(self.name_of(&dep.pkg).to_string())
                    .or_insert_with(|| next.clone());
                queue.push_back((dep.pkg.as_str(), next));
            }
        }
        chains
    }

    /// The direct normal dependencies of `from`, by name.
    pub fn direct_normal(&self, from: &str) -> Vec<String> {
        self.node(from).map_or_else(Vec::new, |n| {
            n.deps
                .iter()
                .filter(|d| d.normal() && self.active(n, &d.pkg, true))
                .map(|d| self.name_of(&d.pkg).to_string())
                .collect()
        })
    }

    /// The packages a build of the workspace compiles: reached from the members through edges of
    /// any kind that are built.
    pub fn built(&self) -> BTreeSet<&str> {
        let mut seen: BTreeSet<&str> = self.workspace_members.iter().map(String::as_str).collect();
        let mut queue: VecDeque<&str> = seen.iter().copied().collect();
        while let Some(id) = queue.pop_front() {
            let Some(node) = self.node(id) else { continue };
            for dep in &node.deps {
                if self.active(node, &dep.pkg, false) && seen.insert(&dep.pkg) {
                    queue.push_back(&dep.pkg);
                }
            }
        }
        seen
    }

    /// The resolved features of every built package called `name`.
    pub fn features_of(&self, name: &str) -> Vec<&Vec<String>> {
        let Some(resolve) = &self.resolve else {
            return Vec::new();
        };
        let built = self.built();
        resolve
            .nodes
            .iter()
            .filter(|n| self.name_of(&n.id) == name && built.contains(n.id.as_str()))
            .map(|n| &n.features)
            .collect()
    }
}
