//! `assets.upload` through the host (docs/spec/server.md 3, the editor's Import, editor.md 8 item
//! 15): a file sent in base64 parts lands in the project, byte for byte, listed by `assets.list`
//! and served at `/assets/<path>`; a part that does not start where the others ended, a path out of
//! the project, an existing file without `overwrite` and an oversized part are refused and leave
//! the project as it was.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use amoris_link::Source;
use amoris_runtime::thread::{GameHandle, GameThread, Pacing, ThreadOptions};
use amoris_runtime::{GameBuilder, Project};
use amoris_server::{GameAccess, Host, Via};
use serde_json::{Value, json};

/// A fresh copy of samples/anim to write into.
fn scratch(name: &str) -> PathBuf {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../samples/anim");
    let dst = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dst);
    copy(&src, &dst);
    dst
}

fn copy(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for e in std::fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let to = dst.join(e.file_name());
        if e.path().is_dir() {
            copy(&e.path(), &to);
        } else {
            std::fs::copy(e.path(), to).unwrap();
        }
    }
}

fn start(root: &Path) -> (GameHandle, Host) {
    let setup = Arc::new(Project::load(root).unwrap().setup(false).unwrap());
    let t0 = Instant::now();
    let clock: amoris_runtime::thread::Clock =
        Arc::new(move || t0.elapsed().as_secs_f64() * 1000.0);
    let dir = root.to_path_buf();
    let handle = GameThread::spawn(
        move || GameBuilder::new(setup).project(dir).build(),
        ThreadOptions {
            pacing: Pacing::RealTime { speed: 1.0 },
            paused: true,
            ..ThreadOptions::new(clock)
        },
    )
    .unwrap();
    let clients = handle.clients();
    let access = GameAccess {
        reader: handle.reader(),
        editor: clients.client(Source::Editor).unwrap(),
        api: clients.developer(),
        developer: Arc::new(move || clients.developer()),
    };
    (handle, Host::new(access, root.to_path_buf(), None))
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// RFC 4648 base64, as a browser's `btoa` writes it.
fn b64(bytes: &[u8]) -> String {
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for k in 0..4 {
            if k <= c.len() {
                out.push(B64[((n >> (18 - 6 * k)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[test]
fn an_upload_in_parts_lands_in_the_project_and_refusals_change_nothing() {
    let root = scratch("assets_upload");
    let (handle, host) = start(&root);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let call = |method: &'static str, params: Value| {
            let host = host.clone();
            async move { host.call(&Via::Editor, method, params).await }
        };
        let catalog = call("catalog.list", json!({})).await.unwrap();
        let entry = catalog
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "assets.upload")
            .cloned()
            .unwrap();
        assert_eq!(entry["kind"], json!("request"), "{entry:#}");
        assert!(
            entry["params"]["properties"]["data"].is_object(),
            "{entry:#}"
        );

        // 2.5 MiB in three parts, of 1 MiB, 1 MiB and the rest: each part answers what has come.
        let part = amoris_server::UPLOAD_PART;
        let file: Vec<u8> = (0..part * 5 / 2).map(|i| (i * 7 + i / 977) as u8).collect();
        let mut offset = 0;
        for (n, chunk) in file.chunks(part).enumerate() {
            let done = offset + chunk.len() == file.len();
            let r = call(
                "assets.upload",
                json!({"path": "models/up/lighthouse.glb", "data": b64(chunk),
                       "offset": offset, "done": done}),
            )
            .await
            .unwrap_or_else(|e| panic!("part {n}: {e:#?}"));
            offset += chunk.len();
            if done {
                assert_eq!(
                    r,
                    json!({"path": "models/up/lighthouse.glb", "kind": "model",
                           "bytes": file.len()})
                );
            } else {
                assert_eq!(
                    r,
                    json!({"path": "models/up/lighthouse.glb", "received": offset})
                );
                // Not in place, nor listed, before the last part.
                assert!(!root.join("models/up/lighthouse.glb").exists());
            }
        }
        assert_eq!(
            std::fs::read(root.join("models/up/lighthouse.glb")).unwrap(),
            file,
            "the bytes"
        );
        let listed = call("assets.list", json!({})).await.unwrap();
        let paths: Vec<&str> = listed
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["path"].as_str().unwrap())
            .collect();
        assert!(paths.contains(&"models/up/lighthouse.glb"), "{paths:?}");
        assert!(paths.iter().all(|p| !p.starts_with(".amoris")), "{paths:?}");
        let staged: Vec<_> = std::fs::read_dir(root.join(".amoris/uploads"))
            .unwrap()
            .collect();
        assert!(staged.is_empty(), "the staging file was moved: {staged:?}");

        // One part, done by default.
        let r = call(
            "assets.upload",
            json!({"path": "textures/dot.png", "data": b64(b"\x89PNG")}),
        )
        .await
        .unwrap();
        assert_eq!(
            r,
            json!({"path": "textures/dot.png", "kind": "texture", "bytes": 4})
        );

        // An existing file is replaced only with overwrite.
        let e = call(
            "assets.upload",
            json!({"path": "textures/dot.png", "data": b64(b"other")}),
        )
        .await
        .unwrap_err();
        assert_eq!(e.code, "assets.exists", "{e:#?}");
        assert_eq!(
            std::fs::read(root.join("textures/dot.png")).unwrap(),
            b"\x89PNG"
        );
        call(
            "assets.upload",
            json!({"path": "textures/dot.png", "data": b64(b"other"), "overwrite": true}),
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read(root.join("textures/dot.png")).unwrap(),
            b"other"
        );

        // A part that does not start where the parts before it ended.
        call(
            "assets.upload",
            json!({"path": "files/a.bin", "data": b64(b"abc"), "done": false}),
        )
        .await
        .unwrap();
        let e = call(
            "assets.upload",
            json!({"path": "files/a.bin", "data": b64(b"def"), "offset": 5}),
        )
        .await
        .unwrap_err();
        assert_eq!(e.code, "assets.upload_offset", "{e:#?}");
        assert_eq!(e.detail["expected"], json!(3), "{e:#?}");
        assert!(!root.join("files/a.bin").exists());
        // From where it ended, it completes.
        call(
            "assets.upload",
            json!({"path": "files/a.bin", "data": b64(b"def"), "offset": 3}),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(root.join("files/a.bin")).unwrap(), b"abcdef");

        // Paths out of the project, into a dot directory or naming no file; data not base64; a
        // part over the limit.
        for path in [
            "../escape.bin",
            "/tmp/escape.bin",
            ".amoris/x.bin",
            "models/",
            "",
        ] {
            let e = call("assets.upload", json!({"path": path, "data": b64(b"x")}))
                .await
                .unwrap_err();
            assert_eq!(e.code, "request.invalid_value", "{path}: {e:#?}");
            assert_eq!(e.detail["path"], json!("/path"), "{path}: {e:#?}");
        }
        assert!(!root.join("../escape.bin").exists());
        let e = call(
            "assets.upload",
            json!({"path": "files/b.bin", "data": "not base64!"}),
        )
        .await
        .unwrap_err();
        assert_eq!(e.code, "request.invalid_value", "{e:#?}");
        assert_eq!(e.detail["path"], json!("/data"), "{e:#?}");
        let e = call(
            "assets.upload",
            json!({"path": "files/b.bin", "data": b64(&vec![0u8; part + 1])}),
        )
        .await
        .unwrap_err();
        assert_eq!(e.code, "request.invalid_value", "{e:#?}");
        assert!(!root.join("files/b.bin").exists());
    });
    drop(host);
    handle.shutdown(5000).unwrap();
}

/// A directory that is a link out of the project is refused at the last part, and nothing is
/// written outside.
#[cfg(unix)]
#[test]
fn an_upload_through_a_link_out_of_the_project_is_refused() {
    let root = scratch("assets_upload_link");
    let outside = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("assets_upload_outside");
    let _ = std::fs::remove_dir_all(&outside);
    std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("linked")).unwrap();
    let (handle, host) = start(&root);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let e = rt
        .block_on(host.call(
            &Via::Editor,
            "assets.upload",
            json!({"path": "linked/x.bin", "data": b64(b"x")}),
        ))
        .unwrap_err();
    assert_eq!(e.code, "request.invalid_value", "{e:#?}");
    assert!(!outside.join("x.bin").exists());
    drop(host);
    handle.shutdown(5000).unwrap();
}
