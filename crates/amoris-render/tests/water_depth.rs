//! Actual production depth stage against an independent f64 shared-face flux graph.
#[path = "../examples/common/depth_oracle.rs"]
mod depth_oracle;
#[path = "../examples/common/depth_transport.rs"]
#[allow(dead_code)]
mod depth_transport;
use amoris_assets::water_field::FieldCheckpoint;
use amoris_render::{BackendChoice, Gpu, water_compute::GpuWater};
#[test]
fn reconstructed_production_flux_matches_the_independent_graph_and_preserves_water() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("depth reference needs GPU");
        gpu.device.on_uncaptured_error(std::sync::Arc::new(|e| {
            panic!("depth reference validation: {e}")
        }));
        let fixtures = depth_oracle::fixtures(&gpu);
        for case in fixtures["cases"].as_array().unwrap() {
            let size = serde_json::from_value(case["size"].clone()).unwrap();
            let mut columns: Vec<[u32; 4]> =
                serde_json::from_value(case["initial"].clone()).unwrap();
            let velocity: Vec<[f32; 2]> = serde_json::from_value(case["velocity"].clone()).unwrap();
            for (c, v) in columns.iter_mut().zip(velocity) {
                c[2] = v[0].to_bits();
                c[3] = v[1].to_bits();
            }
            let initial = FieldCheckpoint::new(size, 0.0, columns);
            let dx = case["dx"].as_f64().unwrap() as f32;
            let dt = case["dt"].as_f64().unwrap() as f32;
            let mut field = GpuWater::new(&gpu.device, &gpu.queue, size).unwrap();
            field.upload_checkpoint(&initial).unwrap();
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            field
                .encode_maccormack(&mut encoder, dx, dt, 1, 0.0)
                .unwrap();
            gpu.queue.submit([encoder.finish()]);
            let actual = field.checkpoint_complete(0.0).unwrap();
            // Returned face words are the separately computed pressure/advection stage input to
            // depth transport, never an expected depth or a result from the tested height kernel.
            let velocity = actual
                .columns
                .iter()
                .map(|c| [f32::from_bits(c[2]), f32::from_bits(c[3])])
                .collect::<Vec<_>>();
            let h = initial
                .columns
                .iter()
                .map(|c| f64::from(f32::from_bits(c[1])))
                .collect::<Vec<_>>();
            let expected =
                depth_oracle::reference(size, &h, &velocity, f64::from(dx), f64::from(dt));
            let mut error = 0.0f64;
            let mut sum = 0.0;
            for (n, (c, e)) in actual.columns.iter().zip(expected).enumerate() {
                let d = f64::from(f32::from_bits(c[1]));
                assert!(d >= 0.0 && d.is_finite());
                assert_eq!(c[0], initial.columns[n][0]);
                error = error.max((d - e).abs());
                sum += d;
            }
            assert!(
                error < 3e-6,
                "production face graph {}: {error}",
                case["case"]
            );
            assert!((sum - h.iter().sum::<f64>()).abs() * f64::from(dx * dx) < 5e-6);
        }
    }
}
