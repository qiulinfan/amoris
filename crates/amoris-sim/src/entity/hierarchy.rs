//! The hierarchy (docs/spec/simulation.md 7.3 and 8.4): a child carries [`Parent`], and that is
//! the whole relation. Nothing derived is stored: who is under whom is read from the `Parent` rows
//! when it is asked for, into a [`Hierarchy`], whose children are in ascending id order whatever
//! the storage's, so a restore, a fork, a replay and the storage shuffle have nothing to rebuild.
//!
//! A `Parent` is checked where the world is in view (`world_edit`, a scene's load, a script's
//! write): it names a live entity and closes no loop. Elsewhere (a snapshot made by hand, a
//! component inserted from Rust) it may name itself, a despawned entity or an entity under its
//! own, so every walk here is iterative and remembers where it has been: none recurses, none loops.

use std::collections::{BTreeMap, BTreeSet};

use amoris_contract::{Problem, detail};
use bevy_ecs::prelude::{Component, World};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::EntityId;

/// Places an entity under another in the hierarchy; its Transform stays in world space.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Parent {
    /// The parent entity.
    pub parent: EntityId,
}

/// Who is under whom, as a set of `Parent` rows gives it: each child's parent, and each parent's
/// children in ascending id order. A value, never stored in the world: [`Hierarchy::read`] makes
/// it from the rows, and a caller that changes the rows as it goes ([`Hierarchy::set`],
/// [`Hierarchy::clear`]) keeps it in step.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hierarchy {
    parent_of: BTreeMap<EntityId, EntityId>,
    children_of: BTreeMap<EntityId, BTreeSet<EntityId>>,
}

impl Hierarchy {
    /// The hierarchy of the world's `Parent` rows; empty for a world that never held one.
    pub fn read(world: &World) -> Hierarchy {
        let Some(mut q) = world.try_query::<(&EntityId, &Parent)>() else {
            return Hierarchy::default();
        };
        #[allow(clippy::disallowed_methods)] // collected into maps ordered by id
        let rows = q.iter(world).map(|(id, p)| (*id, p.parent));
        Hierarchy::from_links(rows)
    }

    /// The hierarchy of `(child, parent)` links; a child given twice keeps its last parent.
    pub fn from_links(links: impl IntoIterator<Item = (EntityId, EntityId)>) -> Hierarchy {
        let mut h = Hierarchy::default();
        for (child, parent) in links {
            h.set(child, parent);
        }
        h
    }

    /// Whether no entity has a parent.
    pub fn is_empty(&self) -> bool {
        self.parent_of.is_empty()
    }

    /// The entity `child`'s `Parent` names, live or not.
    pub fn parent(&self, child: EntityId) -> Option<EntityId> {
        self.parent_of.get(&child).copied()
    }

    /// The entities whose `Parent` names `parent`, ascending by id.
    pub fn children(&self, parent: EntityId) -> impl Iterator<Item = EntityId> + '_ {
        self.children_of.get(&parent).into_iter().flatten().copied()
    }

    /// Puts `child` under `parent`, out from under the parent it had.
    pub fn set(&mut self, child: EntityId, parent: EntityId) {
        self.clear(child);
        self.parent_of.insert(child, parent);
        self.children_of.entry(parent).or_default().insert(child);
    }

    /// Takes `child` out from under its parent; its own children stay under it.
    pub fn clear(&mut self, child: EntityId) {
        let Some(parent) = self.parent_of.remove(&child) else {
            return;
        };
        if let Some(children) = self.children_of.get_mut(&parent) {
            children.remove(&child);
            if children.is_empty() {
                self.children_of.remove(&parent);
            }
        }
    }

    /// Every entity under `root`, each once and never `root` itself: a parent before its children,
    /// siblings ascending by id (depth first). A loop among them ends where it closes.
    pub fn descendants(&self, root: EntityId) -> Vec<EntityId> {
        let mut out = Vec::new();
        let mut seen = BTreeSet::from([root]);
        let mut stack: Vec<EntityId> = self.children(root).collect();
        stack.reverse();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            out.push(id);
            let from = stack.len();
            stack.extend(self.children(id).filter(|c| !seen.contains(c)));
            stack[from..].reverse();
        }
        out
    }

    /// Whether putting `child` under `parent` would close a loop: `parent` is `child` or is under
    /// it. `Some` is the chain from `parent` up to `child`, both included (one id when they are
    /// the same). `child`'s own link is not read, so the answer is the same before and after the
    /// link is made. A walk that meets a loop that does not pass through `child` ends with `None`.
    pub fn loop_through(&self, child: EntityId, parent: EntityId) -> Option<Vec<EntityId>> {
        let mut chain = vec![parent];
        let mut seen = BTreeSet::from([parent]);
        let mut at = parent;
        while at != child {
            at = self.parent(at)?;
            if !seen.insert(at) {
                return None;
            }
            chain.push(at);
        }
        Some(chain)
    }
}

/// `sim.parent_cycle {entity, parent, chain}`: `entity` cannot go under `parent`, which is the
/// entity itself or under it; `chain` is [`Hierarchy::loop_through`]'s, from `parent` up to
/// `entity`.
pub fn parent_cycle(entity: EntityId, parent: EntityId, chain: &[EntityId]) -> Problem {
    let ids: Vec<u64> = chain.iter().map(|id| id.get()).collect();
    let message = if entity == parent {
        format!("Entity {} cannot be its own parent.", entity.get())
    } else {
        let path: Vec<String> = ids.iter().map(u64::to_string).collect();
        format!(
            "Entity {} cannot go under entity {}: {} is under {} ({}).",
            entity.get(),
            parent.get(),
            parent.get(),
            entity.get(),
            path.join(" -> ")
        )
    };
    Problem::new(
        "sim.parent_cycle",
        message,
        detail([
            ("entity", json!(entity.get())),
            ("parent", json!(parent.get())),
            ("chain", json!(ids)),
        ]),
    )
}

/// The deepest a listing nests below the entities it starts from, and what it nests to when no
/// depth is asked for. A level of a tree is two levels of its JSON (the node and its `children`),
/// and `serde_json` refuses to read more than 128.
pub const MAX_TREE_DEPTH: u32 = 48;

/// One entity of a listing: how deep it is below the entities the listing starts from, and how
/// many of its children the depth left out (0 unless it is at the last level).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Listed {
    pub id: EntityId,
    pub level: u32,
    pub more: u32,
}

/// The world as a tree (`world.tree`): the entities of `live` (ascending by id), depth first, a
/// parent before its children and siblings ascending by id, each once.
///
/// - An entity's parent is the one its `Parent` names when that entity is live and is not the
///   entity itself; any other entity is a root. Entities that no root reaches (a loop, and what
///   hangs under it) are placed by taking the lowest such id as one more root, until every live
///   entity has a place.
/// - The listing starts from `root`, or from every root.
/// - It goes `depth` levels below where it starts, [`MAX_TREE_DEPTH`] by default and at most; an
///   entity at the last level carries the number of its children left out in `more`.
/// - An entity is listed when `keep` holds for it or for an entity listed under it, so a match
///   comes with its ancestors.
/// - `limit` keeps the first entities listed.
///
/// With no `Parent` row this is the live entities `keep` holds for, in id order, up to `limit`.
/// `keep` is asked once for each entity walked, in the listing's order, and not past the limit.
pub fn listing<E>(
    live: &[EntityId],
    h: &Hierarchy,
    root: Option<EntityId>,
    depth: Option<u32>,
    limit: Option<u32>,
    mut keep: impl FnMut(EntityId) -> Result<bool, E>,
) -> Result<Vec<Listed>, E> {
    let depth = depth.map_or(MAX_TREE_DEPTH, |d| d.min(MAX_TREE_DEPTH));
    let limit = limit.map_or(usize::MAX, |l| usize::try_from(l).unwrap_or(usize::MAX));
    let is_live = |id: EntityId| live.binary_search(&id).is_ok();
    let parent = |id: EntityId| h.parent(id).filter(|p| *p != id && is_live(*p));
    let starts: Vec<EntityId> = match root {
        Some(r) => vec![r],
        None if h.is_empty() => live.to_vec(),
        None => {
            let mut roots: Vec<EntityId> = live
                .iter()
                .copied()
                .filter(|id| parent(*id).is_none())
                .collect();
            // What the roots reach; the lowest id left over becomes a root, until none is.
            let mut reached = BTreeSet::new();
            let mut from = 0;
            loop {
                let mut stack: Vec<EntityId> = roots[from..].to_vec();
                from = roots.len();
                while let Some(id) = stack.pop() {
                    if reached.insert(id) {
                        stack.extend(h.children(id).filter(|c| is_live(*c)));
                    }
                }
                match live.iter().find(|id| !reached.contains(id)) {
                    Some(id) => roots.push(*id),
                    None => break,
                }
            }
            roots.sort_unstable();
            roots
        }
    };
    let mut out = Vec::new();
    // The entities from a start down to the one being walked, with whether each is listed yet.
    let mut path: Vec<(Listed, bool)> = Vec::new();
    let mut seen: BTreeSet<EntityId> = starts.iter().copied().collect();
    let mut stack: Vec<(EntityId, u32)> = starts.iter().rev().map(|id| (*id, 0)).collect();
    while let Some((id, level)) = stack.pop() {
        if out.len() >= limit {
            break;
        }
        let children = || {
            h.children(id)
                .filter(|c| *c != id && is_live(*c) && !seen.contains(c))
        };
        let mut more = 0u32;
        if level < depth {
            let from = stack.len();
            stack.extend(children().map(|c| (c, level + 1)));
            stack[from..].reverse();
            seen.extend(stack[from..].iter().map(|(c, _)| *c));
        } else {
            more = u32::try_from(children().count()).unwrap_or(u32::MAX);
        }
        path.truncate(usize::try_from(level).unwrap_or(usize::MAX));
        path.push((Listed { id, level, more }, false));
        if keep(id)? {
            for (node, listed) in &mut path {
                if !*listed && out.len() < limit {
                    out.push(*node);
                    *listed = true;
                }
            }
        }
    }
    Ok(out)
}

/// The tree of a [`listing`]: `make` builds each entity's node from the nodes of its children,
/// and the roots' nodes come back, in the listing's order.
pub fn nest<T>(listed: &[Listed], make: &mut impl FnMut(&Listed, Vec<T>) -> T) -> Vec<T> {
    // By level, the nodes built so far that wait for their parent, last listed first.
    let mut waiting: Vec<Vec<T>> = Vec::new();
    for node in listed.iter().rev() {
        let level = usize::try_from(node.level).unwrap_or(usize::MAX);
        if waiting.len() < level.saturating_add(2) {
            waiting.resize_with(level.saturating_add(2), Vec::new);
        }
        let mut children = std::mem::take(&mut waiting[level + 1]);
        children.reverse();
        let built = make(node, children);
        waiting[level].push(built);
    }
    let mut roots = waiting.into_iter().next().unwrap_or_default();
    roots.reverse();
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> EntityId {
        EntityId::new(n).expect("an id")
    }

    fn ids(ns: &[u64]) -> Vec<EntityId> {
        ns.iter().map(|n| id(*n)).collect()
    }

    fn links(pairs: &[(u64, u64)]) -> Hierarchy {
        Hierarchy::from_links(pairs.iter().map(|(c, p)| (id(*c), id(*p))))
    }

    fn got(v: &[EntityId]) -> Vec<u64> {
        v.iter().map(|i| i.get()).collect()
    }

    #[test]
    fn children_and_descendants_are_in_id_order_whatever_the_insertion_order() {
        // 1 has 4, 2 and 3; 2 has 9 and 5; 5 has 6.
        let h = links(&[(9, 2), (4, 1), (6, 5), (3, 1), (5, 2), (2, 1)]);
        let same = links(&[(2, 1), (3, 1), (4, 1), (5, 2), (9, 2), (6, 5)]);
        assert_eq!(h, same);
        assert_eq!(got(&h.children(id(1)).collect::<Vec<_>>()), [2, 3, 4]);
        assert_eq!(got(&h.children(id(7)).collect::<Vec<_>>()), [0u64; 0]);
        assert_eq!(got(&h.descendants(id(1))), [2, 5, 6, 9, 3, 4]);
        assert_eq!(got(&h.descendants(id(2))), [5, 6, 9]);
        assert!(h.descendants(id(6)).is_empty());
        assert_eq!(h.parent(id(6)), Some(id(5)));
        assert_eq!(h.parent(id(1)), None);
        assert!(!h.is_empty());
        assert!(Hierarchy::default().is_empty());
    }

    #[test]
    fn set_and_clear_keep_both_sides_in_step() {
        /// The hierarchy is the one of `want`, read from either side.
        fn holds(h: &Hierarchy, want: &[(u64, u64)]) {
            assert_eq!(*h, links(want));
            for (c, p) in want {
                assert_eq!(h.parent(id(*c)), Some(id(*p)));
                assert!(h.children(id(*p)).any(|x| x == id(*c)));
            }
        }
        let mut h = links(&[(2, 1), (3, 1)]);
        h.set(id(3), id(2));
        holds(&h, &[(2, 1), (3, 2)]);
        assert_eq!(got(&h.children(id(1)).collect::<Vec<_>>()), [2]);
        h.set(id(3), id(1));
        h.clear(id(2));
        holds(&h, &[(3, 1)]);
        // Clearing an entity without a parent changes nothing; its children stay under it.
        h.clear(id(2));
        h.clear(id(1));
        holds(&h, &[(3, 1)]);
        h.clear(id(3));
        assert!(h.is_empty());
        assert_eq!(h, Hierarchy::default());
    }

    /// What no checked write makes but a world may hold: a loop of two with an entity hanging
    /// under it, an entity that is its own parent, and a parent that is not there.
    #[test]
    fn walks_end_on_loops_self_parents_and_dangling_parents() {
        let h = links(&[(5, 6), (6, 5), (3, 5), (7, 7), (8, 99)]);
        assert_eq!(got(&h.descendants(id(5))), [3, 6]);
        assert_eq!(got(&h.descendants(id(6))), [5, 3]);
        assert!(h.descendants(id(7)).is_empty());
        assert_eq!(got(&h.descendants(id(99))), [8]);
        // A walk up that meets the loop without passing through the child ends.
        assert_eq!(h.loop_through(id(1), id(3)), None);
        assert_eq!(h.loop_through(id(1), id(7)), None);
        assert_eq!(h.loop_through(id(1), id(8)), None);
        // The loop that is there is found from inside it.
        assert_eq!(got(&h.loop_through(id(5), id(6)).unwrap()), [6, 5]);
        assert_eq!(got(&h.loop_through(id(7), id(7)).unwrap()), [7]);
    }

    #[test]
    fn a_loop_is_the_entity_itself_or_one_under_it() {
        // 1 > 2 > 3, and 4 beside 2.
        let h = links(&[(2, 1), (3, 2), (4, 1)]);
        assert_eq!(got(&h.loop_through(id(1), id(1)).unwrap()), [1]);
        assert_eq!(got(&h.loop_through(id(1), id(3)).unwrap()), [3, 2, 1]);
        assert_eq!(got(&h.loop_through(id(2), id(3)).unwrap()), [3, 2]);
        assert_eq!(h.loop_through(id(2), id(4)), None, "a sibling");
        assert_eq!(h.loop_through(id(3), id(1)), None, "its own ancestor");
        assert_eq!(h.loop_through(id(9), id(3)), None, "an entity apart");
        let p = parent_cycle(id(1), id(3), &ids(&[3, 2, 1]));
        assert_eq!(p.code, "sim.parent_cycle");
        assert_eq!(
            p.message,
            "Entity 1 cannot go under entity 3: 3 is under 1 (3 -> 2 -> 1)."
        );
        assert_eq!(p.detail["chain"], json!([3, 2, 1]));
        assert_eq!(p.detail["entity"], json!(1));
        assert_eq!(p.detail["parent"], json!(3));
        let own = parent_cycle(id(4), id(4), &ids(&[4]));
        assert_eq!(own.message, "Entity 4 cannot be its own parent.");
    }

    /// 20,000 entities each under the one before: every walk is a loop over a list, not a
    /// recursion, so none overflows the stack.
    #[test]
    fn a_long_chain_is_walked_without_recursion() {
        let n = 20_000u64;
        let h = Hierarchy::from_links((2..=n).map(|i| (id(i), id(i - 1))));
        let d = h.descendants(id(1));
        assert_eq!(d.len(), 19_999);
        assert_eq!(d.first(), Some(&id(2)));
        assert_eq!(d.last(), Some(&id(n)));
        assert_eq!(h.loop_through(id(1), id(n)).map(|c| c.len()), Some(20_000));
        let live: Vec<EntityId> = (1..=n).map(id).collect();
        let all = listing(&live, &h, None, None, None, |_| Ok::<_, ()>(true)).unwrap();
        assert_eq!(all.len(), 49);
        assert_eq!(all.last().map(|l| (l.level, l.more)), Some((48, 1)));
    }

    fn list(
        live: &[u64],
        h: &Hierarchy,
        root: Option<u64>,
        depth: Option<u32>,
        limit: Option<u32>,
        keep: &[u64],
    ) -> Vec<(u64, u32, u32)> {
        let all = keep.is_empty();
        listing(&ids(live), h, root.map(id), depth, limit, |e| {
            Ok::<_, ()>(all || keep.contains(&e.get()))
        })
        .unwrap()
        .iter()
        .map(|l| (l.id.get(), l.level, l.more))
        .collect()
    }

    /// 1 > (2 > (5 > 6, 9), 3), 4 a root, 7 under 4.
    fn family() -> (Vec<u64>, Hierarchy) {
        (
            vec![1, 2, 3, 4, 5, 6, 7, 9],
            links(&[(2, 1), (3, 1), (5, 2), (9, 2), (6, 5), (7, 4)]),
        )
    }

    #[test]
    fn a_listing_is_depth_first_with_roots_and_children_ascending() {
        let (live, h) = family();
        assert_eq!(
            list(&live, &h, None, None, None, &[]),
            [
                (1, 0, 0),
                (2, 1, 0),
                (5, 2, 0),
                (6, 3, 0),
                (9, 2, 0),
                (3, 1, 0),
                (4, 0, 0),
                (7, 1, 0)
            ]
        );
        // One subtree, by its root.
        assert_eq!(
            list(&live, &h, Some(2), None, None, &[]),
            [(2, 0, 0), (5, 1, 0), (6, 2, 0), (9, 1, 0)]
        );
        assert_eq!(list(&live, &h, Some(6), None, None, &[]), [(6, 0, 0)]);
    }

    #[test]
    fn depth_cuts_and_says_how_many_children_it_left() {
        let (live, h) = family();
        assert_eq!(
            list(&live, &h, None, Some(0), None, &[]),
            [(1, 0, 2), (4, 0, 1)]
        );
        assert_eq!(
            list(&live, &h, None, Some(1), None, &[]),
            [(1, 0, 0), (2, 1, 2), (3, 1, 0), (4, 0, 0), (7, 1, 0)]
        );
        assert_eq!(
            list(&live, &h, Some(2), Some(1), None, &[]),
            [(2, 0, 0), (5, 1, 1), (9, 1, 0)]
        );
        // A depth past the cap is the cap: a chain of 100 stops at level 48 with one child left.
        let chain = Hierarchy::from_links((2..=100).map(|i| (id(i), id(i - 1))));
        let live: Vec<u64> = (1..=100).collect();
        for depth in [None, Some(48), Some(1000), Some(u32::MAX)] {
            let l = list(&live, &chain, None, depth, None, &[]);
            assert_eq!(l.len(), 49, "{depth:?}");
            assert_eq!(l[48], (49, 48, 1));
            assert!(l[..48].iter().all(|n| n.2 == 0));
        }
        assert_eq!(list(&live, &chain, None, Some(47), None, &[]).len(), 48);
    }

    #[test]
    fn a_match_is_listed_with_its_ancestors_and_nothing_else() {
        let (live, h) = family();
        assert_eq!(
            list(&live, &h, None, None, None, &[6]),
            [(1, 0, 0), (2, 1, 0), (5, 2, 0), (6, 3, 0)]
        );
        assert_eq!(
            list(&live, &h, None, None, None, &[9, 7]),
            [(1, 0, 0), (2, 1, 0), (9, 2, 0), (4, 0, 0), (7, 1, 0)]
        );
        assert_eq!(list(&live, &h, None, None, None, &[4]), [(4, 0, 0)]);
        assert!(list(&live, &h, None, None, None, &[99]).is_empty());
        // A match below the depth is not walked, so it keeps nothing.
        assert!(list(&live, &h, None, Some(1), None, &[6]).is_empty());
        assert!(list(&live, &h, Some(3), None, None, &[6]).is_empty());
    }

    #[test]
    fn limit_keeps_the_first_listed() {
        let (live, h) = family();
        assert_eq!(
            list(&live, &h, None, None, Some(3), &[]),
            [(1, 0, 0), (2, 1, 0), (5, 2, 0)]
        );
        assert!(list(&live, &h, None, None, Some(0), &[]).is_empty());
        // The ancestors of a match count: the limit can end the listing before the match.
        assert_eq!(
            list(&live, &h, None, None, Some(2), &[6]),
            [(1, 0, 0), (2, 1, 0)]
        );
        assert_eq!(list(&live, &h, None, None, Some(100), &[]).len(), 8);
        // `keep` is not asked past the limit.
        let mut asked = Vec::new();
        let l = listing(&ids(&live), &h, None, None, Some(2), |e| {
            asked.push(e.get());
            Ok::<_, ()>(true)
        });
        assert_eq!(l.unwrap().len(), 2);
        assert_eq!(asked, [1, 2]);
        // Its refusal is the listing's.
        let e = listing(&ids(&live), &h, None, None, None, |e| {
            if e.get() == 5 { Err("five") } else { Ok(true) }
        });
        assert_eq!(e.unwrap_err(), "five");
    }

    #[test]
    fn without_links_a_listing_is_a_filter_then_a_limit() {
        let live = [1, 2, 3, 4, 5, 6];
        let h = Hierarchy::default();
        assert_eq!(
            list(&live, &h, None, None, None, &[]),
            [
                (1, 0, 0),
                (2, 0, 0),
                (3, 0, 0),
                (4, 0, 0),
                (5, 0, 0),
                (6, 0, 0)
            ]
        );
        assert_eq!(
            list(&live, &h, None, Some(0), Some(2), &[2, 4, 6]),
            [(2, 0, 0), (4, 0, 0)]
        );
        assert_eq!(list(&live, &h, Some(3), None, None, &[]), [(3, 0, 0)]);
    }

    /// A parent that is not live or is the entity itself makes a root; a loop lists every live
    /// entity once, starting from its lowest id.
    #[test]
    fn dangling_self_and_looping_parents_are_listed_once() {
        // 8's parent 99 is not live; 7 is its own parent; 5 and 6 are each other's, 3 under 5.
        let h = links(&[(5, 6), (6, 5), (3, 5), (7, 7), (8, 99), (2, 1)]);
        let live = [1, 2, 3, 5, 6, 7, 8];
        assert_eq!(
            list(&live, &h, None, None, None, &[]),
            [
                (1, 0, 0),
                (2, 1, 0),
                (3, 0, 0),
                (5, 0, 0),
                (6, 1, 0),
                (7, 0, 0),
                (8, 0, 0)
            ]
        );
        // From inside the loop the walk comes round once.
        assert_eq!(
            list(&live, &h, Some(6), None, None, &[]),
            [(6, 0, 0), (5, 1, 0), (3, 2, 0)]
        );
        assert_eq!(list(&live, &h, Some(7), None, None, &[]), [(7, 0, 0)]);
        assert_eq!(list(&live, &h, Some(5), Some(0), None, &[]), [(5, 0, 2)]);
    }

    #[test]
    fn nest_rebuilds_the_tree() {
        #[derive(Debug, PartialEq)]
        struct Node(u64, Vec<Node>);
        let (live, h) = family();
        let build = |listed: &[Listed]| nest(listed, &mut |l, children| Node(l.id.get(), children));
        let all = listing(&ids(&live), &h, None, None, None, |_| Ok::<_, ()>(true)).unwrap();
        assert_eq!(
            build(&all),
            [
                Node(
                    1,
                    vec![
                        Node(2, vec![Node(5, vec![Node(6, vec![])]), Node(9, vec![])]),
                        Node(3, vec![])
                    ]
                ),
                Node(4, vec![Node(7, vec![])])
            ]
        );
        // A listing a limit ended is a tree too.
        assert_eq!(
            build(&all[..3]),
            [Node(1, vec![Node(2, vec![Node(5, vec![])])])]
        );
        assert!(build(&[]).is_empty());
    }
}
