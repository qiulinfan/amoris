//! Complete immutable uploads preserve every stored bit and replace prior inactive masks.
use amoris_assets::water_field::{FieldBoundary, FieldCheckpoint, FieldStep};
use amoris_render::{BackendChoice, Gpu, water_compute::GpuWater};
#[test]
fn checked_complete_upload_preserves_words_faces_and_replaces_masked_state() {
    for backend in BackendChoice::under_test() {
        let gpu = Gpu::headless(backend).expect("checked upload needs GPU");
        gpu.device
            .on_uncaptured_error(std::sync::Arc::new(|e| panic!("checked upload: {e}")));
        let size = [37, 19];
        let n = (size[0] * size[1]) as usize;
        let mut raw = FieldCheckpoint::new(
            size,
            1234567.5,
            (0..n)
                .map(|i| {
                    [
                        0,
                        if i % 37 < 18 {
                            2.0f32.to_bits()
                        } else {
                            1.0f32.to_bits()
                        },
                        0,
                        0,
                    ]
                })
                .collect(),
        );
        raw.columns[0][2] = (-0.0f32).to_bits();
        raw.far_faces[3] = (-0.0f32).to_bits();
        raw.far_faces[4] = 0.5f32.to_bits();
        let checked = raw.clone().checked().unwrap();
        let mut a = GpuWater::new(&gpu.device, &gpu.queue, size).unwrap();
        let mut b = GpuWater::new(&gpu.device, &gpu.queue, size).unwrap();
        a.upload(
            &vec![[0.0, 0.0, 0.0, 0.0]; n],
            &(0..n).map(|i| u32::from(i % 2 == 0)).collect::<Vec<_>>(),
        )
        .unwrap();
        a.upload_checked_checkpoint(&checked).unwrap();
        b.upload_checkpoint(&raw).unwrap();
        let read = a.checkpoint_complete(raw.datum).unwrap();
        assert_eq!(read.columns, raw.columns);
        assert_eq!(read.far_faces, raw.far_faces);
        assert_eq!(read.datum.to_bits(), raw.datum.to_bits());
        let other = FieldCheckpoint::new([16, 16], 0.0, vec![[0; 4]; 256])
            .checked()
            .unwrap();
        assert!(a.upload_checked_checkpoint(&other).is_err());
        assert_eq!(a.checkpoint_complete(raw.datum).unwrap(), read);
        let step = FieldStep {
            exterior: vec![],
            sea: None,
            boundaries: [FieldBoundary::Wall; 4],
            cell_m: 0.5,
            substep_dt: 0.005,
            substeps: 2,
            friction: 0.02,
            calm_per_s: amoris_assets::water_field::DEFAULT_CALM_PER_S,
            dry_below: 0.0,
            edits: vec![],
            impulses: vec![],
            displacements: vec![],
            physics: Default::default(),
            wind: None,
            rain_m: 0.0,
        };
        for _ in 0..3 {
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            a.encode_transport(&mut encoder, &step, None).unwrap();
            b.encode_transport(&mut encoder, &step, None).unwrap();
            gpu.queue.submit([encoder.finish()]);
            assert_eq!(
                a.checkpoint_complete(raw.datum).unwrap(),
                b.checkpoint_complete(raw.datum).unwrap()
            );
        }
    }
}
