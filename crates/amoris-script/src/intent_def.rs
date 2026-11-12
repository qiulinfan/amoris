//! A script intent's declaration (docs/spec/script-host.md 5.5): the value of `intent(name, def)`
//! read into the contract's `IntentDef` and the game codes it declares. Its parameters become a
//! closed JSON Schema with their defaults, `timeout_s` (and with `canHold`, `keep`) added; channels
//! and code names are checked against the game's action catalog when the program is installed
//! (`scripts::declare_intents`), since only the world knows the catalog.

use amoris_contract::{Shape, Use};
use amoris_interface::action::defs::{CodeDef, IntentDef, ProgressDef, TargetSpec};
use amoris_interface::perception::defs::Unit;
use rquickjs::{Ctx, Value};
use serde_json::{Map, Value as Json, json};

use crate::define::{bad, entries, items, tagged, text, unknown_key};
use crate::error::ScriptError;
use crate::js;

/// The longest timeout an intent may take, seconds (a day).
const MAX_TIMEOUT_S: f64 = 86_400.0;

/// The contract's own codes an intent may fail with by name (errors.md; actions.md, Intents).
pub const CONTRACT_FAIL_CODES: [&str; 2] = ["perception.target_lost", "intent.timeout"];

/// An intent as the program declares it, with the game codes its `failures` declare.
pub struct Declared {
    pub def: IntentDef,
    pub codes: Vec<CodeDef>,
}

/// `^[a-z][a-z0-9_]*$`, at most 56 bytes: an intent's, a parameter's or a reading's name.
pub fn word(name: &str) -> bool {
    name.len() <= 56
        && name.bytes().next().is_some_and(|c| c.is_ascii_lowercase())
        && name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
}

/// `family.name`, both words: a code's form.
fn code_name(code: &str) -> bool {
    code.split_once('.')
        .is_some_and(|(f, n)| word(f) && n.split('.').all(word))
}

fn number(v: &Value<'_>, what: &str) -> Result<f64, ScriptError> {
    v.as_number()
        .filter(|x| x.is_finite())
        .ok_or_else(|| bad(format!("{what} must be a finite number.")))
}

fn texts<'js>(ctx: &Ctx<'js>, v: &Value<'js>, what: &str) -> Result<Vec<String>, ScriptError> {
    items(ctx, v, what)?.iter().map(|x| text(x, what)).collect()
}

/// One `param` builder's value as a schema property, and its default.
fn param<'js>(
    ctx: &Ctx<'js>,
    v: &Value<'js>,
    what: &str,
) -> Result<(Json, Option<Json>), ScriptError> {
    let Some((tag, e)) = tagged(ctx, v) else {
        return Err(bad(format!(
            "{what} must be made by a param builder, such as param.number(\"doc\")."
        )));
    };
    if tag != "param" {
        return Err(bad(format!("{what} must be made by a param builder.")));
    }
    let get = |k: &str| {
        e.iter()
            .find(|(n, _)| n == k)
            .map(|(_, x)| x.clone())
            .filter(|x| !x.is_undefined())
    };
    let ty = text(
        &get("type").unwrap_or_else(|| Value::new_undefined(ctx.clone())),
        what,
    )?;
    let doc = get("doc")
        .map(|d| text(&d, what))
        .transpose()?
        .unwrap_or_default();
    let options = get("options");
    let mut opts: Vec<(String, Value<'js>)> = Vec::new();
    if let Some(o) = &options {
        opts = entries(ctx, o, &format!("{what}'s options"))?;
    }
    let allowed: &[&str] = match ty.as_str() {
        "number" | "integer" => &["default", "min", "max"],
        "bool" | "choice" | "text" => &["default"],
        other => {
            return Err(bad(format!(
                "{what} has an unknown parameter type '{other}'."
            )));
        }
    };
    for (k, _) in &opts {
        if !allowed.contains(&k.as_str()) {
            return Err(unknown_key(&format!("{what}'s options"), k, allowed));
        }
    }
    let opt = |k: &str| {
        opts.iter()
            .find(|(n, _)| n == k)
            .map(|(_, x)| x.clone())
            .filter(|x| !x.is_undefined())
    };
    let mut prop = Map::new();
    prop.insert("description".into(), json!(doc));
    let default = match ty.as_str() {
        "number" | "integer" => {
            prop.insert(
                "type".into(),
                json!(if ty == "integer" { "integer" } else { "number" }),
            );
            let min = opt("min")
                .map(|x| number(&x, &format!("{what}'s min")))
                .transpose()?;
            let max = opt("max")
                .map(|x| number(&x, &format!("{what}'s max")))
                .transpose()?;
            if let (Some(lo), Some(hi)) = (min, max)
                && lo > hi
            {
                return Err(bad(format!("{what}'s min {lo} is above its max {hi}.")));
            }
            if let Some(lo) = min {
                prop.insert("minimum".into(), amoris_contract::codes::num(lo));
            }
            if let Some(hi) = max {
                prop.insert("maximum".into(), amoris_contract::codes::num(hi));
            }
            match opt("default") {
                Some(d) => {
                    let x = number(&d, &format!("{what}'s default"))?;
                    let integral = ty != "integer" || x.fract() == 0.0;
                    if !integral || min.is_some_and(|lo| x < lo) || max.is_some_and(|hi| x > hi) {
                        return Err(bad(format!(
                            "{what}'s default {x} is not one of its values."
                        )));
                    }
                    Some(amoris_contract::codes::num(x))
                }
                None => None,
            }
        }
        "bool" => {
            prop.insert("type".into(), json!("boolean"));
            match opt("default") {
                Some(d) => Some(json!(d.as_bool().ok_or_else(|| bad(format!(
                    "{what}'s default must be true or false."
                )))?)),
                None => None,
            }
        }
        "choice" => {
            let values = texts(
                ctx,
                &get("values").unwrap_or_else(|| Value::new_undefined(ctx.clone())),
                &format!("{what}'s values"),
            )?;
            if values.is_empty() {
                return Err(bad(format!("{what} needs at least one value.")));
            }
            prop.insert("type".into(), json!("string"));
            prop.insert("enum".into(), json!(values));
            match opt("default") {
                Some(d) => {
                    let d = text(&d, &format!("{what}'s default"))?;
                    if !values.contains(&d) {
                        return Err(bad(format!(
                            "{what}'s default '{d}' is not one of its values."
                        )));
                    }
                    Some(json!(d))
                }
                None => None,
            }
        }
        _ => {
            prop.insert("type".into(), json!("string"));
            prop.insert("maxLength".into(), json!(256));
            match opt("default") {
                Some(d) => Some(json!(text(&d, &format!("{what}'s default"))?)),
                None => None,
            }
        }
    };
    Ok((Json::Object(prop), default))
}

fn target<'js>(ctx: &Ctx<'js>, v: &Value<'js>, name: &str) -> Result<TargetSpec, ScriptError> {
    if v.is_undefined() {
        return Ok(TargetSpec::None);
    }
    let what = format!("intent {name}'s target");
    if let Some(Ok(t)) = js::text(v) {
        return match t.as_str() {
            "none" => Ok(TargetSpec::None),
            "point" => Ok(TargetSpec::Point),
            _ => Err(bad(format!(
                "{what} is \"none\", \"point\", {{entity: kinds}} or {{entityOrPoint: kinds}}."
            ))),
        };
    }
    let e = entries(ctx, v, &what)?;
    match e.as_slice() {
        [(k, kinds)] if k == "entity" || k == "entityOrPoint" => {
            let kinds = texts(ctx, kinds, &format!("{what}.{k}"))?;
            if kinds.is_empty() {
                return Err(bad(format!("{what}.{k} names no kind.")));
            }
            Ok(if k == "entity" {
                TargetSpec::Entity { kinds }
            } else {
                TargetSpec::EntityOrPoint { kinds }
            })
        }
        [(k, _)] => Err(unknown_key(&what, k, &["entity", "entityOrPoint"])),
        _ => Err(bad(format!(
            "{what} is \"none\", \"point\", {{entity: kinds}} or {{entityOrPoint: kinds}}."
        ))),
    }
}

fn code<'js>(ctx: &Ctx<'js>, v: &Value<'js>, name: &str) -> Result<CodeDef, ScriptError> {
    let what = format!("intent {name}'s failure");
    let e = entries(ctx, v, &what)?;
    let keys = ["code", "when", "detail", "template"];
    for (k, _) in &e {
        if !keys.contains(&k.as_str()) {
            return Err(unknown_key(&what, k, &keys));
        }
    }
    let get = |k: &str| {
        e.iter()
            .find(|(n, _)| n == k)
            .map(|(_, x)| x.clone())
            .unwrap_or_else(|| Value::new_undefined(ctx.clone()))
    };
    let code = text(&get("code"), &format!("{what}'s code"))?;
    check_code(&code, name)?;
    let detail = match get("detail") {
        d if d.is_undefined() => Vec::new(),
        d => texts(ctx, &d, &format!("{what} {code}'s detail"))?,
    };
    Ok(CodeDef {
        code: code.clone(),
        uses: vec![Use::Fail],
        when: text(&get("when"), &format!("{what} {code}'s when"))?,
        detail,
        template: text(&get("template"), &format!("{what} {code}'s template"))?,
    })
}

/// A code the game declares: `family.name`, in a family the engine does not own.
fn check_code(code: &str, intent: &str) -> Result<(), ScriptError> {
    if !code_name(code) {
        return Err(bad(format!(
            "intent {intent}'s failure '{code}' is not a code such as \"errand.blocked\"."
        )));
    }
    let family = code.split('.').next().unwrap_or_default();
    if amoris_contract::codes::is_engine_family(family) {
        return Err(bad(format!(
            "intent {intent}'s failure '{code}' is in the engine's family {family}; name it after \
             your game."
        )));
    }
    Ok(())
}

fn progress<'js>(ctx: &Ctx<'js>, v: &Value<'js>, name: &str) -> Result<ProgressDef, ScriptError> {
    let what = format!("intent {name}'s progress reading");
    let e = entries(ctx, v, &what)?;
    let keys = ["name", "doc", "unit", "precision"];
    for (k, _) in &e {
        if !keys.contains(&k.as_str()) {
            return Err(unknown_key(&what, k, &keys));
        }
    }
    let get = |k: &str| {
        e.iter()
            .find(|(n, _)| n == k)
            .map(|(_, x)| x.clone())
            .unwrap_or_else(|| Value::new_undefined(ctx.clone()))
    };
    let reading = text(&get("name"), &format!("{what}'s name"))?;
    if !word(&reading) {
        return Err(bad(format!(
            "'{reading}' is not a reading's name: ^[a-z][a-z0-9_]*$."
        )));
    }
    let unit = crate::convert::plain_from_js(ctx, &get("unit"))
        .ok()
        .and_then(|p| serde_json::from_value::<Unit>(p.to_json()).ok())
        .ok_or_else(|| {
            bad(format!(
                "{what} {reading}'s unit is a perception unit such as {{unit: \"metres\"}}."
            ))
        })?;
    let precision = get("precision")
        .as_number()
        .filter(|p| p.fract() == 0.0 && (0.0..=12.0).contains(p))
        .ok_or_else(|| {
            bad(format!(
                "{what} {reading}'s precision is a whole number of decimals from 0 to 12."
            ))
        })?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // checked above
    let precision = precision as u8;
    let doc = match get("doc") {
        d if d.is_undefined() => String::new(),
        d => text(&d, &format!("{what} {reading}'s doc"))?,
    };
    Ok(ProgressDef {
        name: reading,
        doc,
        unit,
        precision,
    })
}

/// Reads one `intent(name, def)` value.
pub fn intent<'js>(ctx: &Ctx<'js>, v: &Value<'js>) -> Result<Declared, ScriptError> {
    let Some((tag, e)) = tagged(ctx, v) else {
        return Err(bad(
            "executor({intents}) takes values made by intent(name, {...}).".into(),
        ));
    };
    if tag != "intent" {
        return Err(bad(
            "executor({intents}) takes values made by intent(name, {...}).".into(),
        ));
    }
    let undef = || Value::new_undefined(ctx.clone());
    let get = |k: &str| e.iter().find(|(n, _)| n == k).map(|(_, x)| x.clone());
    let name = text(&get("name").unwrap_or_else(undef), "An intent's name")?;
    if !word(&name) {
        return Err(bad(format!(
            "'{name}' is not an intent name: ^[a-z][a-z0-9_]*$, at most 56 bytes."
        )));
    }
    let def = get("def").unwrap_or_else(undef);
    let fields = entries(ctx, &def, &format!("intent {name}"))?;
    let keys = [
        "doc", "params", "target", "channels", "canHold", "timeoutS", "failures", "progress",
    ];
    for (k, _) in &fields {
        if !keys.contains(&k.as_str()) {
            return Err(unknown_key(&format!("intent {name}"), k, &keys));
        }
    }
    let field = |k: &str| {
        fields
            .iter()
            .find(|(n, _)| n == k)
            .map(|(_, x)| x.clone())
            .unwrap_or_else(undef)
    };
    let doc = text(&field("doc"), &format!("intent {name}'s doc"))?;
    let channels = texts(
        ctx,
        &field("channels"),
        &format!("intent {name}'s channels"),
    )?;
    if channels.is_empty() {
        return Err(bad(format!("intent {name} occupies no channel.")));
    }
    let can_hold = match field("canHold") {
        v if v.is_undefined() => false,
        v => v
            .as_bool()
            .ok_or_else(|| bad(format!("intent {name}'s canHold must be true or false.")))?,
    };
    let timeout = number(&field("timeoutS"), &format!("intent {name}'s timeoutS"))?;
    if timeout <= 0.0 || timeout > MAX_TIMEOUT_S {
        return Err(bad(format!(
            "intent {name}'s timeoutS {timeout} is not above 0 and at most {MAX_TIMEOUT_S} s."
        )));
    }
    // The parameters' schema and defaults.
    let mut props = Map::new();
    let mut required = Vec::new();
    let mut defaults = Map::new();
    let p = field("params");
    if !p.is_undefined() {
        for (pname, pv) in entries(ctx, &p, &format!("intent {name}'s params"))? {
            if !word(&pname) {
                return Err(bad(format!(
                    "'{pname}' is not a parameter name: ^[a-z][a-z0-9_]*$."
                )));
            }
            if pname == "timeout_s" || pname == "keep" {
                return Err(bad(format!(
                    "intent {name} declares the parameter {pname}, which the engine adds."
                )));
            }
            let (prop, default) = param(ctx, &pv, &format!("intent {name}'s parameter {pname}"))?;
            match default {
                Some(d) => {
                    defaults.insert(pname.clone(), d);
                }
                None => required.push(pname.clone()),
            }
            props.insert(pname, prop);
        }
    }
    props.insert(
        "timeout_s".into(),
        json!({"type": "number",
               "description": format!("Seconds until it must finish (default {}).",
                                      amoris_contract::codes::num(timeout)),
               "exclusiveMinimum": 0, "maximum": MAX_TIMEOUT_S}),
    );
    defaults.insert("timeout_s".into(), amoris_contract::codes::num(timeout));
    if can_hold {
        props.insert(
            "keep".into(),
            json!({"type": "boolean", "description": "Keep the goal once reached (default false)."}),
        );
        defaults.insert("keep".into(), json!(false));
    }
    let mut schema = json!({"type": "object", "properties": props, "additionalProperties": false});
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    let target = target(ctx, &field("target"), &name)?;
    // Failures: declared codes, and names of codes declared elsewhere.
    let mut failures = Vec::new();
    let mut codes = Vec::new();
    let f = field("failures");
    if !f.is_undefined() {
        for x in items(ctx, &f, &format!("intent {name}'s failures"))? {
            let c = if let Some(t) = js::text(&x) {
                let c =
                    t.map_err(|()| bad(format!("intent {name}'s failures hold a bad string.")))?;
                if !CONTRACT_FAIL_CODES.contains(&c.as_str()) {
                    check_code(&c, &name)?;
                }
                c
            } else {
                let d = code(ctx, &x, &name)?;
                let c = d.code.clone();
                codes.push(d);
                c
            };
            if !failures.contains(&c) {
                failures.push(c);
            }
        }
    }
    if !failures.iter().any(|c| c == "intent.timeout") {
        failures.push("intent.timeout".into());
    }
    let mut readings: Vec<ProgressDef> = Vec::new();
    let pr = field("progress");
    if !pr.is_undefined() {
        for x in items(ctx, &pr, &format!("intent {name}'s progress"))? {
            let r = progress(ctx, &x, &name)?;
            if readings.iter().any(|o| o.name == r.name) {
                return Err(bad(format!(
                    "intent {name} declares the reading {} twice.",
                    r.name
                )));
            }
            readings.push(r);
        }
    }
    Ok(Declared {
        def: IntentDef {
            name,
            doc,
            params: Shape::from_schema(schema),
            defaults,
            target,
            channels,
            can_hold,
            default_timeout_s: timeout,
            failures,
            progress: readings,
        },
        codes,
    })
}
