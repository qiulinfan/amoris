#[path = "common/water_sea.rs"]
mod experiment;
#[path = "../src/water_compute.rs"]
mod water_compute;
use amoris_link::Source;
use amoris_runtime::Command;
use serde_json::json;
use std::sync::Arc;
fn run(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(amoris_runtime::GAME_STACK_BYTES)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap();
}
#[test]
fn real_sea_game_uses_filtered_waves_and_repeats_forks_restores_and_published_checkpoints() {
    run(|| {
        let mut g = water_compute::builder(Arc::new(experiment::shore()), 1)
            .build()
            .unwrap();
        let feed = amoris_assets::Feed::new();
        let mailbox = feed.subscribe();
        let mut extractor = amoris_runtime::Extractor::default();
        for _ in 0..128 {
            experiment::steps(&mut g, 1);
            g.present(&mut extractor, &feed);
            let f = mailbox.take();
            if g.tick().0.is_multiple_of(64) {
                let cp = f.resident_water[0].checkpoint.as_ref().unwrap();
                let actual = cp
                    .columns
                    .iter()
                    .map(|c| f64::from(f32::from_bits(c[1])) * 0.0625)
                    .sum::<f64>();
                let state = experiment::read(&mut g, "Shore", json!(["WaterResident"]));
                let b = &state["WaterResident"]["boundary"];
                let expected = b["initial_volume"].as_f64().unwrap()
                    + b["entered_total"].as_f64().unwrap()
                    - b["left_total"].as_f64().unwrap();
                assert!((actual - expected).abs() < 0.002);
            }
        }
        let state = experiment::read(&mut g, "Shore", json!(["WaterResident", "WaterRegion"]));
        assert!(state["WaterRegion"]["tiles"].as_array().unwrap().is_empty());
        let steps = state["WaterResident"]["recipe"]["steps"]
            .as_array()
            .unwrap();
        assert!(steps.is_empty());
        let saved = g.snapshot().unwrap();
        let mut fork = g.fork().unwrap();
        for _ in 0..16 {
            experiment::steps(&mut g, 1);
            experiment::steps(&mut fork, 1);
            assert_eq!(g.world_hash().unwrap(), fork.world_hash().unwrap());
        }
        let state = experiment::read(&mut g, "Shore", json!(["WaterResident"]));
        let last = state["WaterResident"]["recipe"]["steps"]
            .as_array()
            .unwrap()
            .last()
            .unwrap();
        assert_eq!(
            last["sea"]["waves"].as_array().unwrap().len(),
            2,
            "local wavelength filters did not remove the short wave"
        );
        let target = g.world_hash().unwrap();
        g.restore(&saved).unwrap();
        experiment::steps(&mut g, 16);
        assert_eq!(g.world_hash().unwrap(), target);
        g.apply(&Command::new(Source::Developer(0),2,"world.edit",json!({"label":"sea phase","ops":[{"set":{"entity":"Sea","component":"Sea","value":{"waves":[{"length":64,"height":0.08,"toward_deg":90,"phase":0.4}]}}}]}))).unwrap();
        experiment::steps(&mut g, 1);
        let after = experiment::read(&mut g, "Shore", json!(["WaterResident"]));
        assert_eq!(
            after["WaterResident"]["recipe"]["steps"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()["sea"]["waves"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    });
}
#[test]
fn missing_and_deep_sea_edges_fail_explicitly_before_publishing_an_advanced_field() {
    run(|| {
        let mut s = experiment::shore();
        s.scene
            .entities
            .retain(|e| !e.components.contains_key("Sea"));
        let mut g = water_compute::builder(Arc::new(s), 1).build().unwrap();
        assert_eq!(g.step().unwrap_err().code, "water.no_sea");
        let mut s = experiment::shore();
        s.scene.entities[2].components["Sea"]["waves"] =
            json!([{"length":8,"height":0.01,"toward_deg":90,"phase":0}]);
        let mut g = water_compute::builder(Arc::new(s), 1).build().unwrap();
        assert_eq!(g.step().unwrap_err().code, "water.deep_edge");
    });
}

#[test]
fn actual_game_waves_propagate_at_shallow_speed_with_the_declared_damping() {
    run(|| {
        let mut g = water_compute::builder(Arc::new(experiment::tank(0.25)), 1)
            .build()
            .unwrap();
        let mut samples = Vec::new();
        for tick in 1..=1200 {
            experiment::steps(&mut g, 1);
            if tick >= 480 && tick % 5 == 0 {
                let rows = experiment::probes(&mut g);
                let rows = rows.as_array().unwrap();
                assert_eq!(rows.len(), 3);
                let mut y = [0.0; 3];
                for (i, r) in rows.iter().enumerate() {
                    assert_eq!(r["WaterProbe"]["tick"].as_u64().unwrap(), tick);
                    assert_eq!(r["WaterProbe"]["ready"], true);
                    y[i] = r["WaterProbe"]["surface"].as_f64().unwrap();
                }
                samples.push((tick as f64 / 60.0, y));
            }
        }
        let fit = std::array::from_fn::<_, 3, _>(|i| experiment::harmonic(&samples, i));
        let omega = (9.81 * std::f64::consts::TAU / 16.0).sqrt();
        let reference = (9.81 * 0.125f64).sqrt();
        let expected_decay = (-0.2 / reference).exp();
        for i in 0..2 {
            let phase = (fit[i + 1][1] - fit[i][1]).rem_euclid(std::f64::consts::TAU);
            let speed = omega * 2.0 / phase;
            let attenuation = fit[i + 1][0] / fit[i][0];
            assert!(
                (speed / reference - 1.0).abs() < 0.02,
                "measured shallow speed {speed}/{reference}"
            );
            assert!(
                (attenuation / expected_decay - 1.0).abs() < 0.03,
                "damped amplitude {attenuation}/{expected_decay}"
            );
            eprintln!(
                "actual sea propagation: speed={speed}, reference={reference}, amplitude_ratio={attenuation}, expected={expected_decay}"
            );
        }
        for f in fit {
            assert!(f[0] > 0.0003 && f[0] < 0.0012);
            assert!(f[2] < 0.05 * f[0], "nonharmonic response {f:?}");
        }
    });
}
