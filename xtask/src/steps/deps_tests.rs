//! Tests of the `deps` rules over made-up metadata, including the `bad-edge` and `bad-feature`
//! controls of docs/spec/checks.md 8.6.

use super::*;
use crate::report::Verdict;
use crate::steps::metadata::{DepKind, Dependency, Node, NodeDep, Package, Resolve};

type Pkg<'a> = (&'a str, &'a [(&'a str, Option<&'a str>)], &'a [&'a str]);

fn meta(packages: &[Pkg], members: &[&str]) -> Metadata {
    let id = |n: &str| format!("id-{n}");
    Metadata {
        packages: packages
            .iter()
            .map(|(name, deps, _)| Package {
                id: id(name),
                name: name.to_string(),
                dependencies: deps
                    .iter()
                    .map(|(d, k)| Dependency {
                        name: d.to_string(),
                        kind: k.map(String::from),
                        ..Dependency::default()
                    })
                    .collect(),
                ..Package::default()
            })
            .collect(),
        workspace_members: members.iter().map(|m| id(m)).collect(),
        resolve: Some(Resolve {
            nodes: packages
                .iter()
                .map(|(name, deps, features)| Node {
                    id: id(name),
                    deps: deps
                        .iter()
                        .map(|(d, k)| NodeDep {
                            pkg: id(d),
                            dep_kinds: vec![DepKind {
                                kind: k.map(String::from),
                            }],
                        })
                        .collect(),
                    features: features.iter().map(|f| f.to_string()).collect(),
                })
                .collect(),
        }),
        target_directory: String::new(),
    }
}

fn graph() -> Graph {
    crate::config::parse(
        CONFIG,
        r#"
[crate.pocket-contract]
group = "contract"
deps = []
web = true
any = true

[crate.pocket-sim]
group = "game"
deps = []
web = true

[crate.pocket-physics]
group = "game"
deps = ["pocket-sim"]
web = true

[crate.pocket-render]
group = "presenter"
deps = ["pocket-sim"]
web = true
planned = true

[group.game]
forbid = ["wgpu", "tokio"]
forbid_direct = ["js-sys"]

[features.rapier3d]
require = ["enhanced-determinism"]
forbid = ["parallel"]

[build]
cflags_must_contain = "-ffp-contract=off"
rustflags_forbid = ["target-cpu", "+fma"]
"#,
    )
    .unwrap()
}

fn good_flags() -> Flags {
    Flags {
        cflags: vec![(
            ".cargo/config.toml [env] CFLAGS".into(),
            "-ffp-contract=off".into(),
        )],
        rustflags: vec![],
    }
}

fn codes(step: &StepResult) -> Vec<String> {
    step.errors.iter().map(|p| p.code.clone()).collect()
}

const MEMBERS: &[&str] = &["pocket-contract", "pocket-sim", "pocket-physics"];

#[test]
fn a_graph_that_follows_the_table_passes() {
    let m = meta(
        &[
            ("pocket-contract", &[], &[]),
            (
                "pocket-sim",
                &[("pocket-contract", None), ("bevy_ecs", None)],
                &[],
            ),
            (
                "pocket-physics",
                &[
                    ("pocket-sim", None),
                    ("rapier3d", None),
                    ("pocket-contract", Some("dev")),
                ],
                &[],
            ),
            ("bevy_ecs", &[], &[]),
            (
                "rapier3d",
                &[("tokio", Some("dev"))],
                &["enhanced-determinism", "std"],
            ),
            ("tokio", &[], &[]),
        ],
        MEMBERS,
    );
    let step = check(&graph(), &[("host".into(), m)], &good_flags());
    assert_eq!(step.verdict, Verdict::Pass, "{:?}", step.errors);
}

/// The `bad-edge` control: an edge the table does not allow, here from a dev-dependency.
#[test]
fn bad_edge_control() {
    let m = meta(
        &[
            ("pocket-contract", &[], &[]),
            ("pocket-sim", &[("pocket-physics", Some("dev"))], &[]),
            ("pocket-physics", &[("pocket-sim", None)], &[]),
        ],
        MEMBERS,
    );
    let step = check(&graph(), &[("host".into(), m)], &good_flags());
    assert_eq!(codes(&step), ["deps.edge_not_allowed"]);
    assert_eq!(step.errors[0].detail["from"], json!("pocket-sim"));
    assert_eq!(step.errors[0].detail["to"], json!("pocket-physics"));
    assert_eq!(step.errors[0].detail["kind"], json!("dev"));
}

/// The `bad-feature` control: `rapier3d` with `parallel`, and without a required feature.
#[test]
fn bad_feature_control() {
    let m = meta(
        &[
            ("pocket-contract", &[], &[]),
            ("pocket-sim", &[], &[]),
            (
                "pocket-physics",
                &[("pocket-sim", None), ("rapier3d", None)],
                &[],
            ),
            ("rapier3d", &[], &["parallel"]),
        ],
        MEMBERS,
    );
    let step = check(
        &graph(),
        &[("wasm32-unknown-unknown".into(), m)],
        &good_flags(),
    );
    assert_eq!(
        codes(&step),
        ["deps.feature_not_allowed", "deps.feature_missing"]
    );
    assert_eq!(step.errors[0].detail["feature"], json!("parallel"));
    assert_eq!(
        step.errors[0].detail["target"],
        json!("wasm32-unknown-unknown")
    );
    assert_eq!(
        step.errors[1].detail["feature"],
        json!("enhanced-determinism")
    );
}

#[test]
fn a_forbidden_crate_below_the_game_group_is_named_with_its_chain() {
    let m = meta(
        &[
            ("pocket-contract", &[], &[]),
            ("pocket-sim", &[("helper", None), ("js-sys", None)], &[]),
            ("pocket-physics", &[("pocket-sim", None)], &[]),
            ("helper", &[("wgpu", None)], &[]),
            ("wgpu", &[], &[]),
            ("js-sys", &[], &[]),
        ],
        MEMBERS,
    );
    let step = check(&graph(), &[("host".into(), m)], &good_flags());
    let found: Vec<(String, String, Value)> = step
        .errors
        .iter()
        .map(|p| {
            (
                p.code.clone(),
                p.detail["crate"].as_str().unwrap().to_string(),
                p.detail["chain"].clone(),
            )
        })
        .collect();
    assert!(found.contains(&(
        "deps.external_not_allowed".into(),
        "pocket-sim".into(),
        json!(["pocket-sim", "helper", "wgpu"])
    )));
    assert!(found.contains(&(
        "deps.external_not_allowed".into(),
        "pocket-sim".into(),
        json!(["pocket-sim", "js-sys"])
    )));
    // pocket-physics reaches wgpu through pocket-sim: reported with its own chain.
    assert!(found.contains(&(
        "deps.external_not_allowed".into(),
        "pocket-physics".into(),
        json!(["pocket-physics", "pocket-sim", "helper", "wgpu"])
    )));
}

#[test]
fn unlisted_and_missing_crates() {
    let m = meta(
        &[
            ("pocket-contract", &[], &[]),
            ("pocket-sim", &[], &[]),
            ("pocket-new", &[], &[]),
        ],
        &["pocket-contract", "pocket-sim", "pocket-new"],
    );
    let step = check(&graph(), &[("host".into(), m)], &good_flags());
    // pocket-physics is listed and not a member; pocket-render is planned, so it may be absent.
    assert_eq!(codes(&step), ["deps.crate_unlisted", "deps.crate_missing"]);
    assert_eq!(step.errors[0].detail["crate"], json!("pocket-new"));
    assert_eq!(step.errors[1].detail["crate"], json!("pocket-physics"));
}

#[test]
fn build_flags() {
    let m = meta(
        &[
            ("pocket-contract", &[], &[]),
            ("pocket-sim", &[], &[]),
            ("pocket-physics", &[("pocket-sim", None)], &[]),
        ],
        MEMBERS,
    );
    let flags = Flags {
        cflags: vec![
            (
                ".cargo/config.toml [env] CFLAGS".into(),
                "-ffp-contract=off".into(),
            ),
            ("environment CFLAGS".into(), "-O2".into()),
        ],
        rustflags: vec![(
            "environment RUSTFLAGS".into(),
            "-C target-cpu=native".into(),
        )],
    };
    let step = check(&graph(), &[("host".into(), m)], &flags);
    assert_eq!(
        codes(&step),
        ["deps.cflags_missing", "deps.rustflags_not_allowed"]
    );
    assert_eq!(step.errors[0].detail["source"], json!("environment CFLAGS"));
}

#[test]
fn the_committed_graph_file_parses_and_mirrors_architecture_md() {
    // Found at run time: `env!` would bake in the directory the test binary was built in.
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo test sets it");
    let root = std::path::Path::new(&manifest).parent().unwrap();
    let g = load(root).unwrap();
    for name in [
        "pocket-sim",
        "pocket-runtime",
        "pocket-web",
        "xtask",
        "pocket-contract",
    ] {
        assert!(g.crates.contains_key(name), "{name}");
    }
    assert!(g.crates["pocket-contract"].any);
    assert!(g.group["game"].forbid.contains(&"wgpu".to_string()));
    assert_eq!(g.build.cflags_must_contain, "-ffp-contract=off");
}

/// cargo metadata lists an optional dependency no feature turns on (bevy_ecs with `std` only and
/// its `bevy_reflect`); it is not built, so what lies below it is not reported.
#[test]
fn optional_dependencies_count_only_when_a_feature_turns_them_on() {
    let metadata = |ecs_features: Value| -> Metadata {
        serde_json::from_value(json!({
            "packages": [
                {"id": "sim", "name": "pocket-sim", "dependencies": [{"name": "bevy_ecs", "kind": null}]},
                {"id": "contract", "name": "pocket-contract"},
                {"id": "physics", "name": "pocket-physics"},
                {"id": "ecs", "name": "bevy_ecs",
                 "dependencies": [{"name": "bevy_reflect", "kind": null, "optional": true}],
                 "features": {"bevy_reflect": ["dep:bevy_reflect"], "std": ["bevy_reflect?/std"]}},
                {"id": "reflect", "name": "bevy_reflect", "dependencies": [{"name": "wgpu", "kind": null}]},
                {"id": "wgpu", "name": "wgpu"}
            ],
            "workspace_members": ["sim", "contract", "physics"],
            "resolve": {"nodes": [
                {"id": "sim", "deps": [{"pkg": "ecs", "dep_kinds": [{"kind": null}]}], "features": []},
                {"id": "contract", "deps": [], "features": []},
                {"id": "physics", "deps": [], "features": []},
                {"id": "ecs", "deps": [{"pkg": "reflect", "dep_kinds": [{"kind": null}]}], "features": ecs_features},
                {"id": "reflect", "deps": [{"pkg": "wgpu", "dep_kinds": [{"kind": null}]}], "features": []},
                {"id": "wgpu", "deps": [], "features": []}
            ]}
        }))
        .unwrap()
    };
    let off = check(
        &graph(),
        &[("wasm32-unknown-unknown".into(), metadata(json!(["std"])))],
        &good_flags(),
    );
    assert!(
        off.errors
            .iter()
            .all(|p| p.code != "deps.external_not_allowed"),
        "{:?}",
        off.errors
    );
    let on = check(
        &graph(),
        &[(
            "wasm32-unknown-unknown".into(),
            metadata(json!(["std", "bevy_reflect"])),
        )],
        &good_flags(),
    );
    let chain = &on
        .errors
        .iter()
        .find(|p| p.code == "deps.external_not_allowed")
        .unwrap()
        .detail["chain"];
    assert_eq!(
        chain,
        &json!(["pocket-sim", "bevy_ecs", "bevy_reflect", "wgpu"])
    );
}

use serde_json::Value;
