//! Suspended water ticks through the real worker loop. An identity host isolates continuation
//! ordering; actual browser tests run the GPU kernels and asynchronous readback.
use amoris_runtime::{
    GameBuilder, Project,
    water_compute::{ComputeQueue, Output, Request, install},
};
use amoris_web::{
    Pacing,
    worker::{Next, Out, WorkerCore},
};
use serde_json::{Value, json};
use std::sync::Arc;

fn run(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(amoris_runtime::GAME_STACK_BYTES)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap();
}
fn setup(queue: ComputeQueue) -> WorkerCore {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/scenes/water-dam");
    let mut setup = Project::load(&root).unwrap().setup(false).unwrap();
    let mut second = setup.scene.entities[1].clone();
    second.name = Some("Second".into());
    second.components.remove("Lake");
    second
        .components
        .insert("Transform".into(), json!({"position":[64.0,0.0,0.0]}));
    setup.scene.entities.push(second);
    setup.scene.entities.push(
        serde_json::from_value(json!({"name":"Spring","components":{
            "WaterSource":{"region":3,"at":[64.5,0.5],"rate_m3s":6.0}
        }}))
        .unwrap(),
    );
    let game = GameBuilder::new(Arc::new(setup))
        .system(move |s| {
            install(s, queue.backend());
            Ok(())
        })
        .build()
        .unwrap();
    let mut worker = WorkerCore::new(game, Pacing::Stepped, Arc::new(|| 0.0)).unwrap();
    worker.take_out();
    worker.ack(1);
    worker
}
fn cmd(seq: u64, name: &str, params: Value) -> Value {
    json!({"t":"cmd","source":{"developer":0},"seq":seq,"name":name,"params":params.to_string()})
}
fn identity(r: &Request) -> Output {
    let [nx, nz] = r.size;
    Output {
        columns: (0..nx * nz)
            .map(|n| [r.depth[n], r.u[n % nx + (nx + 1) * (n / nx)], r.w[n]])
            .collect(),
    }
}

#[test]
fn waiting_water_keeps_commands_snapshots_and_sources_at_the_right_boundary() {
    run(|| {
        let queue = ComputeQueue::default();
        let mut w = setup(queue.clone());
        w.push(&cmd(1, "step", json!({"ticks":2})));
        assert_eq!(w.run(8.0), Next::External);
        let first = queue.take_request().unwrap().unwrap();
        assert!(
            w.game().snapshot().is_err(),
            "a suspended tick is not a snapshot boundary"
        );
        w.push(&cmd(
            2,
            "world.get",
            json!({"entity":"Second","fields":["WaterRegion"]}),
        ));
        for _ in 0..3 {
            assert_eq!(w.run(8.0), Next::External);
        }
        assert!(
            queue.take_request().unwrap().is_none(),
            "polling must not resubmit the job"
        );
        assert!(w.take_out().iter().all(|o| !matches!(o, Out::Snap(..))));
        assert!(queue.complete(first.id, Ok(identity(&first))).unwrap());
        assert_eq!(w.run(8.0), Next::External);
        let second = queue.take_request().unwrap().unwrap();
        assert!((second.depth.iter().sum::<f64>() - 0.1).abs() < 1e-12);
        for _ in 0..3 {
            assert_eq!(w.run(8.0), Next::External);
            w.ack(1);
        }
        assert!(w.take_out().iter().all(|o| !matches!(o, Out::Snap(..))));
        assert!(queue.complete(second.id, Ok(identity(&second))).unwrap());
        // Tick 1 completes, the queued read runs at its boundary, then tick 2 suspends.
        assert_eq!(w.run(8.0), Next::Now);
        assert!(w.game().snapshot().is_ok());
        assert_eq!(w.run(8.0), Next::External);
        let mut messages = w.take_out();
        let reply = messages
            .iter()
            .find_map(|o| match o {
                Out::Message(m) if m["seq"] == 2 => Some(m),
                _ => None,
            })
            .unwrap();
        let depth: f64 = reply["ok"]["fields"]["WaterRegion"]["tiles"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|t| t["depth"].as_array().unwrap())
            .map(|v| v.as_f64().unwrap())
            .sum();
        assert!(
            (depth - 0.1).abs() < 1e-12,
            "sources must apply once, not on each poll"
        );
        let third = queue.take_request().unwrap().unwrap();
        assert!(
            (third.depth.iter().sum::<f64>() - 1024.0).abs() < 1e-12,
            "the script ran once on tick 2"
        );
        queue.complete(third.id, Ok(identity(&third))).unwrap();
        assert_eq!(w.run(8.0), Next::External);
        let fourth = queue.take_request().unwrap().unwrap();
        assert!((fourth.depth.iter().sum::<f64>() - 0.2).abs() < 1e-12);
        queue.complete(fourth.id, Ok(identity(&fourth))).unwrap();
        assert_eq!(w.run(8.0), Next::Now);
        assert_eq!(w.run(8.0), Next::Command);
        messages.extend(w.take_out());
        assert!(
            messages
                .iter()
                .any(|o| matches!(o,Out::Message(m) if m["seq"]==1 && m["ok"]["tick"]==2))
        );
        assert_eq!(w.game().snapshot().unwrap().header().tick.0, 2);
    });
}

#[test]
fn asynchronous_failure_republishes_the_last_complete_world() {
    run(|| {
        let queue = ComputeQueue::default();
        let mut w = setup(queue.clone());
        w.push(&cmd(1, "step", json!({"ticks":1})));
        assert_eq!(w.run(8.0), Next::External);
        let job = queue.take_request().unwrap().unwrap();
        queue
            .complete(
                job.id,
                Err(amoris_contract::Problem::new(
                    "water.compute_failed",
                    "test device lost",
                    amoris_contract::detail([]),
                )),
            )
            .unwrap();
        assert_eq!(w.run(8.0), Next::Now);
        assert_eq!(w.run(8.0), Next::Command);
        assert_eq!(
            w.game().sim().poisoned().unwrap().problem.code,
            "water.compute_failed"
        );
        let out = w.take_out();
        let snaps: Vec<_> = out
            .iter()
            .filter_map(|o| match o {
                Out::Snap(meta, _) => Some(meta),
                _ => None,
            })
            .collect();
        assert!(!snaps.is_empty());
        assert!(
            snaps
                .iter()
                .all(|m| m["header"]["tick"] == 0 && m["time"]["halted"] == true)
        );
        assert!(
            !queue.complete(job.id, Ok(identity(&job))).unwrap(),
            "a late response cannot overwrite the consumed failure"
        );
    });
}
