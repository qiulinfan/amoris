//! The shape checker against the request cases of shared/contract/errors.md (Checks) that need no
//! world: a request type shaped like the contract's `act`, checked through its schemars schema.

use pocket_contract::{CheckOptions, Pointer, Shape, decode};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
struct Act {
    actions: Vec<Action>,
    contract: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
struct Action {
    #[serde(rename = "do")]
    verb: Do,
    intent: Option<String>,
    params: Option<ComeToHeading>,
    controls: Option<Controls>,
    target: Option<Target>,
    entity: Option<EntityRef>,
}

#[derive(Debug, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Do {
    Start,
    Set,
    Cancel,
}

#[derive(Debug, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
struct ComeToHeading {
    #[schemars(extend("minimum" = 0, "exclusiveMaximum" = 360))]
    heading_deg: f64,
    tolerance_deg: Option<f64>,
    turn: Option<Turn>,
    settle_s: Option<f64>,
    keep: Option<bool>,
    timeout_s: Option<f64>,
    settle_ticks: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Turn {
    Shortest,
    Port,
    Starboard,
}

#[derive(Debug, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
struct Controls {
    #[schemars(range(min = -1, max = 1))]
    rudder: Option<f64>,
    #[schemars(range(min = 0, max = 1), extend("x-aliases" = ["mainsheet"]))]
    sheet: Option<f64>,
}

#[derive(Debug, Deserialize, JsonSchema, PartialEq)]
enum Target {
    Entity(u64),
    Point { x: f64, z: f64 },
}

#[derive(Debug, Deserialize, JsonSchema, PartialEq)]
#[serde(untagged)]
enum EntityRef {
    Id(u64),
    Name(String),
}

fn params_owner(p: &Pointer) -> Option<String> {
    (p.field_name() == "params").then(|| "come_to_heading".to_owned())
}

fn opts() -> CheckOptions<'static> {
    CheckOptions {
        owner: "the act request",
        base: Pointer::root(),
        owner_of: Some(&params_owner),
    }
}

fn act(params: Value) -> Value {
    json!({"actions": [{"do": "start", "intent": "come_to_heading", "params": params}]})
}

fn refuse(v: Value) -> pocket_contract::Problem {
    Shape::of::<Act>().check(&v, &opts()).expect_err("refused")
}

#[test]
fn a_valid_request_decodes() {
    let d = decode::<Act>(&act(json!({"heading_deg": 90, "turn": "port"})), &opts()).unwrap();
    assert!(d.warnings.is_empty());
    let p = d.value.actions[0].params.as_ref().unwrap();
    assert_eq!(p.heading_deg, 90.0);
    assert_eq!(p.turn, Some(Turn::Port));
}

#[test]
fn unknown_parameter_with_its_suggestion() {
    let p = refuse(act(json!({"headng": 90})));
    assert_eq!(p.code, "request.unknown_field");
    assert_eq!(p.path(), Some("/actions/0/params/headng"));
    assert_eq!(p.detail["suggestions"], json!(["heading_deg"]));
    assert_eq!(
        p.message,
        "come_to_heading has no parameter 'headng'; did you mean 'heading_deg'?"
    );
    // The missing heading_deg is the same mistake, not reported twice.
    assert!(p.also().is_empty());
    // headingDeg is the same name in another style: refused with a score-0 suggestion, never an
    // alias.
    let p = refuse(act(json!({"headingDeg": 90})));
    assert_eq!(p.code, "request.unknown_field");
    assert_eq!(p.detail["suggestions"], json!(["heading_deg"]));
}

#[test]
fn unit_stem_alias_is_accepted_with_a_warning() {
    let d = decode::<Act>(&act(json!({"heading": 90})), &opts()).unwrap();
    assert_eq!(
        d.value.actions[0].params.as_ref().unwrap().heading_deg,
        90.0
    );
    assert_eq!(d.warnings.len(), 1);
    assert_eq!(d.warnings[0].code, "request.alias_used");
    assert_eq!(d.warnings[0].detail["alias"], json!("heading"));
    assert_eq!(d.warnings[0].detail["field"], json!("heading_deg"));
    let p = refuse(act(json!({"heading": 90, "heading_deg": 90})));
    assert_eq!(p.code, "request.conflict");
    // A declared alias.
    let v = json!({"actions": [{"do": "set", "controls": {"mainsheet": 0.5}}]});
    let d = decode::<Act>(&v, &opts()).unwrap();
    assert_eq!(
        d.value.actions[0].controls.as_ref().unwrap().sheet,
        Some(0.5)
    );
    assert_eq!(d.warnings[0].detail["alias"], json!("mainsheet"));
}

#[test]
fn misplaced_parameter() {
    let v = json!({"actions": [{"do": "start", "intent": "come_to_heading", "heading_deg": 90}]});
    let p = refuse(v);
    assert_eq!(p.code, "request.misplaced_field");
    assert_eq!(
        p.detail["belongs_at"],
        json!("/actions/0/params/heading_deg")
    );
    // A single action sent without the actions array.
    let p = refuse(json!({"do": "start", "intent": "come_to_heading"}));
    assert_eq!(p.code, "request.misplaced_field");
    assert_eq!(p.detail["belongs_at"], json!("/actions/0/do"));
}

#[test]
fn ranges_types_and_values() {
    let p = refuse(act(json!({"heading_deg": 360})));
    assert_eq!(p.code, "request.out_of_range");
    assert_eq!(p.detail["min"], json!(0));
    assert_eq!(p.detail["max_exclusive"], json!(360));
    assert_eq!(
        p.message,
        "'heading_deg' must be from 0 up to but not including 360; got 360."
    );
    let p = refuse(act(json!({"heading_deg": "90"})));
    assert_eq!(p.code, "request.wrong_type");
    assert_eq!(p.detail["expected"], json!("number"));
    assert_eq!(p.detail["got"], json!("string"));
    let p = refuse(act(json!({"heading_deg": 90, "turn": "left"})));
    assert_eq!(p.code, "request.invalid_value");
    assert_eq!(
        p.detail["allowed"],
        json!(["shortest", "port", "starboard"])
    );
    assert_eq!(p.detail["suggestions"], json!([]));
    let p = refuse(json!({"actions": [{"do": "set", "controls": {"rudder": 1.5}}]}));
    assert_eq!(p.code, "request.out_of_range");
    assert_eq!(p.path(), Some("/actions/0/controls/rudder"));
    assert_eq!(p.detail["min"], json!(-1));
    assert_eq!(p.detail["max"], json!(1));
    let p = refuse(json!({"actions": [{"do": "sett"}]}));
    assert_eq!(p.code, "request.invalid_value");
    assert_eq!(p.detail["suggestions"], json!(["set"]));
}

#[test]
fn integers() {
    let p = refuse(act(json!({"heading_deg": 1, "settle_ticks": 2.5})));
    assert_eq!(p.code, "request.not_integer");
    let p = refuse(act(json!({"heading_deg": 1, "settle_ticks": -1})));
    assert_eq!(p.code, "request.out_of_range");
    assert_eq!(p.detail["max"], json!(4_294_967_295u64));
    let d = decode::<Act>(
        &act(json!({"heading_deg": 1, "settle_ticks": 3.0})),
        &opts(),
    )
    .unwrap();
    assert_eq!(
        d.value.actions[0].params.as_ref().unwrap().settle_ticks,
        Some(3)
    );
}

#[test]
fn several_problems_come_together_ordered_by_path() {
    let p = refuse(act(
        json!({"heading_deg": 1, "turnn": "port", "keeep": true}),
    ));
    assert_eq!(p.code, "request.unknown_field");
    assert_eq!(p.path(), Some("/actions/0/params/keeep"));
    assert_eq!(p.detail["suggestions"], json!(["keep"]));
    let also = p.also();
    assert_eq!(also.len(), 1);
    assert_eq!(also[0].path(), Some("/actions/0/params/turnn"));
    assert_eq!(also[0].detail["suggestions"], json!(["turn"]));
}

#[test]
fn enums_and_untagged_forms() {
    let ok = json!({"actions": [{"do": "start", "target": {"Point": {"x": 1, "z": 2}}}]});
    assert!(Shape::of::<Act>().check(&ok, &opts()).is_ok());
    let p = refuse(json!({"actions": [{"do": "start", "target": {"Pont": {"x": 1, "z": 2}}}]}));
    assert_eq!(p.code, "request.unknown_field");
    assert_eq!(p.detail["suggestions"], json!(["Point"]));
    let p = refuse(json!({"actions": [{"do": "start", "target": {"Point": {"x": 1}}}]}));
    assert_eq!(p.code, "request.missing_field");
    assert_eq!(p.path(), Some("/actions/0/target/Point/z"));
    let ok = json!({"actions": [{"do": "start", "entity": "Mark1"}]});
    assert!(Shape::of::<Act>().check(&ok, &opts()).is_ok());
    let p = refuse(json!({"actions": [{"do": "start", "entity": true}]}));
    assert_eq!(p.code, "request.wrong_type");
    assert_eq!(p.detail["got"], json!("boolean"));
}

#[test]
fn missing_fields_say_what_they_take() {
    let p = refuse(json!({}));
    assert_eq!(p.code, "request.missing_field");
    assert_eq!(p.message, "the act request needs 'actions' (a list).");
    let p = refuse(act(json!({})));
    assert_eq!(
        p.message,
        "come_to_heading needs 'heading_deg' (a number from 0 up to but not including 360)."
    );
    let p = refuse(json!({"actions": [{}]}));
    assert_eq!(
        p.message,
        "actions[0] needs 'do' (one of start, set, cancel)."
    );
}

#[test]
fn base_path_and_owner_without_hook() {
    let opts = CheckOptions {
        owner: "come_to_heading",
        base: Pointer::parse("/actions/0/params"),
        owner_of: None,
    };
    let p = Shape::of::<ComeToHeading>()
        .check(&json!({"heading_deg": 3, "ruder": 1}), &opts)
        .unwrap_err();
    assert_eq!(p.path(), Some("/actions/0/params/ruder"));
    // The names come from the schema, whose properties serde_json keeps in byte order.
    assert_eq!(
        p.message,
        "come_to_heading has no parameter 'ruder'; it takes heading_deg, keep, settle_s, \
         settle_ticks, timeout_s, tolerance_deg, turn."
    );
}
