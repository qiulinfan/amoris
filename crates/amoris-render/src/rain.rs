//! Rain falling through the air (water.md 67): where the feed's weather says it rains, streaks of
//! drops in a box of world-anchored cells around the camera, falling at the drops' terminal speed
//! and slanted by the wind (rain.wgsl). Presentation only: nothing is simulated or kept, every
//! drop's place follows from its cell, its slot and the drawn time.
use amoris_assets::frame::{RainView, WindSeaView};
use bytemuck::{Pod, Zeroable};

use crate::post::{DEPTH, SceneFormat};
use crate::shaders;

/// The box: cells of `CELL_M` metres, `SIDE` along x and z, `HEIGHT_M` high with its bottom
/// `BELOW_M` under the eye.
const CELL_M: f32 = 2.0;
const SIDE: u32 = 12;
const HEIGHT_M: f32 = 12.0;
const BELOW_M: f32 = 5.0;
/// The share of the drops a real rain holds that is drawn, and the most drawn: thin streaks a
/// frame long would otherwise cover the view many times over at a few metres.
const DRAWN: f32 = 0.08;
const MAX_DROPS: u32 = 49_152;
/// The streak's length in time: the path a drop covers while a camera's shutter is open.
const EXPOSURE_S: f32 = 1.0 / 30.0;
/// How opaque a drop is where it covers a whole pixel.
const OPACITY: f32 = 0.7;
/// The wind near the ground carries the drops at this share of the wind 10 m up.
const WIND_SHARE: f32 = 0.6;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    origin: [f32; 4],
    cells: [u32; 4],
    velocity: [f32; 4],
    look: [f32; 4],
    /// x: how far through the box the drops have fallen, 0 to 1, from the drawn time in f64.
    clock: [f32; 4],
}

/// A drop's terminal speed by its diameter in mm, m/s: D. Atlas, R. C. Srivastava and R. S.
/// Sekhon's fit to measurements, 9.65 - 10.3 exp(-0.6 D) ("Doppler radar characteristics of
/// precipitation at vertical incidence", Rev. Geophys. 11(1), 1973).
pub fn terminal_speed(drop_mm: f32) -> f32 {
    (9.65 - 10.3 * (-0.6 * drop_mm).exp()).max(0.5)
}

pub struct Rain {
    module: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    /// Group 0 (the view, the params) and group 3 (a water region's field, water_field.wgsl).
    frame_layout: wgpu::BindGroupLayout,
    field_layout: wgpu::BindGroupLayout,
    /// Groups 1 and 2, which the rain does not use.
    empty: wgpu::BindGroup,
    /// The field bound with no region under the eye: a zeroed uniform, whose zero columns hold no
    /// water.
    no_field: (wgpu::Buffer, wgpu::Buffer),
    draw: wgpu::RenderPipeline,
    params: wgpu::Buffer,
    /// The drops drawn this frame; none while it does not rain.
    drops: u32,
    eye: [f32; 3],
}

/// The streaks' pipeline where the particles draw (premultiplied blending, depth tested, not
/// written; the pass's other targets left as they are).
fn draw_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: SceneFormat,
) -> wgpu::RenderPipeline {
    let premultiplied = wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent::OVER,
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("rain"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &format.targets(Some(premultiplied), false),
        }),
        primitive: Default::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Greater),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: format.multisample(),
        multiview_mask: None,
        cache: None,
    })
}

impl Rain {
    pub fn new(device: &wgpu::Device, format: SceneFormat) -> Rain {
        let module = shaders::module(device, "rain");
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty,
            count: None,
        };
        let uniform = wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rain"),
            entries: &[entry(0, uniform), entry(1, uniform)],
        });
        let field_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rain field"),
            entries: &[
                entry(1, uniform),
                entry(
                    2,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
            ],
        });
        let empty_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rain unused"),
            entries: &[],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rain"),
            bind_group_layouts: &[
                Some(&frame_layout),
                Some(&empty_layout),
                Some(&empty_layout),
                Some(&field_layout),
            ],
            immediate_size: 0,
        });
        let empty = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rain unused"),
            layout: &empty_layout,
            entries: &[],
        });
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let no_field = (
            buffer(
                "rain no field",
                crate::water::UNIFORM_BYTES,
                wgpu::BufferUsages::UNIFORM,
            ),
            buffer("rain no columns", 16, wgpu::BufferUsages::STORAGE),
        );
        let draw = draw_pipeline(device, &module, &layout, format);
        let params = buffer(
            "rain params",
            std::mem::size_of::<Params>() as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        Rain {
            module,
            layout,
            frame_layout,
            field_layout,
            empty,
            no_field,
            draw,
            params,
            drops: 0,
            eye: [0.0; 3],
        }
    }

    /// Rebuilds the drawing pipeline for a pass of `format`.
    pub fn set_format(&mut self, device: &wgpu::Device, format: SceneFormat) {
        self.draw = draw_pipeline(device, &self.module, &self.layout, format);
    }

    /// The eye of this frame's rain.
    pub fn eye(&self) -> [f32; 3] {
        self.eye
    }

    /// This frame's rain around the eye at `eye` at drawn simulated time `t`: `rain` falling,
    /// slanted by `wind` (the wind sea the weather brings, whose speed is the wind 10 m up).
    pub fn update(
        &mut self,
        queue: &wgpu::Queue,
        rain: Option<RainView>,
        wind: Option<WindSeaView>,
        eye: [f32; 3],
        t: f64,
    ) {
        self.eye = eye;
        let Some(r) = rain.filter(|r| r.intensity_mm_h > 0.0 && r.drop_mm > 0.0) else {
            self.drops = 0;
            return;
        };
        let speed = terminal_speed(r.drop_mm);
        let diameter = r.drop_mm * 1e-3;
        let drop_m3 = std::f32::consts::PI / 6.0 * diameter * diameter * diameter;
        // Drops in a cubic metre of air: the rain's flux over their speed and volume.
        let per_m3 = r.intensity_mm_h / 3.6e6 / speed / drop_m3;
        let box_m3 = (CELL_M * SIDE as f32).powi(2) * HEIGHT_M;
        let drawn = (per_m3 * box_m3 * DRAWN).min(MAX_DROPS as f32);
        let per_cell = ((drawn / (SIDE * SIDE) as f32).ceil() as u32).max(1);
        let half = CELL_M * SIDE as f32 * 0.5;
        let corner = [
            ((eye[0] - half) / CELL_M).floor() * CELL_M,
            ((eye[2] - half) / CELL_M).floor() * CELL_M,
        ];
        let (x, z) = wind.map_or((0.0, 0.0), |w| {
            let toward = w.toward_deg.to_radians();
            (
                toward.sin() * w.speed * WIND_SHARE,
                toward.cos() * w.speed * WIND_SHARE,
            )
        });
        queue.write_buffer(
            &self.params,
            0,
            bytemuck::bytes_of(&Params {
                origin: [corner[0], eye[1] - BELOW_M, corner[1], HEIGHT_M],
                cells: [SIDE, SIDE, per_cell, 0],
                velocity: [x, -speed, z, EXPOSURE_S],
                look: [CELL_M, diameter, half, OPACITY],
                clock: [
                    (t * f64::from(speed) / f64::from(HEIGHT_M)).rem_euclid(1.0) as f32,
                    0.0,
                    0.0,
                    0.0,
                ],
            }),
        );
        self.drops = SIDE * SIDE * per_cell;
    }

    /// Draws the drops, those under the surface of `field` (the uniform and columns of the water
    /// region under the eye, if any) left out.
    pub fn draw(
        &self,
        device: &wgpu::Device,
        pass: &mut wgpu::RenderPass<'_>,
        view: &wgpu::Buffer,
        field: Option<(&wgpu::Buffer, &wgpu::Buffer)>,
    ) {
        if self.drops == 0 {
            return;
        }
        let (uniform, columns) = field.unwrap_or((&self.no_field.0, &self.no_field.1));
        let field = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rain field"),
            layout: &self.field_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: columns.as_entire_binding(),
                },
            ],
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rain"),
            layout: &self.frame_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: view.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.params.as_entire_binding(),
                },
            ],
        });
        pass.set_pipeline(&self.draw);
        pass.set_bind_group(0, &group, &[]);
        pass.set_bind_group(1, &self.empty, &[]);
        pass.set_bind_group(2, &self.empty, &[]);
        pass.set_bind_group(3, &field, &[]);
        pass.draw(0..6, 0..self.drops);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_fall_at_their_measured_speed() {
        // Atlas, Srivastava and Sekhon's fit: a 1 mm drop falls at 4.0 m/s, a 2.5 mm one at 7.35,
        // a 6 mm one at 9.37.
        assert!((terminal_speed(1.0) - 4.0).abs() < 0.1);
        assert!((terminal_speed(2.5) - 7.35).abs() < 0.05);
        assert!((terminal_speed(6.0) - 9.37).abs() < 0.05);
    }
}
