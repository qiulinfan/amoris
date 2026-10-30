//! Animation graphs (animation.md 3): a file's parameters, clips' metadata, masks, layers of state
//! machines and slots, and montages; a graph bound to its skeleton and clips (`Rig`); and one tick
//! of a graph's state (`step`): transitions and fades, blend spaces, montages and their sections,
//! notifies, root motion and the pose.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use amoris_sim::math::{max, min};

use super::clip::{Bound, ClipSet, Skeleton};
use super::math::{self, V3, Xform};
use super::state::{AnimParam, AnimState, LayerState, SlotState};

/// A parameter comparison or the clip's end.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Cond {
    Param {
        name: String,
        op: String,
        value: f64,
    },
    End,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Motion {
    Clip(String),
    Blend1 {
        param: String,
        points: Vec<(f64, String)>,
    },
    Blend2 {
        x: String,
        y: String,
        points: Vec<(f64, f64, String)>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StateDef {
    pub name: String,
    pub motion: Motion,
    pub looped: bool,
    pub rate: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TransitionDef {
    pub from: String,
    pub to: String,
    pub when: Vec<Cond>,
    pub fade: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LayerKind {
    Machine {
        start: String,
        states: Vec<StateDef>,
        transitions: Vec<TransitionDef>,
    },
    Slot(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayerDef {
    pub name: String,
    pub mask: Option<String>,
    pub kind: LayerKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NotifyDef {
    pub name: String,
    pub from: f64,
    /// The window's end; none for a notify at one time.
    pub to: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClipMeta {
    pub name: String,
    pub root_motion: bool,
    pub notifies: Vec<NotifyDef>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SectionDef {
    pub name: String,
    pub clip: String,
    pub next: Option<String>,
    pub rate: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MontageDef {
    pub name: String,
    pub slot: String,
    pub fade_in: f64,
    pub fade_out: f64,
    pub sections: Vec<SectionDef>,
}

/// A graph file, parsed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GraphDef {
    pub skeleton: String,
    pub libraries: Vec<String>,
    /// Joints whose clip translation is kept, scaled by `scale_by`'s rest offset (retargeting).
    pub translate: Vec<String>,
    pub scale_by: String,
    /// The joint whose horizontal motion a root-motion clip hands to the body.
    pub root: String,
    pub params: Vec<(String, f64)>,
    pub clips: Vec<ClipMeta>,
    pub masks: Vec<(String, Vec<(String, f32)>)>,
    pub layers: Vec<LayerDef>,
    pub montages: Vec<MontageDef>,
    /// Joints whose place the simulation keeps each tick (`AnimState.sockets`).
    pub sockets: Vec<String>,
}

fn num(v: &Value, what: &str) -> Result<f64, String> {
    match v {
        Value::Number(n) => n.as_f64().ok_or_else(|| format!("{what} is not a number")),
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        _ => Err(format!("{what} must be a number or a boolean")),
    }
}

fn text(v: &Value, what: &str) -> Result<String, String> {
    v.as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{what} must be a string"))
}

fn opt_num(
    o: &serde_json::Map<String, Value>,
    key: &str,
    default: f64,
    what: &str,
) -> Result<f64, String> {
    o.get(key)
        .map_or(Ok(default), |v| num(v, &format!("{what}'s {key}")))
}

fn strings(v: Option<&Value>, what: &str) -> Result<Vec<String>, String> {
    match v {
        None => Ok(Vec::new()),
        Some(Value::Array(a)) => a.iter().map(|x| text(x, what)).collect(),
        Some(_) => Err(format!("{what} must be a list of strings")),
    }
}

impl GraphDef {
    /// Parses a graph file (`format: "amoris-animgraph"`, version 1).
    pub fn parse(v: &Value) -> Result<GraphDef, String> {
        let o = v.as_object().ok_or("a graph is a JSON object")?;
        if o.get("format").and_then(Value::as_str) != Some("amoris-animgraph") {
            return Err("its format must be \"amoris-animgraph\"".into());
        }
        if o.get("version").and_then(Value::as_u64) != Some(1) {
            return Err("its version must be 1".into());
        }
        let mut g = GraphDef {
            skeleton: text(o.get("skeleton").unwrap_or(&Value::Null), "skeleton")?,
            libraries: strings(o.get("libraries"), "libraries")?,
            translate: vec!["root".into(), "pelvis".into()],
            scale_by: "pelvis".into(),
            root: "root".into(),
            ..GraphDef::default()
        };
        if let Some(r) = o.get("retarget").and_then(Value::as_object) {
            if r.contains_key("translate") {
                g.translate = strings(r.get("translate"), "retarget.translate")?;
            }
            if let Some(s) = r.get("scale_by") {
                g.scale_by = text(s, "retarget.scale_by")?;
            }
        }
        if let Some(r) = o.get("root") {
            g.root = text(r, "root")?;
        }
        g.sockets = strings(o.get("sockets"), "sockets")?;
        if let Some(p) = o.get("params").and_then(Value::as_object) {
            for (k, v) in p {
                g.params.push((k.clone(), num(v, &format!("param {k}"))?));
            }
        }
        if let Some(c) = o.get("clips").and_then(Value::as_object) {
            for (name, m) in c {
                let m = m
                    .as_object()
                    .ok_or(format!("clip {name} must be an object"))?;
                let mut meta = ClipMeta {
                    name: name.clone(),
                    root_motion: m
                        .get("root_motion")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    notifies: Vec::new(),
                };
                if let Some(Value::Array(ns)) = m.get("notifies") {
                    for n in ns {
                        let n = n
                            .as_object()
                            .ok_or(format!("a notify of {name} must be an object"))?;
                        let nname = text(n.get("name").unwrap_or(&Value::Null), "a notify's name")?;
                        let (from, to) = match (n.get("at"), n.get("from"), n.get("to")) {
                            (Some(at), None, None) => (num(at, "a notify's at")?, None),
                            (None, Some(f), Some(t)) => {
                                let (f, t) = (num(f, "a notify's from")?, num(t, "a notify's to")?);
                                if t < f {
                                    return Err(format!(
                                        "notify {nname} of {name} ends before it begins"
                                    ));
                                }
                                (f, Some(t))
                            }
                            _ => {
                                return Err(format!(
                                    "notify {nname} of {name} needs at, or from and to"
                                ));
                            }
                        };
                        meta.notifies.push(NotifyDef {
                            name: nname,
                            from,
                            to,
                        });
                    }
                }
                g.clips.push(meta);
            }
        }
        if let Some(m) = o.get("masks").and_then(Value::as_object) {
            for (name, list) in m {
                let list = list
                    .as_array()
                    .ok_or(format!("mask {name} must be a list"))?;
                let mut joints = Vec::new();
                for e in list {
                    let e = e
                        .as_object()
                        .ok_or(format!("an entry of mask {name} must be an object"))?;
                    let j = text(e.get("joint").unwrap_or(&Value::Null), "a mask's joint")?;
                    let w = opt_num(e, "weight", 1.0, "a mask entry")? as f32;
                    joints.push((j, w));
                }
                g.masks.push((name.clone(), joints));
            }
        }
        let layers = o
            .get("layers")
            .and_then(Value::as_array)
            .ok_or("a graph needs layers")?;
        for (i, l) in layers.iter().enumerate() {
            let l = l.as_object().ok_or("a layer must be an object")?;
            let name = l
                .get("name")
                .and_then(Value::as_str)
                .map_or(format!("layer{i}"), str::to_owned);
            let mask = l
                .get("mask")
                .map(|m| text(m, "a layer's mask"))
                .transpose()?;
            if let Some(m) = &mask
                && !g.masks.iter().any(|(n, _)| n == m)
            {
                return Err(format!("layer {name}'s mask {m} is not declared"));
            }
            let kind = if let Some(s) = l.get("slot") {
                if i == 0 {
                    return Err("the first layer must be a state machine".into());
                }
                LayerKind::Slot(text(s, "a layer's slot")?)
            } else {
                parse_machine(&name, l)?
            };
            g.layers.push(LayerDef { name, mask, kind });
        }
        if g.layers.is_empty() {
            return Err("a graph needs at least one layer".into());
        }
        if let Some(ms) = o.get("montages").and_then(Value::as_object) {
            for (name, m) in ms {
                let m = m
                    .as_object()
                    .ok_or(format!("montage {name} must be an object"))?;
                let slot = text(
                    m.get("slot").unwrap_or(&Value::Null),
                    &format!("montage {name}'s slot"),
                )?;
                if !g
                    .layers
                    .iter()
                    .any(|l| l.kind == LayerKind::Slot(slot.clone()))
                {
                    return Err(format!("montage {name}'s slot {slot} is no layer's"));
                }
                let mut sections = Vec::new();
                for s in m
                    .get("sections")
                    .and_then(Value::as_array)
                    .ok_or(format!("montage {name} needs sections"))?
                {
                    let s = s.as_object().ok_or("a section must be an object")?;
                    sections.push(SectionDef {
                        name: text(s.get("name").unwrap_or(&Value::Null), "a section's name")?,
                        clip: text(s.get("clip").unwrap_or(&Value::Null), "a section's clip")?,
                        next: s
                            .get("next")
                            .map(|n| text(n, "a section's next"))
                            .transpose()?,
                        rate: opt_num(s, "rate", 1.0, "a section")?,
                    });
                }
                for s in &sections {
                    if let Some(n) = &s.next
                        && !sections.iter().any(|x| &x.name == n)
                    {
                        return Err(format!(
                            "montage {name}'s section {} names no section {n}",
                            s.name
                        ));
                    }
                }
                if sections.is_empty() {
                    return Err(format!("montage {name} needs sections"));
                }
                g.montages.push(MontageDef {
                    name: name.clone(),
                    slot,
                    fade_in: opt_num(m, "fade_in", 0.1, "a montage")?,
                    fade_out: opt_num(m, "fade_out", 0.2, "a montage")?,
                    sections,
                });
            }
        }
        Ok(g)
    }

    /// Every clip name the graph plays.
    pub fn clip_names(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut add = |n: &str| {
            if !out.iter().any(|x| x == n) {
                out.push(n.to_owned());
            }
        };
        for l in &self.layers {
            if let LayerKind::Machine { states, .. } = &l.kind {
                for s in states {
                    match &s.motion {
                        Motion::Clip(c) => add(c),
                        Motion::Blend1 { points, .. } => points.iter().for_each(|p| add(&p.1)),
                        Motion::Blend2 { points, .. } => points.iter().for_each(|p| add(&p.2)),
                    }
                }
            }
        }
        for m in &self.montages {
            m.sections.iter().for_each(|s| add(&s.clip));
        }
        out
    }
}

fn parse_machine(layer: &str, l: &serde_json::Map<String, Value>) -> Result<LayerKind, String> {
    let states_v = l
        .get("states")
        .and_then(Value::as_object)
        .ok_or(format!("layer {layer} needs states or a slot"))?;
    let mut states = Vec::new();
    for (name, s) in states_v {
        let s = s
            .as_object()
            .ok_or(format!("state {name} must be an object"))?;
        let motion = if let Some(c) = s.get("clip") {
            Motion::Clip(text(c, &format!("state {name}'s clip"))?)
        } else if let Some(b) = s.get("blend1d").and_then(Value::as_object) {
            let param = text(b.get("param").unwrap_or(&Value::Null), "a blend1d's param")?;
            let mut points = Vec::new();
            for p in b
                .get("points")
                .and_then(Value::as_array)
                .ok_or("a blend1d needs points")?
            {
                let p = p
                    .as_array()
                    .filter(|p| p.len() == 2)
                    .ok_or("a blend1d point is [at, clip]")?;
                points.push((
                    num(&p[0], "a blend1d point")?,
                    text(&p[1], "a blend1d point's clip")?,
                ));
            }
            points.sort_by(|a, b| a.0.total_cmp(&b.0));
            if points.is_empty() {
                return Err(format!("state {name}'s blend1d has no points"));
            }
            Motion::Blend1 { param, points }
        } else if let Some(b) = s.get("blend2d").and_then(Value::as_object) {
            let x = text(b.get("x").unwrap_or(&Value::Null), "a blend2d's x")?;
            let y = text(b.get("y").unwrap_or(&Value::Null), "a blend2d's y")?;
            let mut points = Vec::new();
            for p in b
                .get("points")
                .and_then(Value::as_array)
                .ok_or("a blend2d needs points")?
            {
                let p = p
                    .as_array()
                    .filter(|p| p.len() == 3)
                    .ok_or("a blend2d point is [x, y, clip]")?;
                points.push((
                    num(&p[0], "a blend2d point")?,
                    num(&p[1], "a blend2d point")?,
                    text(&p[2], "a blend2d point's clip")?,
                ));
            }
            if points.is_empty() {
                return Err(format!("state {name}'s blend2d has no points"));
            }
            Motion::Blend2 { x, y, points }
        } else {
            return Err(format!("state {name} needs clip, blend1d or blend2d"));
        };
        states.push(StateDef {
            name: name.clone(),
            motion,
            looped: s.get("loop").and_then(Value::as_bool).unwrap_or(true),
            rate: opt_num(s, "rate", 1.0, "a state")?,
        });
    }
    let start = l
        .get("start")
        .map(|s| text(s, "a layer's start"))
        .transpose()?
        .or_else(|| states.first().map(|s| s.name.clone()))
        .ok_or(format!("layer {layer} has no states"))?;
    if !states.iter().any(|s| s.name == start) {
        return Err(format!(
            "layer {layer}'s start {start} is not one of its states"
        ));
    }
    let mut transitions = Vec::new();
    for t in l
        .get("transitions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let t = t.as_object().ok_or("a transition must be an object")?;
        let from = text(t.get("from").unwrap_or(&Value::Null), "a transition's from")?;
        let to = text(t.get("to").unwrap_or(&Value::Null), "a transition's to")?;
        for s in [&from, &to] {
            if s != "*" && !states.iter().any(|x| &x.name == s) {
                return Err(format!("layer {layer}'s transition names no state {s}"));
            }
        }
        let mut when = Vec::new();
        for c in t
            .get("when")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let c = c.as_array().ok_or("a condition is a list")?;
            match c.as_slice() {
                [Value::String(e)] if e == "end" => when.push(Cond::End),
                [n, op, v] => {
                    let op = text(op, "a condition's operator")?;
                    if !["==", "!=", "<", "<=", ">", ">="].contains(&op.as_str()) {
                        return Err(format!("unknown operator {op}"));
                    }
                    when.push(Cond::Param {
                        name: text(n, "a condition's param")?,
                        op,
                        value: num(v, "a condition's value")?,
                    });
                }
                _ => return Err("a condition is [param, op, value] or [\"end\"]".into()),
            }
        }
        transitions.push(TransitionDef {
            from,
            to,
            when,
            fade: opt_num(t, "fade", 0.2, "a transition")?,
        });
    }
    Ok(LayerKind::Machine {
        start,
        states,
        transitions,
    })
}

/// A graph bound to its skeleton and clips.
#[derive(Debug)]
pub struct Rig {
    pub def: GraphDef,
    pub skeleton: Skeleton,
    clips: Vec<Bound>,
    by_name: BTreeMap<String, usize>,
    meta: Vec<Option<usize>>,
    /// Per layer, per joint, how much the layer covers it.
    weights: Vec<Vec<f32>>,
    root: Option<usize>,
    /// The root's parent in the file's space at rest (root motion is taken in that space).
    root_parent: Xform,
    sockets: Vec<(String, usize)>,
}

impl Rig {
    /// Binds `def` to `sets` (by path: its skeleton's file and its libraries).
    pub fn new(def: GraphDef, sets: &[(String, ClipSet)]) -> Result<Rig, String> {
        let skeleton = sets
            .iter()
            .find(|(p, _)| *p == def.skeleton)
            .map(|(_, s)| s.skeleton.clone())
            .ok_or(format!("its skeleton {} is not loaded", def.skeleton))?;
        let mut clips = Vec::new();
        let mut by_name = BTreeMap::new();
        for name in def.clip_names() {
            let found = def
                .libraries
                .iter()
                .chain(std::iter::once(&def.skeleton))
                .find_map(|lib| {
                    let i = sets.iter().position(|(p, _)| p == lib)?;
                    let c = sets[i].1.clips.iter().position(|c| c.name == name)?;
                    Some((i, c))
                });
            let Some((i, c)) = found else {
                return Err(format!("no library has its clip {name}"));
            };
            by_name.insert(name, clips.len());
            clips.push(Bound::new(
                &skeleton,
                &sets[i].1,
                i,
                c,
                &def.translate,
                &def.scale_by,
            ));
        }
        let meta = clips
            .iter()
            .map(|b| def.clips.iter().position(|m| m.name == b.name))
            .collect();
        let n = skeleton.joints.len();
        let mut weights = Vec::new();
        for l in &def.layers {
            let w = match &l.mask {
                None => vec![1.0; n],
                Some(m) => {
                    let entries = &def
                        .masks
                        .iter()
                        .find(|(name, _)| name == m)
                        .expect("parse checks masks")
                        .1;
                    let mut w = vec![0.0f32; n];
                    let mut set = vec![false; n];
                    // Parents first, so an entry deeper in the tree overrides one above it.
                    for &j in &skeleton.order {
                        let j = j as usize;
                        if let Some(&(_, wt)) = entries
                            .iter()
                            .rev()
                            .find(|(name, _)| *name == skeleton.joints[j].name)
                        {
                            w[j] = wt;
                            set[j] = true;
                        } else if let Some(p) = skeleton.joints[j].parent.map(|p| p as usize)
                            && p < n
                            && set[p]
                        {
                            w[j] = w[p];
                            set[j] = true;
                        }
                    }
                    w
                }
            };
            weights.push(w);
        }
        let root = skeleton.find(&def.root);
        let root_parent = match root.and_then(|r| skeleton.joints[r].parent) {
            Some(p) => skeleton.global(&skeleton.rest(), p as usize),
            None => Xform::IDENTITY,
        };
        let mut sockets = Vec::new();
        for s in &def.sockets {
            let j = skeleton
                .find(s)
                .ok_or(format!("its socket {s} is no joint"))?;
            sockets.push((s.clone(), j));
        }
        Ok(Rig {
            def,
            skeleton,
            clips,
            by_name,
            meta,
            weights,
            root,
            root_parent,
            sockets,
        })
    }

    fn clip(&self, name: &str) -> Option<usize> {
        self.by_name.get(name).copied()
    }

    fn meta(&self, clip: usize) -> Option<&ClipMeta> {
        self.meta[clip].map(|m| &self.def.clips[m])
    }

    pub fn duration(&self, name: &str) -> f64 {
        self.clip(name)
            .map_or(0.0, |c| f64::from(self.clips[c].duration))
    }

    fn machine(&self, layer: usize) -> Option<(&[StateDef], &[TransitionDef])> {
        match &self.def.layers[layer].kind {
            LayerKind::Machine {
                states,
                transitions,
                ..
            } => Some((states, transitions)),
            LayerKind::Slot(_) => None,
        }
    }

    fn montage(&self, name: &str) -> Option<&MontageDef> {
        self.def.montages.iter().find(|m| m.name == name)
    }

    /// A fresh state: parameters at their defaults, each machine in its start state.
    pub fn initial(&self, graph: &str) -> AnimState {
        AnimState {
            graph: graph.to_owned(),
            params: self
                .def
                .params
                .iter()
                .map(|(n, v)| AnimParam {
                    name: n.clone(),
                    value: *v,
                })
                .collect(),
            layers: self
                .def
                .layers
                .iter()
                .map(|l| match &l.kind {
                    LayerKind::Machine { start, .. } => LayerState {
                        state: start.clone(),
                        ..LayerState::default()
                    },
                    LayerKind::Slot(s) => LayerState {
                        state: format!("slot:{s}"),
                        ..LayerState::default()
                    },
                })
                .collect(),
            slots: self
                .def
                .layers
                .iter()
                .filter_map(|l| match &l.kind {
                    LayerKind::Slot(s) => Some(SlotState {
                        slot: s.clone(),
                        ..SlotState::default()
                    }),
                    LayerKind::Machine { .. } => None,
                })
                .collect(),
            ..AnimState::default()
        }
    }

    /// The model-space transforms of the graph's sockets for a pose.
    pub fn sockets(&self, pose: &[Xform]) -> Vec<(String, Xform)> {
        self.sockets
            .iter()
            .map(|(n, j)| (n.clone(), self.skeleton.global(pose, *j)))
            .collect()
    }
}

/// A notify crossed in a tick: its name, its clip and `at`, `begin` or `end`.
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub name: String,
    pub clip: String,
    pub phase: &'static str,
}

/// What one tick of a graph gives: the pose (local transforms, the skeleton's order), the notifies
/// crossed, and the root's horizontal motion in the model's space.
#[derive(Clone, Debug, Default)]
pub struct Output {
    pub pose: Vec<Xform>,
    pub notices: Vec<Notice>,
    pub root_motion: V3,
}

fn param(st: &AnimState, name: &str) -> f64 {
    st.params
        .iter()
        .find(|p| p.name == name)
        .map_or(0.0, |p| p.value)
}

fn holds(c: &Cond, st: &AnimState, ended: bool) -> bool {
    match c {
        Cond::End => ended,
        Cond::Param { name, op, value } => {
            let v = param(st, name);
            match op.as_str() {
                "==" => v == *value,
                "!=" => v != *value,
                "<" => v < *value,
                "<=" => v <= *value,
                ">" => v > *value,
                _ => v >= *value,
            }
        }
    }
}

/// A clip's sampling time: wrapped when looping, held at its ends otherwise.
fn clip_time(duration: f64, t: f64, looped: bool) -> f64 {
    if duration <= 0.0 {
        0.0
    } else if looped {
        t.rem_euclid(duration)
    } else {
        t.clamp(0.0, duration)
    }
}

/// The weights of a 1D blend's points at `x`: the two around it, linearly.
fn blend1(points: &[(f64, String)], x: f64) -> Vec<(usize, f64)> {
    if x <= points[0].0 || points.len() == 1 {
        return vec![(0, 1.0)];
    }
    let n = points.len();
    if x >= points[n - 1].0 {
        return vec![(n - 1, 1.0)];
    }
    let i = points
        .iter()
        .rposition(|p| p.0 <= x)
        .unwrap_or(0)
        .min(n - 2);
    let (a, b) = (points[i].0, points[i + 1].0);
    let u = if b > a { (x - a) / (b - a) } else { 0.0 };
    vec![(i, 1.0 - u), (i + 1, u)]
}

/// The weights of a 2D blend's points at `(x, y)`: gradient band interpolation (each point's
/// influence falls off along the line to every other point), normalized.
fn blend2(points: &[(f64, f64, String)], x: f64, y: f64) -> Vec<(usize, f64)> {
    let mut w: Vec<f64> = points
        .iter()
        .enumerate()
        .map(|(i, pi)| {
            let mut m: f64 = 1.0;
            for (j, pj) in points.iter().enumerate() {
                if i == j {
                    continue;
                }
                let (dx, dy) = (pj.0 - pi.0, pj.1 - pi.1);
                let len2 = dx * dx + dy * dy;
                if len2 <= 0.0 {
                    continue;
                }
                let h = 1.0 - ((x - pi.0) * dx + (y - pi.1) * dy) / len2;
                m = min(m, h.clamp(0.0, 1.0));
            }
            m
        })
        .collect();
    let sum: f64 = w.iter().sum();
    if sum <= 0.0 {
        // Outside every band: the nearest point.
        let near = points
            .iter()
            .enumerate()
            .min_by(|a, b| {
                let da = (a.1.0 - x) * (a.1.0 - x) + (a.1.1 - y) * (a.1.1 - y);
                let db = (b.1.0 - x) * (b.1.0 - x) + (b.1.1 - y) * (b.1.1 - y);
                da.total_cmp(&db)
            })
            .map_or(0, |p| p.0);
        return vec![(near, 1.0)];
    }
    w.iter_mut().for_each(|v| *v /= sum);
    w.into_iter()
        .enumerate()
        .filter(|(_, v)| *v > 0.0)
        .collect()
}

/// One sample of a machine state: its clips with weights, at a time.
struct Sampled {
    /// (clip, seconds into it, weight)
    parts: Vec<(usize, f64, f64)>,
}

fn sample_state(rig: &Rig, st: &AnimState, s: &StateDef, time: f64) -> Sampled {
    let parts = match &s.motion {
        Motion::Clip(c) => rig
            .clip(c)
            .map(|c| {
                vec![(
                    c,
                    clip_time(f64::from(rig.clips[c].duration), time, s.looped),
                    1.0,
                )]
            })
            .unwrap_or_default(),
        Motion::Blend1 { param: p, points } => blend1(points, param(st, p))
            .into_iter()
            .filter_map(|(i, w)| {
                let c = rig.clip(&points[i].1)?;
                let d = f64::from(rig.clips[c].duration);
                Some((c, phase_time(d, time, s.looped), w))
            })
            .collect(),
        Motion::Blend2 { x, y, points } => blend2(points, param(st, x), param(st, y))
            .into_iter()
            .filter_map(|(i, w)| {
                let c = rig.clip(&points[i].2)?;
                let d = f64::from(rig.clips[c].duration);
                Some((c, phase_time(d, time, s.looped), w))
            })
            .collect(),
    };
    Sampled { parts }
}

/// A blend space's clips advance by a shared phase (`time` counts cycles): this clip's seconds.
fn phase_time(duration: f64, phase: f64, looped: bool) -> f64 {
    let p = if looped {
        phase.rem_euclid(1.0)
    } else {
        phase.clamp(0.0, 1.0)
    };
    p * duration
}

/// How long one cycle of a state lasts (a blend space: its clips' durations weighted).
fn cycle(rig: &Rig, sampled: &Sampled) -> f64 {
    let (mut d, mut w) = (0.0, 0.0);
    for &(c, _, wt) in &sampled.parts {
        d += f64::from(rig.clips[c].duration) * wt;
        w += wt;
    }
    if w > 0.0 { d / w } else { 0.0 }
}

fn is_blend(s: &StateDef) -> bool {
    !matches!(s.motion, Motion::Clip(_))
}

/// Writes a sampled state's pose (its clips weighted) over the rest.
fn pose_of(rig: &Rig, sets: &[ClipSet], sampled: &Sampled, lock_root: bool) -> Vec<Xform> {
    let rest = rig.skeleton.rest();
    let mut out = rest.clone();
    let mut acc = 0.0f64;
    let mut buf = rest.clone();
    for &(c, t, w) in &sampled.parts {
        if w <= 0.0 {
            continue;
        }
        buf.copy_from_slice(&rest);
        rig.clips[c].sample(sets, t as f32, &mut buf);
        if lock_root && rig.meta(c).is_some_and(|m| m.root_motion) {
            lock(rig, sets, c, &mut buf);
        }
        acc += w;
        let u = (w / acc) as f32;
        for (o, b) in out.iter_mut().zip(&buf) {
            *o = if acc == w { *b } else { Xform::mix(o, b, u) };
        }
    }
    out
}

/// Holds a root-motion clip's root at its horizontal place at the clip's start (the body moves
/// instead).
fn lock(rig: &Rig, sets: &[ClipSet], clip: usize, pose: &mut [Xform]) {
    let Some(r) = rig.root else { return };
    let Some(start) = rig.clips[clip].translation(sets, r, 0.0) else {
        return;
    };
    let p = &rig.root_parent;
    let now = p.apply(pose[r].t);
    let at0 = p.apply(start);
    let target = [at0[0], now[1], at0[2]];
    // Back into the parent's space: inverse rotation and scale of the parent.
    let d = math::rotate(math::qconj(p.r), math::sub(target, p.t));
    pose[r].t = [d[0] / p.s[0], d[1] / p.s[1], d[2] / p.s[2]];
}

/// The root's horizontal motion (model space) of `clip` from `t0` to `t1` (seconds, `t1 >= t0`),
/// wrapping a looped clip.
fn root_delta(rig: &Rig, sets: &[ClipSet], clip: usize, t0: f64, t1: f64, looped: bool) -> V3 {
    let Some(r) = rig.root else { return [0.0; 3] };
    if !rig.meta(clip).is_some_and(|m| m.root_motion) {
        return [0.0; 3];
    }
    let b = &rig.clips[clip];
    let d = f64::from(b.duration);
    let at = |t: f64| -> V3 {
        let local = b.translation(sets, r, t as f32).unwrap_or([0.0; 3]);
        rig.root_parent.apply(local)
    };
    let span = |a: f64, z: f64| -> V3 {
        let (pa, pz) = (at(a), at(z));
        [pz[0] - pa[0], 0.0, pz[2] - pa[2]]
    };
    if !looped || d <= 0.0 {
        return span(t0.clamp(0.0, max(d, 0.0)), t1.clamp(0.0, max(d, 0.0)));
    }
    let (w0, w1) = (t0.rem_euclid(d), t1.rem_euclid(d));
    let laps = ((t1 - t0) / d).floor();
    if w1 >= w0 && laps == 0.0 {
        span(w0, w1)
    } else {
        let mut v = math::add(span(w0, d), span(0.0, w1));
        let whole = span(0.0, d);
        for _ in 0..(laps as i64).max(0) {
            v = math::add(v, whole);
        }
        v
    }
}

/// The notifies of `clip` crossed going from `t0` to `t1` (seconds into the clip, `t1 >= t0`).
fn crossed(rig: &Rig, clip: usize, t0: f64, t1: f64, looped: bool, out: &mut Vec<Notice>) {
    let Some(meta) = rig.meta(clip) else { return };
    if meta.notifies.is_empty() || t1 <= t0 && t0 != 0.0 {
        return;
    }
    let d = f64::from(rig.clips[clip].duration);
    // The spans of clip time passed, a looped clip's split at its end.
    let mut spans = Vec::new();
    if looped && d > 0.0 {
        let (mut a, end) = (t0, t1);
        while a < end {
            let lap = (a / d).floor() * d;
            let b = min(end, lap + d);
            spans.push((a - lap, b - lap));
            a = b;
        }
    } else {
        spans.push((t0, min(t1, max(d, t0))));
    }
    for (a, b) in spans {
        for n in &meta.notifies {
            // A time is crossed when a < at <= b; the first tick (a == 0) includes 0.
            let hit = |at: f64| (at > a || a == 0.0 && at == 0.0) && at <= b;
            match n.to {
                None if hit(n.from) => out.push(Notice {
                    name: n.name.clone(),
                    clip: meta.name.clone(),
                    phase: "at",
                }),
                Some(to) => {
                    if hit(n.from) {
                        out.push(Notice {
                            name: n.name.clone(),
                            clip: meta.name.clone(),
                            phase: "begin",
                        });
                    }
                    if hit(to) {
                        out.push(Notice {
                            name: n.name.clone(),
                            clip: meta.name.clone(),
                            phase: "end",
                        });
                    }
                }
                None => {}
            }
        }
    }
}

/// The windows of `clip` open at `t` (left by a fade or a jump): their `end`s.
fn close_windows(rig: &Rig, clip: usize, t: f64, looped: bool, out: &mut Vec<Notice>) {
    let Some(meta) = rig.meta(clip) else { return };
    let d = f64::from(rig.clips[clip].duration);
    let t = clip_time(d, t, looped);
    for n in &meta.notifies {
        if let Some(to) = n.to
            && n.from < t
            && t < to
        {
            out.push(Notice {
                name: n.name.clone(),
                clip: meta.name.clone(),
                phase: "end",
            });
        }
    }
}

fn section<'a>(m: &'a MontageDef, name: &str) -> Option<&'a SectionDef> {
    m.sections.iter().find(|s| s.name == name)
}

/// Applies the commands queued by scripts.
fn commands(rig: &Rig, st: &mut AnimState, out: &mut Vec<Notice>) {
    let cmds = std::mem::take(&mut st.commands);
    for c in cmds {
        match c.op.as_str() {
            "play" => {
                let Some(m) = rig.montage(&c.montage) else {
                    continue;
                };
                let Some(first) = section(m, &c.section).or(m.sections.first()) else {
                    continue;
                };
                let Some(slot) = st.slots.iter_mut().find(|s| s.slot == m.slot) else {
                    continue;
                };
                let busy = !slot.montage.is_empty();
                slot.from_clip.clear();
                if busy
                    && let Some(cur) = rig
                        .montage(&slot.montage)
                        .and_then(|cm| section(cm, &slot.section))
                    && let Some(cc) = rig.clip(&cur.clip)
                {
                    if slot.out < 0.0 {
                        close_windows(rig, cc, slot.time, false, out);
                    }
                    slot.from_weight = slot_weight(rig, slot);
                    slot.from_clip = cur.clip.clone();
                    slot.from_time = slot.time;
                }
                slot.montage = c.montage.clone();
                slot.section = first.name.clone();
                slot.next = first.next.clone().unwrap_or_default();
                slot.time = 0.0;
                slot.age = 0.0;
                slot.out = -1.0;
                slot.rate = if c.rate > 0.0 { c.rate } else { 1.0 };
                slot.fade_out = m.fade_out;
            }
            "next" | "jump" => {
                let playing = |x: &SlotState| {
                    !x.montage.is_empty()
                        && x.out < 0.0
                        && rig
                            .montage(&x.montage)
                            .is_some_and(|m| section(m, &c.section).is_some())
                };
                let Some(slot) = st.slots.iter_mut().find(|x| playing(x)) else {
                    continue;
                };
                if c.op == "next" {
                    slot.next = c.section.clone();
                    continue;
                }
                let m = rig.montage(&slot.montage).expect("checked");
                if let Some(cc) = section(m, &slot.section).and_then(|cur| rig.clip(&cur.clip)) {
                    close_windows(rig, cc, slot.time, false, out);
                }
                let to = section(m, &c.section).expect("checked");
                slot.section = to.name.clone();
                slot.next = to.next.clone().unwrap_or_default();
                slot.time = 0.0;
            }
            "stop" => {
                for slot in st
                    .slots
                    .iter_mut()
                    .filter(|x| !x.montage.is_empty() && x.out < 0.0)
                {
                    if let Some(cc) = rig
                        .montage(&slot.montage)
                        .and_then(|m| section(m, &slot.section))
                        .and_then(|s| rig.clip(&s.clip))
                    {
                        close_windows(rig, cc, slot.time, false, out);
                    }
                    slot.out = 0.0;
                    slot.fade_out = max(c.fade, 0.0);
                }
            }
            _ => {}
        }
    }
}

/// A slot's weight: fading in, held, or fading out.
fn slot_weight(rig: &Rig, s: &SlotState) -> f64 {
    if s.montage.is_empty() {
        return 0.0;
    }
    let fade_in = rig.montage(&s.montage).map_or(0.0, |m| m.fade_in);
    let w_in = if fade_in > 0.0 {
        min(s.age / fade_in, 1.0)
    } else {
        1.0
    };
    let w_out = if s.out < 0.0 {
        1.0
    } else if s.fade_out > 0.0 {
        max(1.0 - s.out / s.fade_out, 0.0)
    } else {
        0.0
    };
    min(w_in, w_out)
}

/// One tick of `dt` seconds of a graph's state; returns the pose, the notifies and the root motion.
pub fn step(rig: &Rig, sets: &[ClipSet], st: &mut AnimState, dt: f64) -> Output {
    let mut out = Output::default();
    commands(rig, st, &mut out.notices);
    let mut pose = rig.skeleton.rest();
    let mut root = [0.0f32; 3];
    for (li, layer) in rig.def.layers.iter().enumerate() {
        let weights = &rig.weights[li];
        match &layer.kind {
            LayerKind::Machine { .. } => {
                let (states, transitions) = rig.machine(li).expect("a machine");
                let ls = &mut st.layers[li];
                let Some(cur) = states.iter().position(|s| s.name == ls.state) else {
                    ls.state = states[0].name.clone();
                    continue;
                };
                // A transition from the current state, or from any.
                let sampled_now = sample_state(rig, st, &states[cur], st.layers[li].time);
                let ended = {
                    let t = st.layers[li].time;
                    if is_blend(&states[cur]) {
                        t >= 1.0
                    } else {
                        t >= cycle(rig, &sampled_now)
                    }
                };
                let next = transitions.iter().find(|t| {
                    (t.from == "*" && t.to != states[cur].name || t.from == states[cur].name)
                        && t.when.iter().all(|c| holds(c, st, ended))
                });
                if let Some(t) = next {
                    let to = states
                        .iter()
                        .position(|s| s.name == t.to)
                        .expect("parse checks states");
                    let ls = &st.layers[li];
                    if !is_blend(&states[cur])
                        && let Some((c, _, _)) = sampled_now.parts.first()
                    {
                        close_windows(rig, *c, ls.time, states[cur].looped, &mut out.notices);
                    }
                    let (from_state, from_time) = (states[cur].name.clone(), ls.time);
                    let ls = &mut st.layers[li];
                    ls.from = from_state;
                    ls.from_time = from_time;
                    ls.state = states[to].name.clone();
                    ls.time = 0.0;
                    ls.fade = 0.0;
                    ls.fade_len = t.fade;
                }
                let cur = states
                    .iter()
                    .position(|s| s.name == st.layers[li].state)
                    .expect("a state");
                let s = &states[cur];
                // Advance: a clip by seconds, a blend space by its cycle's share.
                let before = sample_state(rig, st, s, st.layers[li].time);
                let t0 = st.layers[li].time;
                let t1 = if is_blend(s) {
                    let c = cycle(rig, &before);
                    t0 + if c > 0.0 { dt * s.rate / c } else { 0.0 }
                } else {
                    t0 + dt * s.rate
                };
                st.layers[li].time = t1;
                if st.layers[li].fade_len > 0.0 {
                    st.layers[li].fade += dt;
                }
                let now = sample_state(rig, st, s, t1);
                // Notifies and root motion of the dominant clip (the heaviest of a blend space).
                if let Some(&(c, _, _)) = now.parts.iter().max_by(|a, b| a.2.total_cmp(&b.2)) {
                    let d = f64::from(rig.clips[c].duration);
                    let (a, b) = if is_blend(s) {
                        (t0 * d, t1 * d)
                    } else {
                        (t0, t1)
                    };
                    crossed(rig, c, a, b, s.looped, &mut out.notices);
                    let w = layer_share(&st.layers[li]);
                    let dv = root_delta(rig, sets, c, a, b, s.looped);
                    let k =
                        (w * f64::from(weights.get(rig.root.unwrap_or(0)).copied().unwrap_or(0.0)))
                            as f32;
                    root = math::add(root, math::scale(dv, k));
                }
                let mut lp = pose_of(rig, sets, &now, true);
                let ls = &st.layers[li];
                if ls.fade_len > 0.0 && ls.fade < ls.fade_len && !ls.from.is_empty() {
                    if let Some(f) = states.iter().find(|x| x.name == ls.from) {
                        let from =
                            pose_of(rig, sets, &sample_state(rig, st, f, ls.from_time), true);
                        let u = (ls.fade / ls.fade_len) as f32;
                        for (o, a) in lp.iter_mut().zip(&from) {
                            *o = Xform::mix(a, o, u);
                        }
                    }
                } else if ls.fade_len > 0.0 {
                    let ls = &mut st.layers[li];
                    ls.from.clear();
                    ls.fade_len = 0.0;
                    ls.fade = 0.0;
                }
                over(&mut pose, &lp, weights, 1.0);
            }
            LayerKind::Slot(name) => {
                let Some(si) = st.slots.iter().position(|s| &s.slot == name) else {
                    continue;
                };
                if st.slots[si].montage.is_empty() {
                    continue;
                }
                let slot = &mut st.slots[si];
                let Some(m) = rig.montage(&slot.montage) else {
                    slot.montage.clear();
                    continue;
                };
                // Advance through sections.
                let mut left = dt * slot.rate;
                slot.age += dt;
                if slot.out >= 0.0 {
                    slot.out += dt;
                    left = 0.0;
                }
                let mut guard = 0;
                while left > 0.0 && guard < 16 {
                    guard += 1;
                    let Some(sec) = section(m, &slot.section) else {
                        break;
                    };
                    let Some(c) = rig.clip(&sec.clip) else { break };
                    let d = f64::from(rig.clips[c].duration);
                    let t0 = slot.time;
                    let t1 = t0 + left * sec.rate;
                    if t1 < d {
                        crossed(rig, c, t0, t1, false, &mut out.notices);
                        let dv = root_delta(rig, sets, c, t0, t1, false);
                        root = math::add(
                            root,
                            math::scale(
                                dv,
                                slot_weight(rig, slot) as f32
                                    * weights.get(rig.root.unwrap_or(0)).copied().unwrap_or(0.0),
                            ),
                        );
                        slot.time = t1;
                        left = 0.0;
                    } else {
                        crossed(rig, c, t0, d, false, &mut out.notices);
                        let dv = root_delta(rig, sets, c, t0, d, false);
                        root = math::add(
                            root,
                            math::scale(
                                dv,
                                slot_weight(rig, slot) as f32
                                    * weights.get(rig.root.unwrap_or(0)).copied().unwrap_or(0.0),
                            ),
                        );
                        left -= if sec.rate > 0.0 {
                            (d - t0) / sec.rate
                        } else {
                            left
                        };
                        match section(m, &slot.next).filter(|_| !slot.next.is_empty()) {
                            Some(n) => {
                                slot.section = n.name.clone();
                                slot.next = n.next.clone().unwrap_or_default();
                                slot.time = 0.0;
                            }
                            None => {
                                slot.time = d;
                                slot.out = 0.0;
                                slot.fade_out = m.fade_out;
                                left = 0.0;
                            }
                        }
                    }
                }
                let w = slot_weight(rig, slot);
                if slot.out >= 0.0 && w <= 0.0 {
                    slot.montage.clear();
                    slot.section.clear();
                    slot.from_clip.clear();
                    continue;
                }
                let sec = section(m, &slot.section);
                let Some(c) = sec.and_then(|s| rig.clip(&s.clip)) else {
                    continue;
                };
                let mut lp = pose_of(
                    rig,
                    sets,
                    &Sampled {
                        parts: vec![(c, slot.time, 1.0)],
                    },
                    true,
                );
                // A montage that interrupted another fades from its frozen pose.
                let fade_in = m.fade_in;
                if !slot.from_clip.is_empty() {
                    if let Some(fc) = rig.clip(&slot.from_clip)
                        && fade_in > 0.0
                        && slot.age < fade_in
                    {
                        let from = pose_of(
                            rig,
                            sets,
                            &Sampled {
                                parts: vec![(fc, slot.from_time, 1.0)],
                            },
                            true,
                        );
                        let u = (slot.age / fade_in) as f32;
                        for (o, a) in lp.iter_mut().zip(&from) {
                            *o = Xform::mix(a, o, u);
                        }
                    } else {
                        slot.from_clip.clear();
                    }
                }
                // With a frozen pose to fade from, the slot holds its whole weight while fading in.
                let w = if slot.from_clip.is_empty() {
                    w
                } else {
                    max(slot.from_weight, w)
                };
                over(&mut pose, &lp, weights, w as f32);
            }
        }
    }
    out.pose = pose;
    out.root_motion = root;
    out
}

/// The share of a machine layer's current state while it fades in (the root motion of a fading
/// state counts by it).
fn layer_share(ls: &LayerState) -> f64 {
    if ls.fade_len > 0.0 && !ls.from.is_empty() {
        (ls.fade / ls.fade_len).clamp(0.0, 1.0)
    } else {
        1.0
    }
}

/// `layer` over `pose` by each joint's weight times `w`.
fn over(pose: &mut [Xform], layer: &[Xform], weights: &[f32], w: f32) {
    for ((p, l), m) in pose.iter_mut().zip(layer).zip(weights) {
        let u = m * w;
        if u >= 1.0 {
            *p = *l;
        } else if u > 0.0 {
            *p = Xform::mix(p, l, u);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_1d_blend_weighs_the_two_points_around_it() {
        let pts = vec![
            (0.0, "a".to_owned()),
            (2.0, "b".to_owned()),
            (6.0, "c".to_owned()),
        ];
        assert_eq!(blend1(&pts, -1.0), vec![(0, 1.0)]);
        assert_eq!(blend1(&pts, 1.0), vec![(0, 0.5), (1, 0.5)]);
        assert_eq!(blend1(&pts, 5.0), vec![(1, 0.25), (2, 0.75)]);
        assert_eq!(blend1(&pts, 9.0), vec![(2, 1.0)]);
    }

    #[test]
    fn a_2d_blend_is_the_point_itself_at_a_point_and_sums_to_one_between() {
        let pts = vec![
            (0.0, 0.0, "idle".to_owned()),
            (0.0, 1.0, "fwd".to_owned()),
            (1.0, 0.0, "right".to_owned()),
            (-1.0, 0.0, "left".to_owned()),
        ];
        assert_eq!(blend2(&pts, 0.0, 1.0), vec![(1, 1.0)]);
        let w = blend2(&pts, 0.5, 0.5);
        let sum: f64 = w.iter().map(|p| p.1).sum();
        assert!((sum - 1.0).abs() < 1e-12, "{w:?}");
        assert!(
            w.iter().all(|p| p.0 != 3),
            "the left point plays no part on the right: {w:?}"
        );
    }

    #[test]
    fn a_graph_file_parses_and_refuses_what_is_wrong() {
        let ok: Value = serde_json::from_str(r#"{
            "format": "amoris-animgraph", "version": 1, "skeleton": "a.glb", "libraries": [],
            "params": {"speed": 0, "dead": false},
            "clips": {"Swing": {"root_motion": true, "notifies": [{"name": "hit", "from": 0.1, "to": 0.2}, {"name": "whoosh", "at": 0.05}]}},
            "masks": {"upper": [{"joint": "spine", "weight": 0.5}]},
            "layers": [
              {"name": "base", "start": "move", "states": {
                "move": {"blend1d": {"param": "speed", "points": [[0, "Idle"], [3, "Run"]]}},
                "dead": {"clip": "Die", "loop": false}},
               "transitions": [{"from": "*", "to": "dead", "when": [["dead", "==", true]], "fade": 0.3}]},
              {"name": "upper", "mask": "upper", "slot": "upper"}],
            "montages": {"attack": {"slot": "upper", "sections": [{"name": "a", "clip": "Swing", "next": "b"}, {"name": "b", "clip": "Swing"}]}}
        }"#).unwrap();
        let g = GraphDef::parse(&ok).unwrap();
        assert_eq!(g.params, vec![("dead".into(), 0.0), ("speed".into(), 0.0)]);
        // States come in their names' order (a JSON object's keys), so every host reads one order.
        assert_eq!(g.clip_names(), vec!["Die", "Idle", "Run", "Swing"]);
        assert_eq!(g.clips[0].notifies[0].to, Some(0.2));
        let mut bad = ok.clone();
        bad["montages"]["attack"]["slot"] = "legs".into();
        assert!(GraphDef::parse(&bad).unwrap_err().contains("slot legs"));
        let mut bad = ok.clone();
        bad["layers"][0]["transitions"][0]["to"] = "flying".into();
        assert!(GraphDef::parse(&bad).unwrap_err().contains("flying"));
    }
}
