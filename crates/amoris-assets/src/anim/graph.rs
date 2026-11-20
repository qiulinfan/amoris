//! Animation graphs (animation.md 3): a file's parameters, clips' metadata, masks, layers of state
//! machines and slots, and montages; a graph bound to its skeleton and clips (`Rig`); and one tick
//! of a graph's state (`step`): transitions and fades, blend spaces, montages and their sections,
//! notifies, root motion and the pose; additive layers and inertialized switches (animation.md
//! 3a).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use amoris_sim::math::{clamp, max, min};

use super::clip::{Bound, ClipSet, Skeleton};
use super::ik::{IkDef, IkNode};
use super::math::{self, IDENTITY, V3, Xform};
use super::ragdoll::{RagdollDef, RagdollRig};
use super::state::{AnimParam, AnimState, LayerState, SlotState};

/// A weight: a number, or a graph parameter read each tick; either clamped to 0..1 (an IK node's,
/// a layer's).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ParamWeight {
    Value(f64),
    Param(String),
}

impl Default for ParamWeight {
    fn default() -> Self {
        ParamWeight::Value(1.0)
    }
}

impl ParamWeight {
    /// Parses `v` (none: 1); a name must be one of `params`. `what` names the owner in errors.
    pub fn parse(
        v: Option<&Value>,
        params: &[(String, f64)],
        what: &str,
    ) -> Result<ParamWeight, String> {
        match v {
            None => Ok(ParamWeight::Value(1.0)),
            Some(Value::String(p)) => {
                if !params.iter().any(|(n, _)| n == p) {
                    return Err(format!("{what}'s weight names no parameter {p}"));
                }
                Ok(ParamWeight::Param(p.clone()))
            }
            Some(w) => Ok(ParamWeight::Value(num(w, &format!("{what}'s weight"))?)),
        }
    }

    /// Its value for `st`, clamped to 0..1.
    pub fn value(&self, st: &AnimState) -> f64 {
        let v = match self {
            ParamWeight::Value(v) => *v,
            ParamWeight::Param(p) => param(st, p),
        };
        clamp(v, 0.0, 1.0)
    }
}

/// How a layer goes over the pose below it (animation.md 3a).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayerBlend {
    /// Its pose replaces the one below by its weights.
    #[default]
    Override,
    /// Its clips play as differences from their references, added to the pose below.
    Add,
}

/// The pose an additive clip is the difference from.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Reference {
    /// The clip itself at 0 s.
    #[default]
    First,
    /// The skeleton's rest pose.
    Rest,
    /// Another clip's pose at a time (seconds).
    Clip { clip: String, at: f64 },
}

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
    /// The switch is inertialized over `fade` instead of crossfaded.
    #[serde(default)]
    pub inertial: bool,
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
    #[serde(default)]
    pub blend: LayerBlend,
    /// Multiplies the mask's weights (the base layer's is 1).
    #[serde(default)]
    pub weight: ParamWeight,
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
    /// What the clip is a difference from when an additive layer plays it.
    #[serde(default)]
    pub reference: Reference,
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
    /// It starts (and a jump within it switches) inertialized over `fade_in`, not faded.
    #[serde(default)]
    pub inertial: bool,
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
    /// The IK nodes that bend the pose after the layers, in order (animation.md 6a).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ik: Vec<IkDef>,
    /// The ragdoll's bodies and joints (animation.md 6b).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ragdoll: Option<RagdollDef>,
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
                    reference: match m.get("reference") {
                        None => Reference::First,
                        Some(Value::String(r)) if r == "first" => Reference::First,
                        Some(Value::String(r)) if r == "rest" => Reference::Rest,
                        Some(Value::Object(r)) => Reference::Clip {
                            clip: text(
                                r.get("clip").unwrap_or(&Value::Null),
                                &format!("clip {name}'s reference clip"),
                            )?,
                            at: opt_num(r, "at", 0.0, &format!("clip {name}'s reference"))?,
                        },
                        Some(_) => {
                            return Err(format!(
                                "clip {name}'s reference must be \"first\", \"rest\" or \
                                 {{\"clip\", \"at\"}}"
                            ));
                        }
                    },
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
            let blend = match l
                .get("blend")
                .map(|b| text(b, "a layer's blend"))
                .transpose()?
            {
                None => LayerBlend::Override,
                Some(b) if b == "override" => LayerBlend::Override,
                Some(b) if b == "add" => LayerBlend::Add,
                Some(b) => {
                    return Err(format!(
                        "layer {name}'s blend {b} is neither \"override\" nor \"add\""
                    ));
                }
            };
            if i == 0 && (blend != LayerBlend::Override || l.contains_key("weight")) {
                return Err("the base layer is neither additive nor weighted".into());
            }
            let weight = ParamWeight::parse(l.get("weight"), &g.params, &format!("layer {name}"))?;
            g.layers.push(LayerDef {
                name,
                mask,
                kind,
                blend,
                weight,
            });
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
                    inertial: inertial(m.get("blend_in"), &format!("montage {name}'s blend_in"))?,
                });
            }
        }
        if let Some(ik) = o.get("ik") {
            g.ik = IkDef::parse_list(ik, &g.params)?;
        }
        if let Some(r) = o.get("ragdoll") {
            g.ragdoll = Some(RagdollDef::parse(r)?);
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
        for c in &self.clips {
            if let Reference::Clip { clip, .. } = &c.reference {
                add(clip);
            }
        }
        out
    }
}

/// A switch's blend: `"fade"` (none given) or `"inertial"`.
fn inertial(v: Option<&Value>, what: &str) -> Result<bool, String> {
    match v.map(|b| text(b, what)).transpose()?.as_deref() {
        None | Some("fade") => Ok(false),
        Some("inertial") => Ok(true),
        Some(b) => Err(format!("{what} {b} is neither \"fade\" nor \"inertial\"")),
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
            inertial: inertial(t.get("blend"), "a transition's blend")?,
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
    /// Per clip, the pose an additive layer takes its difference from: a clip at a time (seconds),
    /// or none for the rest pose.
    refs: Vec<Option<(usize, f32)>>,
    /// Per layer, per joint, how much the layer covers it.
    weights: Vec<Vec<f32>>,
    /// Where each joint's morph weights start in a pose's flat weights, the total last.
    morph_at: Vec<usize>,
    root: Option<usize>,
    /// The root's parent in the file's space at rest (root motion is taken in that space).
    root_parent: Xform,
    sockets: Vec<(String, usize)>,
    /// The IK nodes bound to the skeleton (animation.md 6a).
    ik: Vec<IkNode>,
    /// The ragdoll bound to the skeleton (animation.md 6b).
    ragdoll: Option<RagdollRig>,
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
        let meta: Vec<Option<usize>> = clips
            .iter()
            .map(|b| def.clips.iter().position(|m| m.name == b.name))
            .collect();
        let refs = meta
            .iter()
            .enumerate()
            .map(|(c, m)| match m.map(|m| &def.clips[m].reference) {
                None | Some(Reference::First) => Some((c, 0.0)),
                Some(Reference::Rest) => None,
                Some(Reference::Clip { clip, at }) => Some((by_name[clip.as_str()], *at as f32)),
            })
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
        let morph_at = skeleton.morph_offsets();
        let ik = def
            .ik
            .iter()
            .map(|d| IkNode::bind(d, &skeleton))
            .collect::<Result<Vec<_>, _>>()?;
        let ragdoll = def
            .ragdoll
            .as_ref()
            .map(|r| RagdollRig::bind(r, &skeleton))
            .transpose()?;
        Ok(Rig {
            ragdoll,
            def,
            skeleton,
            clips,
            by_name,
            meta,
            refs,
            weights,
            morph_at,
            root,
            root_parent,
            sockets,
            ik,
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

    /// How many morph weights a pose holds (every target of the skeleton's meshes).
    pub fn morph_len(&self) -> usize {
        self.morph_at.last().copied().unwrap_or(0)
    }

    /// Each morph weight's target name and joint, in a pose's flat order.
    pub fn morph_targets(&self) -> impl Iterator<Item = (&str, usize)> {
        self.skeleton
            .joints
            .iter()
            .enumerate()
            .flat_map(|(j, x)| x.targets.iter().map(move |n| (n.as_str(), j)))
    }

    /// The graph's IK nodes, in order (animation.md 6a); empty for most graphs.
    pub fn ik(&self) -> &[IkNode] {
        &self.ik
    }

    /// The graph's ragdoll (animation.md 6b), if it has one.
    pub fn ragdoll(&self) -> Option<&RagdollRig> {
        self.ragdoll.as_ref()
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

/// What one tick of a graph gives: the pose (local transforms, the skeleton's order), its morph
/// weights (flat, the skeleton's order; animation.md 5a), the notifies crossed, and the root's
/// horizontal motion in the model's space.
#[derive(Clone, Debug, Default)]
pub struct Output {
    pub pose: Vec<Xform>,
    pub morph: Vec<f32>,
    pub notices: Vec<Notice>,
    pub root_motion: V3,
}

/// A pose while it is blended: the joints' transforms and the flat morph weights.
struct Pose {
    x: Vec<Xform>,
    w: Vec<f32>,
}

impl Pose {
    fn rest(rig: &Rig) -> Pose {
        Pose {
            x: rig.skeleton.rest(),
            w: rig.skeleton.rest_weights(),
        }
    }

    /// The identity difference: no translation, no turn, unit scale, no weights.
    fn identity(rig: &Rig) -> Pose {
        Pose {
            x: vec![Xform::IDENTITY; rig.skeleton.joints.len()],
            w: vec![0.0; rig.morph_len()],
        }
    }

    /// `self` becomes `from` mixed toward `self` by `u` (a fade's outgoing pose under it).
    fn fade_from(&mut self, from: &Pose, u: f32) {
        for (o, a) in self.x.iter_mut().zip(&from.x) {
            *o = Xform::mix(a, o, u);
        }
        for (o, a) in self.w.iter_mut().zip(&from.w) {
            *o = a + (*o - a) * u;
        }
    }
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

/// Writes a sampled state's pose (its clips weighted) over the rest; for an additive layer
/// (`add`), each clip's difference from its reference over the identity difference.
fn pose_of(rig: &Rig, sets: &[ClipSet], sampled: &Sampled, lock_root: bool, add: bool) -> Pose {
    let rest = Pose::rest(rig);
    let mut out = if add {
        Pose::identity(rig)
    } else {
        Pose {
            x: rest.x.clone(),
            w: rest.w.clone(),
        }
    };
    let mut acc = 0.0f64;
    let mut buf = rest.x.clone();
    let mut wbuf = rest.w.clone();
    for &(c, t, w) in &sampled.parts {
        if w <= 0.0 {
            continue;
        }
        buf.copy_from_slice(&rest.x);
        rig.clips[c].sample(sets, t as f32, &mut buf);
        if lock_root && !add && rig.meta(c).is_some_and(|m| m.root_motion) {
            lock(rig, sets, c, &mut buf);
        }
        if !wbuf.is_empty() {
            wbuf.copy_from_slice(&rest.w);
            rig.clips[c].sample_weights(sets, t as f32, &mut wbuf);
        }
        if add {
            difference(rig, sets, c, &rest, &mut buf, &mut wbuf);
        }
        acc += w;
        let u = (w / acc) as f32;
        for (o, b) in out.x.iter_mut().zip(&buf) {
            *o = if acc == w { *b } else { Xform::mix(o, b, u) };
        }
        for (o, b) in out.w.iter_mut().zip(&wbuf) {
            *o = if acc == w { *b } else { *o + (b - *o) * u };
        }
    }
    out
}

/// A sampled clip's pose (`x`, `w`) made its difference from the clip's reference (animation.md
/// 3a): `r * conj(r_ref)`, `t - t_ref`, `s / s_ref`, `w - w_ref`.
fn difference(
    rig: &Rig,
    sets: &[ClipSet],
    clip: usize,
    rest: &Pose,
    x: &mut [Xform],
    w: &mut [f32],
) {
    let mut rx = rest.x.clone();
    let mut rw = rest.w.clone();
    if let Some((rc, at)) = rig.refs[clip] {
        rig.clips[rc].sample(sets, at, &mut rx);
        if !rw.is_empty() {
            rig.clips[rc].sample_weights(sets, at, &mut rw);
        }
    }
    for (d, r) in x.iter_mut().zip(&rx) {
        let ratio = |a: f32, b: f32| if b != 0.0 { a / b } else { 1.0 };
        *d = Xform {
            t: math::sub(d.t, r.t),
            r: math::normalize(math::qmul(d.r, math::qconj(r.r))),
            s: [
                ratio(d.s[0], r.s[0]),
                ratio(d.s[1], r.s[1]),
                ratio(d.s[2], r.s[2]),
            ],
        };
    }
    for (d, r) in w.iter_mut().zip(&rw) {
        *d -= r;
    }
}

/// Values an inertialization keeps per joint (the rotation's vector and velocity, the
/// translation's and the scale's differences and velocities) and per morph weight.
const PER_JOINT: usize = 12;
const PER_WEIGHT: usize = 2;

fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The offsets of an inertialized switch from its target `to` to the source as it is now (`s0`)
/// and was a tick earlier (`s1`), `dt` seconds before (animation.md 3a).
fn offsets(s0: &Pose, s1: &Pose, to: &Pose, dt: f64) -> Vec<f32> {
    let dt = dt as f32;
    let speed = |x0: f32, x1: f32| if dt > 0.0 { (x0 - x1) / dt } else { 0.0 };
    // A direction's size now, and its velocity from the one a tick earlier along it.
    let along = |d0: V3, d1: V3| -> f32 {
        let x0 = math::length(d0);
        let x1 = if x0 > 0.0 { dot(d1, d0) / x0 } else { 0.0 };
        speed(x0, x1)
    };
    let mut out = Vec::with_capacity(to.x.len() * PER_JOINT + to.w.len() * PER_WEIGHT);
    for ((a, b), t) in s0.x.iter().zip(&s1.x).zip(&to.x) {
        let mut q = math::qmul(a.r, math::qconj(t.r));
        if q[3] < 0.0 {
            q = [-q[0], -q[1], -q[2], -q[3]];
        }
        let mut q1 = math::qmul(b.r, math::qconj(t.r));
        if q1[0] * q[0] + q1[1] * q[1] + q1[2] * q[2] + q1[3] * q[3] < 0.0 {
            q1 = [-q1[0], -q1[1], -q1[2], -q1[3]];
        }
        let (v, v1) = ([q[0], q[1], q[2]], [q1[0], q1[1], q1[2]]);
        out.extend_from_slice(&v);
        out.push(along(v, v1));
        let (d, d1) = (math::sub(a.t, t.t), math::sub(b.t, t.t));
        out.extend_from_slice(&d);
        out.push(along(d, d1));
        let (d, d1) = (math::sub(a.s, t.s), math::sub(b.s, t.s));
        out.extend_from_slice(&d);
        out.push(along(d, d1));
    }
    for ((a, b), t) in s0.w.iter().zip(&s1.w).zip(&to.w) {
        let d = a - t;
        let x1 = if d < 0.0 { t - b } else { b - t };
        out.push(d);
        out.push(speed(d.abs(), x1));
    }
    out
}

/// What is left at `t` of an offset `x0` that moves at `v0` and runs out over `t1` seconds:
/// Bollo's quintic, which never passes through zero (animation.md 3a).
pub fn decay(x0: f32, v0: f32, t: f32, t1: f32) -> f32 {
    if x0 <= 0.0 || t1 <= 0.0 || t >= t1 {
        return 0.0;
    }
    let v0 = if v0 > 0.0 { 0.0 } else { v0 };
    let t1 = if v0 < 0.0 && -5.0 * x0 / v0 < t1 {
        -5.0 * x0 / v0
    } else {
        t1
    };
    if t >= t1 {
        return 0.0;
    }
    let (t2, t3) = (t1 * t1, t1 * t1 * t1);
    let (t4, t5) = (t3 * t1, t3 * t2);
    let a0 = (-8.0 * v0 * t1 - 20.0 * x0) / t2;
    let a0 = if a0 > 0.0 { a0 } else { 0.0 };
    let a = -(a0 * t2 + 6.0 * v0 * t1 + 12.0 * x0) / (2.0 * t5);
    let b = (3.0 * a0 * t2 + 16.0 * v0 * t1 + 30.0 * x0) / (2.0 * t4);
    let c = -(3.0 * a0 * t2 + 12.0 * v0 * t1 + 20.0 * x0) / (2.0 * t3);
    let x = (((((a * t + b) * t + c) * t + 0.5 * a0) * t + v0) * t) + x0;
    if x < 0.0 {
        0.0
    } else if x > x0 {
        x0
    } else {
        x
    }
}

/// Lays an inertialization's offsets, `t` seconds into its `len`, on a layer's content.
fn lay(content: &mut Pose, off: &[f32], t: f64, len: f64) {
    let (t, len) = (t as f32, len as f32);
    for (j, x) in content.x.iter_mut().enumerate() {
        let o = &off[j * PER_JOINT..(j + 1) * PER_JOINT];
        let v: V3 = [o[0], o[1], o[2]];
        let x0 = math::length(v);
        let k = decay(x0, o[3], t, len);
        if k > 0.0 {
            let u = k / x0;
            let q = [
                v[0] * u,
                v[1] * u,
                v[2] * u,
                amoris_sim::math::max_f32(1.0 - k * k, 0.0).sqrt(),
            ];
            x.r = math::normalize(math::qmul(q, x.r));
        }
        for (at, field) in [(4, &mut x.t), (8, &mut x.s)] {
            let d: V3 = [o[at], o[at + 1], o[at + 2]];
            let x0 = math::length(d);
            let k = decay(x0, o[at + 3], t, len);
            if k > 0.0 {
                *field = math::add(*field, math::scale(d, k / x0));
            }
        }
    }
    let base = content.x.len() * PER_JOINT;
    for (i, w) in content.w.iter_mut().enumerate() {
        let (d, v0) = (off[base + i * PER_WEIGHT], off[base + i * PER_WEIGHT + 1]);
        let k = decay(d.abs(), v0, t, len);
        if k > 0.0 {
            *w += if d < 0.0 { -k } else { k };
        }
    }
}

/// The pose `a` mixed toward `b` by `u`.
fn mixed(a: &Pose, b: &Pose, u: f32) -> Pose {
    let mut out = Pose {
        x: b.x.clone(),
        w: b.w.clone(),
    };
    out.fade_from(a, u);
    out
}

/// A machine layer's content (what it lays over the pose below at full weight): its state at its
/// time, with its fade and its running offsets, as it was `back` seconds of the graph earlier.
fn machine_content(rig: &Rig, sets: &[ClipSet], st: &AnimState, li: usize, back: f64) -> Pose {
    let add = rig.def.layers[li].blend == LayerBlend::Add;
    let (states, _) = rig.machine(li).expect("a machine");
    let ls = &st.layers[li];
    let s = states.iter().find(|s| s.name == ls.state).expect("a state");
    let mut time = ls.time;
    if back > 0.0 {
        time -= if is_blend(s) {
            let c = cycle(rig, &sample_state(rig, st, s, time));
            if c > 0.0 { back * s.rate / c } else { 0.0 }
        } else {
            back * s.rate
        };
        time = max(time, 0.0);
    }
    let mut lp = pose_of(rig, sets, &sample_state(rig, st, s, time), true, add);
    let fade = max(ls.fade - back, 0.0);
    if ls.fade_len > 0.0
        && fade < ls.fade_len
        && !ls.from.is_empty()
        && let Some(f) = states.iter().find(|x| x.name == ls.from)
    {
        let from = pose_of(
            rig,
            sets,
            &sample_state(rig, st, f, ls.from_time),
            true,
            add,
        );
        lp.fade_from(&from, (fade / ls.fade_len) as f32);
    }
    if !ls.inertia.is_empty() {
        lay(
            &mut lp,
            &ls.inertia,
            max(ls.inertia_t - back, 0.0),
            ls.inertia_len,
        );
    }
    lp
}

/// What an inertial montage's start or jump switches from: the clip it left and its time and rate
/// (none when the slot held nothing), the share it had over the pose below, and its running
/// offsets.
struct SlotSource {
    from: Option<(usize, f64, f64)>,
    share: f64,
    inertia: Vec<f32>,
    inertia_t: f64,
    inertia_len: f64,
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

/// The clip a slot plays now, its time and its rate (the section's times the slot's).
fn slot_clip(rig: &Rig, slot: &SlotState) -> Option<(usize, f64, f64)> {
    let sec = rig
        .montage(&slot.montage)
        .and_then(|m| section(m, &slot.section))?;
    Some((rig.clip(&sec.clip)?, slot.time, sec.rate * slot.rate))
}

/// Applies the commands queued by scripts; returns, per slot, the source of an inertial montage's
/// start or jump made now (animation.md 3a).
fn commands(rig: &Rig, st: &mut AnimState, out: &mut Vec<Notice>) -> Vec<Option<SlotSource>> {
    let mut sources: Vec<Option<SlotSource>> = st.slots.iter().map(|_| None).collect();
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
                let Some(si) = st.slots.iter().position(|s| s.slot == m.slot) else {
                    continue;
                };
                let slot = &mut st.slots[si];
                let busy = !slot.montage.is_empty();
                if m.inertial {
                    // A second start in the same tick keeps the first one's source.
                    if sources[si].is_none() {
                        sources[si] = Some(SlotSource {
                            from: slot_clip(rig, slot).filter(|_| busy),
                            share: if busy { slot_weight(rig, slot) } else { 0.0 },
                            inertia: std::mem::take(&mut slot.inertia),
                            inertia_t: slot.inertia_t,
                            inertia_len: slot.inertia_len,
                        });
                    }
                    slot.inertia.clear();
                }
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
                let Some(si) = st.slots.iter().position(playing) else {
                    continue;
                };
                let slot = &mut st.slots[si];
                if c.op == "next" {
                    slot.next = c.section.clone();
                    continue;
                }
                let m = rig.montage(&slot.montage).expect("checked");
                if let Some(cc) = section(m, &slot.section).and_then(|cur| rig.clip(&cur.clip)) {
                    close_windows(rig, cc, slot.time, false, out);
                }
                if m.inertial && sources[si].is_none() {
                    sources[si] = Some(SlotSource {
                        from: slot_clip(rig, slot),
                        share: 1.0,
                        inertia: std::mem::take(&mut slot.inertia),
                        inertia_t: slot.inertia_t,
                        inertia_len: slot.inertia_len,
                    });
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
    sources
}

/// A slot's weight: fading in, held, or fading out.
fn slot_weight(rig: &Rig, s: &SlotState) -> f64 {
    if s.montage.is_empty() {
        return 0.0;
    }
    let m = rig.montage(&s.montage);
    let fade_in = m.map_or(0.0, |m| m.fade_in);
    let w_in = if m.is_some_and(|m| m.inertial) {
        1.0
    } else if fade_in > 0.0 {
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
    let mut sources = commands(rig, st, &mut out.notices);
    let mut pose = Pose::rest(rig);
    let mut root = [0.0f32; 3];
    for (li, layer) in rig.def.layers.iter().enumerate() {
        let weights = &rig.weights[li];
        let add = layer.blend == LayerBlend::Add;
        let lw = layer.weight.value(st);
        // The share of the root's motion a layer hands to the body: none from an additive one.
        let root_share = if add {
            0.0
        } else {
            lw as f32 * weights.get(rig.root.unwrap_or(0)).copied().unwrap_or(0.0)
        };
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
                    if t.inertial {
                        // The content before the switch, now and a tick earlier, against the
                        // target at its start.
                        let s0 = machine_content(rig, sets, st, li, 0.0);
                        let s1 = machine_content(rig, sets, st, li, dt);
                        let target = pose_of(
                            rig,
                            sets,
                            &sample_state(rig, st, &states[to], 0.0),
                            true,
                            add,
                        );
                        let ls = &mut st.layers[li];
                        ls.inertia = offsets(&s0, &s1, &target, dt);
                        ls.inertia_t = 0.0;
                        ls.inertia_len = t.fade;
                        ls.from.clear();
                        ls.from_time = 0.0;
                        ls.state = states[to].name.clone();
                        ls.time = 0.0;
                        ls.fade = 0.0;
                        ls.fade_len = 0.0;
                    } else {
                        let (from_state, from_time) = (states[cur].name.clone(), ls.time);
                        let ls = &mut st.layers[li];
                        ls.from = from_state;
                        ls.from_time = from_time;
                        ls.state = states[to].name.clone();
                        ls.time = 0.0;
                        ls.fade = 0.0;
                        ls.fade_len = t.fade;
                    }
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
                    if !add {
                        let w = layer_share(&st.layers[li]);
                        let dv = root_delta(rig, sets, c, a, b, s.looped);
                        root = math::add(root, math::scale(dv, (w * f64::from(root_share)) as f32));
                    }
                }
                let ls = &mut st.layers[li];
                if ls.fade_len > 0.0 && !(ls.fade < ls.fade_len && !ls.from.is_empty()) {
                    ls.from.clear();
                    ls.fade_len = 0.0;
                    ls.fade = 0.0;
                }
                if !ls.inertia.is_empty() {
                    ls.inertia_t += dt;
                    if ls.inertia_t >= ls.inertia_len {
                        ls.inertia.clear();
                        ls.inertia_t = 0.0;
                        ls.inertia_len = 0.0;
                    }
                }
                let lp = machine_content(rig, sets, st, li, 0.0);
                lay_over(rig, &mut pose, &lp, weights, lw as f32, add);
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
                // An inertial start or jump: the content it switches from, mixed over the pose
                // below by the share it had, now and a tick earlier, against its first frame.
                if let Some(src) = sources[si].take()
                    && let Some((c, _, _)) = slot_clip(rig, slot)
                {
                    let base = if add {
                        Pose::identity(rig)
                    } else {
                        Pose {
                            x: pose.x.clone(),
                            w: pose.w.clone(),
                        }
                    };
                    let content = |back: f64| -> Pose {
                        let Some((fc, ft, rate)) = src.from else {
                            return Pose {
                                x: base.x.clone(),
                                w: base.w.clone(),
                            };
                        };
                        let mut old = pose_of(
                            rig,
                            sets,
                            &Sampled {
                                parts: vec![(fc, max(ft - back * rate, 0.0), 1.0)],
                            },
                            true,
                            add,
                        );
                        if !src.inertia.is_empty() {
                            lay(
                                &mut old,
                                &src.inertia,
                                max(src.inertia_t - back, 0.0),
                                src.inertia_len,
                            );
                        }
                        mixed(&base, &old, src.share as f32)
                    };
                    let target = pose_of(
                        rig,
                        sets,
                        &Sampled {
                            parts: vec![(c, slot.time, 1.0)],
                        },
                        true,
                        add,
                    );
                    slot.inertia = offsets(&content(0.0), &content(dt), &target, dt);
                    slot.inertia_t = 0.0;
                    slot.inertia_len = m.fade_in;
                    slot.from_clip.clear();
                }
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
                    let end = t1 >= d;
                    let t1 = if end { d } else { t1 };
                    crossed(rig, c, t0, t1, false, &mut out.notices);
                    if !add {
                        let dv = root_delta(rig, sets, c, t0, t1, false);
                        root = math::add(
                            root,
                            math::scale(dv, slot_weight(rig, slot) as f32 * root_share),
                        );
                    }
                    if !end {
                        slot.time = t1;
                        left = 0.0;
                    } else {
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
                if !slot.inertia.is_empty() {
                    slot.inertia_t += dt;
                    if slot.inertia_t >= slot.inertia_len {
                        slot.inertia.clear();
                        slot.inertia_t = 0.0;
                        slot.inertia_len = 0.0;
                    }
                }
                let w = slot_weight(rig, slot);
                if slot.out >= 0.0 && w <= 0.0 {
                    slot.montage.clear();
                    slot.section.clear();
                    slot.from_clip.clear();
                    slot.inertia.clear();
                    slot.inertia_t = 0.0;
                    slot.inertia_len = 0.0;
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
                    add,
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
                            add,
                        );
                        let u = (slot.age / fade_in) as f32;
                        lp.fade_from(&from, u);
                    } else {
                        slot.from_clip.clear();
                    }
                }
                if !slot.inertia.is_empty() {
                    lay(&mut lp, &slot.inertia, slot.inertia_t, slot.inertia_len);
                }
                // With a frozen pose to fade from, the slot holds its whole weight while fading in.
                let w = if slot.from_clip.is_empty() {
                    w
                } else {
                    max(slot.from_weight, w)
                };
                lay_over(rig, &mut pose, &lp, weights, (w * lw) as f32, add);
            }
        }
    }
    out.pose = pose.x;
    out.morph = pose.w;
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

/// A layer's content over `pose`: an override layer's pose, or an additive layer's differences.
fn lay_over(rig: &Rig, pose: &mut Pose, layer: &Pose, weights: &[f32], w: f32, add: bool) {
    if add {
        add_over(rig, pose, layer, weights, w);
    } else {
        over(rig, pose, layer, weights, w);
    }
}

/// Differences added to `pose` by each joint's weight times `w` (animation.md 3a): the rotation
/// turned by the difference's share in the parent's frame, the translation moved, the scale
/// multiplied, the morph weights added.
fn add_over(rig: &Rig, pose: &mut Pose, diff: &Pose, weights: &[f32], w: f32) {
    for ((p, d), m) in pose.x.iter_mut().zip(&diff.x).zip(weights) {
        let u = m * w;
        // A joint the layer leaves as it is keeps its bits.
        if u <= 0.0 || *d == Xform::IDENTITY {
            continue;
        }
        let u = if u > 1.0 { 1.0 } else { u };
        let r = if u >= 1.0 {
            d.r
        } else {
            math::nlerp(IDENTITY, d.r, u)
        };
        p.r = math::normalize(math::qmul(r, p.r));
        p.t = math::add(p.t, math::scale(d.t, u));
        p.s = [
            p.s[0] * (1.0 + (d.s[0] - 1.0) * u),
            p.s[1] * (1.0 + (d.s[1] - 1.0) * u),
            p.s[2] * (1.0 + (d.s[2] - 1.0) * u),
        ];
    }
    if pose.w.is_empty() {
        return;
    }
    for (j, m) in weights.iter().enumerate() {
        let u = m * w;
        if u <= 0.0 {
            continue;
        }
        let u = if u > 1.0 { 1.0 } else { u };
        let (a, b) = (rig.morph_at[j], rig.morph_at[j + 1]);
        for (p, d) in pose.w[a..b].iter_mut().zip(&diff.w[a..b]) {
            *p += d * u;
        }
    }
}

/// `layer` over `pose` by each joint's weight times `w`; a joint's morph weights go with it.
fn over(rig: &Rig, pose: &mut Pose, layer: &Pose, weights: &[f32], w: f32) {
    for ((p, l), m) in pose.x.iter_mut().zip(&layer.x).zip(weights) {
        let u = m * w;
        if u >= 1.0 {
            *p = *l;
        } else if u > 0.0 {
            *p = Xform::mix(p, l, u);
        }
    }
    if pose.w.is_empty() {
        return;
    }
    for (j, m) in weights.iter().enumerate() {
        let u = m * w;
        let (a, b) = (rig.morph_at[j], rig.morph_at[j + 1]);
        for (p, l) in pose.w[a..b].iter_mut().zip(&layer.w[a..b]) {
            if u >= 1.0 {
                *p = *l;
            } else if u > 0.0 {
                *p += (*l - *p) * u;
            }
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

    use super::super::clip::{Clip, Joint, Keys, Track};
    use super::super::state::AnimCommand;

    fn joint(name: &str, parent: Option<u32>, t: V3) -> Joint {
        Joint {
            name: name.into(),
            parent,
            rest: Xform {
                t,
                ..Xform::IDENTITY
            },
            targets: Vec::new(),
            weights: Vec::new(),
        }
    }

    /// A turn of `a` about z (sine and cosine of the half angle given, so no transcendental).
    fn turn(s: f32, c: f32) -> Vec<f32> {
        vec![0.0, 0.0, s, c]
    }

    fn track(joint: &str, times: &[f32], rots: &[Vec<f32>]) -> Track {
        Track {
            joint: joint.into(),
            t: Keys::default(),
            r: Keys {
                times: times.to_vec(),
                values: rots.concat(),
                step: false,
            },
            s: Keys::default(),
            w: Keys::default(),
        }
    }

    /// A pelvis, a spine and an arm, with clips that turn them: `Swing` turns the pelvis back and
    /// forth over 1 s, `Idle` holds the spine a little turned, `Lean` leans the spine, `Flinch`
    /// turns it out and back over 0.5 s, `Wave` turns the arm over 1 s.
    fn test_rig(graph: &str) -> (Rig, Vec<ClipSet>) {
        let skeleton = Skeleton::new(vec![
            joint("root", None, [0.0; 3]),
            joint("pelvis", Some(0), [0.0, 1.0, 0.0]),
            joint("spine", Some(1), [0.0, 0.5, 0.0]),
            joint("arm", Some(2), [0.3, 0.0, 0.0]),
        ]);
        let clip = |name: &str, duration: f32, tracks: Vec<Track>| Clip {
            name: name.into(),
            duration,
            tracks,
        };
        let set = ClipSet {
            skeleton,
            clips: vec![
                clip(
                    "Swing",
                    1.0,
                    vec![track(
                        "pelvis",
                        &[0.0, 0.5, 1.0],
                        &[turn(-0.3, 0.954), turn(0.3, 0.954), turn(-0.3, 0.954)],
                    )],
                ),
                clip(
                    "Idle",
                    1.0,
                    vec![track("spine", &[0.0], &[turn(0.1, 0.995)])],
                ),
                clip(
                    "Lean",
                    1.0,
                    vec![track("spine", &[0.0], &[turn(0.2, 0.98)])],
                ),
                clip(
                    "Flinch",
                    0.5,
                    vec![track(
                        "spine",
                        &[0.0, 0.25, 0.5],
                        &[turn(0.0, 1.0), turn(0.4, 0.9165), turn(0.0, 1.0)],
                    )],
                ),
                clip(
                    "Wave",
                    1.0,
                    vec![track("arm", &[0.0, 1.0], &[turn(0.0, 1.0), turn(0.6, 0.8)])],
                ),
            ],
        };
        let def = GraphDef::parse(&serde_json::from_str(graph).unwrap()).unwrap();
        let sets = vec![("s.glb".to_owned(), set)];
        let rig = Rig::new(def, &sets).unwrap();
        (rig, sets.into_iter().map(|s| s.1).collect())
    }

    const DT: f64 = 1.0 / 60.0;

    fn set(st: &mut AnimState, name: &str, value: f64) {
        match st.params.iter_mut().find(|p| p.name == name) {
            Some(p) => p.value = value,
            None => st.params.push(AnimParam {
                name: name.into(),
                value,
            }),
        }
    }

    /// The angle between two rotations' poses of a joint, as `1 - |dot|` (0 when the same).
    fn apart(a: Q4, b: Q4) -> f32 {
        1.0 - (a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]).abs()
    }
    type Q4 = [f32; 4];

    const LEAN: &str = r#"{
        "format": "amoris-animgraph", "version": 1, "skeleton": "s.glb",
        "params": {"lean": 0},
        "clips": {"Lean": {"reference": "rest"}},
        "layers": [
          {"name": "base", "states": {"idle": {"clip": "Idle"}}},
          {"name": "lean", "blend": "add", "weight": "lean", "states": {"lean": {"clip": "Lean"}}}]
    }"#;

    #[test]
    fn an_additive_layer_turns_the_pose_by_its_difference_times_its_weight() {
        let (rig, sets) = test_rig(LEAN);
        let idle = [0.0, 0.0, 0.1, 0.995];
        let lean = math::normalize([0.0, 0.0, 0.2, 0.98]);
        for (w, want) in [
            (0.0, math::normalize(idle)),
            (1.0, math::normalize(math::qmul(lean, idle))),
            (
                0.5,
                math::normalize(math::qmul(math::nlerp(IDENTITY, lean, 0.5), idle)),
            ),
        ] {
            let mut st = rig.initial("g");
            set(&mut st, "lean", w);
            let out = step(&rig, &sets, &mut st, DT);
            assert!(
                apart(out.pose[2].r, want) < 1e-6,
                "{w}: {:?}",
                out.pose[2].r
            );
            // The other joints keep the base's pose; an additive layer moves no body.
            assert_eq!(out.pose[3], rig.skeleton.rest()[3]);
            assert_eq!(out.root_motion, [0.0; 3]);
        }
        // At weight 0 the pose is the base layer's alone, to the bit.
        let alone = LEAN.replace(
            r#",
          {"name": "lean", "blend": "add", "weight": "lean", "states": {"lean": {"clip": "Lean"}}}"#,
            "",
        );
        let (base, bsets) = test_rig(&alone);
        let (mut a, mut b) = (rig.initial("g"), base.initial("g"));
        for _ in 0..10 {
            assert_eq!(
                step(&rig, &sets, &mut a, DT).pose,
                step(&base, &bsets, &mut b, DT).pose
            );
        }
    }

    #[test]
    fn an_additive_montage_flinches_without_stopping_the_steps() {
        let (rig, sets) = test_rig(
            r#"{
            "format": "amoris-animgraph", "version": 1, "skeleton": "s.glb",
            "layers": [
              {"name": "base", "states": {"swing": {"clip": "Swing"}}},
              {"name": "flinch", "blend": "add", "slot": "flinch"}],
            "montages": {"flinch": {"slot": "flinch", "fade_in": 0, "fade_out": 0,
                                    "sections": [{"name": "a", "clip": "Flinch"}]}}
        }"#,
        );
        let (mut with, mut without) = (rig.initial("g"), rig.initial("g"));
        with.commands.push(AnimCommand {
            op: "play".into(),
            montage: "flinch".into(),
            rate: 1.0,
            ..AnimCommand::default()
        });
        for tick in 0..30 {
            let a = step(&rig, &sets, &mut with, DT);
            let b = step(&rig, &sets, &mut without, DT);
            // The pelvis swings on as if nothing happened.
            assert_eq!(a.pose[1], b.pose[1], "tick {tick}");
            if tick == 14 {
                // A quarter of a second in: the spine is turned by the flinch's difference from
                // its first frame (the identity), over the rest.
                assert!(apart(a.pose[2].r, math::normalize([0.0, 0.0, 0.4, 0.9165])) < 1e-4);
            }
        }
    }

    #[test]
    fn the_decay_starts_at_the_offset_ends_at_zero_and_never_overshoots() {
        for &x0 in &[0.01f32, 0.3, 1.0] {
            for &v0 in &[0.0f32, -0.5, -4.0, -100.0, 3.0] {
                for &t1 in &[0.1f32, 0.3] {
                    assert_eq!(decay(x0, v0, 0.0, t1), x0);
                    assert_eq!(decay(x0, v0, t1, t1), 0.0);
                    let mut last = x0;
                    for i in 1..=100 {
                        let x = decay(x0, v0, t1 * i as f32 / 100.0, t1);
                        assert!((0.0..=x0).contains(&x), "{x0} {v0} {t1}: {x}");
                        assert!(x <= last + 1e-6, "{x0} {v0} {t1}: rises to {x} from {last}");
                        last = x;
                    }
                }
            }
        }
        assert_eq!(decay(0.0, -1.0, 0.0, 0.2), 0.0);
    }

    const SWITCH: &str = r#"{
        "format": "amoris-animgraph", "version": 1, "skeleton": "s.glb",
        "params": {"go": false},
        "layers": [{"name": "base", "start": "swing", "states": {
            "swing": {"clip": "Swing"}, "wave": {"clip": "Wave"}},
          "transitions": [{"from": "swing", "to": "wave", "when": [["go", "==", true]],
                           "fade": 0.25, "blend": "inertial"}]}]
    }"#;

    #[test]
    fn an_inertial_switch_keeps_the_motion_and_ends_on_the_target_to_the_bit() {
        let (rig, sets) = test_rig(SWITCH);
        let mut st = rig.initial("g");
        let mut poses = Vec::new();
        for tick in 0..60 {
            if tick == 20 {
                set(&mut st, "go", 1.0);
            }
            poses.push(step(&rig, &sets, &mut st, DT).pose);
            if tick == 20 {
                assert_eq!(st.layers[0].state, "wave");
                assert!(!st.layers[0].inertia.is_empty());
                assert!(st.layers[0].from.is_empty(), "no crossfade");
            }
        }
        // Across the switch the pelvis moves about as much as a tick before it did (the swing's
        // speed carried on), not to the wave's rest at once.
        let before = apart(poses[19][1].r, poses[18][1].r);
        let across = apart(poses[20][1].r, poses[19][1].r);
        assert!(across < 4.0 * before + 1e-7, "{across} against {before}");
        assert!(apart(poses[20][1].r, IDENTITY) > 1e-3, "still turned");
        // Once the offsets run out (0.25 s), the layer plays the wave alone, as a graph that
        // started on it does, bit for bit.
        assert!(st.layers[0].inertia.is_empty());
        let (wave, wsets) = test_rig(&SWITCH.replace(r#""start": "swing""#, r#""start": "wave""#));
        let mut w = wave.initial("g");
        let alone: Vec<Vec<Xform>> = (0..40)
            .map(|_| step(&wave, &wsets, &mut w, DT).pose)
            .collect();
        for k in 20..40 {
            assert_eq!(poses[20 + k], alone[k], "tick {k} after the switch");
        }
    }

    #[test]
    fn an_inertial_montage_takes_its_whole_weight_at_once_and_starts_from_the_pose() {
        let (rig, sets) = test_rig(
            r#"{
            "format": "amoris-animgraph", "version": 1, "skeleton": "s.glb",
            "layers": [
              {"name": "base", "states": {"idle": {"clip": "Idle"}}},
              {"name": "full", "slot": "full"}],
            "montages": {
              "wave": {"slot": "full", "blend_in": "inertial", "fade_in": 0.2, "fade_out": 0.1,
                       "sections": [{"name": "a", "clip": "Wave", "next": "b"},
                                    {"name": "b", "clip": "Flinch"}]}}
        }"#,
        );
        let mut st = rig.initial("g");
        let idle = step(&rig, &sets, &mut st, DT).pose;
        st.commands.push(AnimCommand {
            op: "play".into(),
            montage: "wave".into(),
            rate: 1.0,
            ..AnimCommand::default()
        });
        let first = step(&rig, &sets, &mut st, DT).pose;
        assert_eq!(slot_weight(&rig, &st.slots[0]), 1.0);
        // The spine, which the wave leaves at rest, starts where the idle held it.
        assert!(
            apart(first[2].r, idle[2].r) < 1e-4,
            "{:?} {:?}",
            first[2].r,
            idle[2].r
        );
        for _ in 0..20 {
            step(&rig, &sets, &mut st, DT);
        }
        assert!(st.slots[0].inertia.is_empty(), "run out after 0.2 s");
        assert_eq!(st.slots[0].from_clip, "");
        // A jump inertializes too: the arm starts where the wave had it, not at the flinch's rest.
        let held = step(&rig, &sets, &mut st, DT).pose;
        st.commands.push(AnimCommand {
            op: "jump".into(),
            section: "b".into(),
            ..AnimCommand::default()
        });
        let jumped = step(&rig, &sets, &mut st, DT).pose;
        assert_eq!(st.slots[0].section, "b");
        assert!(
            apart(jumped[3].r, held[3].r) < 1e-3,
            "{:?} {:?}",
            jumped[3].r,
            held[3].r
        );
        assert!(apart(jumped[3].r, IDENTITY) > 1e-2);
    }

    #[test]
    fn layer_blends_weights_and_references_parse_and_refuse_what_is_wrong() {
        let v = |s: &str| serde_json::from_str::<Value>(s).unwrap();
        let g = GraphDef::parse(&v(SWITCH)).unwrap();
        assert!(g.layers[0].kind != LayerKind::Slot(String::new()));
        let LayerKind::Machine { transitions, .. } = &g.layers[0].kind else {
            panic!()
        };
        assert!(transitions[0].inertial);
        let g = GraphDef::parse(&v(LEAN)).unwrap();
        assert_eq!(g.layers[1].blend, LayerBlend::Add);
        assert_eq!(g.layers[1].weight, ParamWeight::Param("lean".into()));
        assert_eq!(g.clips[0].reference, Reference::Rest);
        let refused = |from: &str, to: &str, says: &str| {
            let e = GraphDef::parse(&v(&LEAN.replace(from, to))).unwrap_err();
            assert!(e.contains(says), "{e}");
        };
        refused(r#""blend": "add""#, r#""blend": "multiply""#, "multiply");
        refused(
            r#""weight": "lean""#,
            r#""weight": "tilt""#,
            "no parameter tilt",
        );
        refused(
            r#"{"name": "base","#,
            r#"{"name": "base", "blend": "add","#,
            "base layer",
        );
        refused(r#""reference": "rest""#, r#""reference": 3"#, "reference");
        let e = GraphDef::parse(&v(&SWITCH.replace("inertial", "snap"))).unwrap_err();
        assert!(e.contains("snap"), "{e}");
        // A reference clip is bound like any clip; one no library has is refused.
        let missing = LEAN.replace(r#""reference": "rest""#, r#""reference": {"clip": "Nope"}"#);
        let def = GraphDef::parse(&v(&missing)).unwrap();
        assert!(def.clip_names().contains(&"Nope".to_owned()));
    }
}
