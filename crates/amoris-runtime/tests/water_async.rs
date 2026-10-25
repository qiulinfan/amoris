//! Asynchronous water completion respects the same snapshot/fork/restore boundaries as sync
//! water. The identity executor isolates scheduler semantics from numerical solver tests.
#![cfg(feature = "transpile")]
use amoris_contract::{Problem, detail};
use amoris_link::Source;
use amoris_runtime::{
    Command, Game, GameBuilder, Project,
    water_compute::{ComputeQueue, Output, Request, install},
};
use serde_json::json;
use std::sync::Arc;

fn identity(r: &Request) -> Output {
    let [nx, nz] = r.size;
    Output {
        columns: (0..nx * nz)
            .map(|n| [r.depth[n], r.u[n % nx + (nx + 1) * (n / nx)], r.w[n]])
            .collect(),
    }
}
fn step(g: &mut Game, q: &ComputeQueue) {
    for _ in 0..16 {
        if g.poll_step().unwrap().is_ready() {
            return;
        }
        let r = q
            .take_request()
            .unwrap()
            .expect("a suspended water request");
        q.complete(r.id, Ok(identity(&r))).unwrap();
    }
    panic!("water failed to complete");
}

#[test]
fn async_boundaries_fork_restore_refuse_partial_access_and_ignore_late_failure_results() {
    std::thread::Builder::new()
        .stack_size(amoris_runtime::GAME_STACK_BYTES)
        .spawn(|| {
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures/scenes/water-dam");
            let setup = Arc::new(Project::load(&root).unwrap().setup(false).unwrap());
            let queue = ComputeQueue::default();
            let host = queue.clone();
            let mut game = GameBuilder::new(setup)
                .system(move |s| {
                    install(s, host.backend());
                    Ok(())
                })
                .build()
                .unwrap();
            step(&mut game, &queue);
            step(&mut game, &queue);
            let saved = game.snapshot().unwrap();
            let mut fork = game.fork().unwrap();
            for _ in 0..4 {
                step(&mut game, &queue);
                step(&mut fork, &queue);
                assert_eq!(game.world_hash().unwrap(), fork.world_hash().unwrap());
            }
            game.restore(&saved).unwrap();
            let mut expected = game.fork().unwrap();
            let feed = amoris_assets::Feed::new();
            let mailbox = feed.subscribe();
            let mut extractor = amoris_runtime::Extractor::new();
            game.present(&mut extractor, &feed);
            let _ = mailbox.take();
            assert!(game.poll_step().unwrap().is_pending());
            let old = queue.take_request().unwrap().unwrap();
            assert!(game.snapshot().is_err());
            assert!(game.fork().is_err());
            assert_eq!(
                game.apply(&Command::new(
                    Source::Developer(0),
                    1,
                    "world.get",
                    json!({"entity":"Lake","fields":["WaterRegion"]})
                ))
                .unwrap_err()
                .code,
                "sim.external_pending"
            );
            assert_eq!(
                game.restore(&saved).unwrap_err().code,
                "sim.external_pending"
            );
            game.present(&mut extractor, &feed);
            assert!(mailbox.take().is_empty());
            queue
                .complete(
                    old.id,
                    Err(Problem::new(
                        "water.compute_failed",
                        "test lost device",
                        detail([]),
                    )),
                )
                .unwrap();
            assert_eq!(game.poll_step().unwrap_err().code, "water.compute_failed");
            game.present(&mut extractor, &feed);
            assert!(mailbox.take().is_empty());
            game.restore(&saved).unwrap();
            assert!(!queue.complete(old.id, Ok(identity(&old))).unwrap());
            step(&mut game, &queue);
            step(&mut expected, &queue);
            assert_eq!(game.world_hash().unwrap(), expected.world_hash().unwrap());
        })
        .unwrap()
        .join()
        .unwrap();
}
