//! The SDK's types (script-host.md 12, test 7), with the real `tsc`: `pocket check`'s `types` step
//! over tests/fixtures/sdk/every, a component with one field of every type and a system using every
//! API of the prelude, writes the project's declarations and type-checks it clean, the builder's
//! value type and the emitted one included (the fixture proves them mutually assignable); a
//! misspelt `"Component.field"` is then a `types.error` naming the field it meant. Skipped, with a
//! line saying so, when no `tsc` is installed (`cd sdk && bun install`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pocket_check::{Options, Report, Verdict, check_project};

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let target = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &target);
        } else {
            std::fs::copy(e.path(), target).unwrap();
        }
    }
}

fn check_types(dir: PathBuf) -> Report {
    std::thread::Builder::new()
        .stack_size(pocket_runtime::GAME_STACK_BYTES)
        .spawn(move || {
            let opts = Options {
                only: Some(vec!["types".into()]),
                typecheck: Some(Arc::new(|d: &Path| {
                    pocket_server::typecheck::run_blocking(d)
                })),
                ..Options::default()
            };
            check_project(&dir, &opts)
        })
        .unwrap()
        .join()
        .unwrap()
}

#[test]
fn the_declarations_type_check_every_field_type_and_api() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("sdk-every");
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(&repo.join("tests/fixtures/sdk/every"), &dir);
    if pocket_server::typecheck::find_tsc(&dir).is_none() {
        eprintln!("skipped: no tsc (POCKET_TSC, or `bun install` in sdk/)");
        return;
    }

    let r = check_types(dir.clone());
    let step = &r.steps[0];
    assert_eq!(step.verdict, Verdict::Pass, "{:#?}", step.errors);
    assert!(
        step.measurements.iter().any(|m| m.name == "tsc"),
        "{step:#?}"
    );
    let components = std::fs::read_to_string(dir.join(".pocket/types/components.d.ts")).unwrap();
    assert!(
        components.contains("readonly a_enum: \"calm\" | \"cross\";"),
        "{components}"
    );
    assert!(dir.join("tsconfig.json").is_file());

    let rules = dir.join("scripts/rules.ts");
    let text = std::fs::read_to_string(&rules).unwrap();
    std::fs::write(
        &rules,
        text.replacen("\"Every.a_f64\"", "\"Every.a_f46\"", 1),
    )
    .unwrap();
    let r = check_types(dir);
    let step = &r.steps[0];
    assert_eq!(step.verdict, Verdict::Fail);
    let e = &step.errors[0];
    assert_eq!(e.code, "types.error", "{e:#?}");
    assert_eq!(e.detail["path"], "scripts/rules.ts", "{e:#?}");
    assert_eq!(e.detail["code"], "TS2820", "{e:#?}");
    assert!(
        e.message.ends_with("Did you mean '\"Every.a_f64\"'?"),
        "{}",
        e.message
    );
}
