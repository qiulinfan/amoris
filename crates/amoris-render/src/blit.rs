//! GPU copies with filtering: uploading an image of any size into a texture-array layer (resized by
//! the sampler, or by point sampling when its glTF sampler magnifies with `NEAREST`) and building
//! its mip chain by repeated half-size draws.

use std::collections::HashMap;

use amoris_assets::mesh::ImageData;

use crate::shaders;

pub struct Blitter {
    module: wgpu::ShaderModule,
    sampler: wgpu::Sampler,
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
}

impl Blitter {
    pub fn new(device: &wgpu::Device) -> Blitter {
        Blitter {
            module: shaders::module(device, "post"),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("blit"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                ..Default::default()
            }),
            pipelines: HashMap::new(),
        }
    }

    fn pipeline(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> &wgpu::RenderPipeline {
        let module = &self.module;
        self.pipelines.entry(format).or_insert_with(|| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("blit"),
                layout: None,
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs_full"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs_copy"),
                    compilation_options: Default::default(),
                    targets: &[Some(format.into())],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        })
    }

    /// Draws `src` into `dst` (one mip of one layer) with linear filtering.
    pub fn blit(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        dst: &wgpu::TextureView,
        format: wgpu::TextureFormat,
    ) {
        let pipeline = self.pipeline(device, format).clone();
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(src),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("blit"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Uploads `image` into layer `layer` of `array` (resized to the array's size) and fills its
    /// mip chain. The resize is where an image's filter applies, since one linear sampler draws
    /// every material: a marked image ([`ImageData::nearest`]) is point-sampled to the layer's size
    /// first ([`point_sampled`]), any other is stretched by the linear sampler.
    #[allow(clippy::too_many_arguments)]
    pub fn upload_layer(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        image: &ImageData,
        array: &wgpu::Texture,
        format: wgpu::TextureFormat,
        layer: u32,
        mips: u32,
    ) {
        let resized = point_sampled(image, array.width(), array.height());
        let (width, height, pixels) = match &resized {
            Some(pixels) => (array.width(), array.height(), &pixels[..]),
            None => (image.width, image.height, &image.rgba8[..]),
        };
        let staging = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("image upload"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        if pixels.len() as u64 >= u64::from(width) * u64::from(height) * 4 {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &staging,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width * 4),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        }
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("texture layer"),
        });
        let view_of = |mip: u32| {
            array.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: layer,
                array_layer_count: Some(1),
                base_mip_level: mip,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        let src = staging.create_view(&wgpu::TextureViewDescriptor::default());
        self.blit(device, &mut enc, &src, &view_of(0), format);
        for mip in 1..mips {
            let s = view_of(mip - 1);
            let d = view_of(mip);
            self.blit(device, &mut enc, &s, &d, format);
        }
        queue.submit([enc.finish()]);
    }
}

/// A marked image ([`ImageData::nearest`]: its glTF sampler magnifies with `NEAREST`) resized to
/// `width` x `height` by point sampling: each texel of the result takes the image's texel under its
/// centre, `floor((2x + 1) * w / (2 * width))` across and the same down. The arithmetic is integer,
/// so every backend uploads the same bytes, and an image whose size divides the result's repeats
/// each texel a whole number of times; otherwise its texels come out one result texel apart in
/// width. `None` for an image the linear sampler resizes instead: one that is not marked, is wider
/// or taller than the result (a minification, which a magnification filter does not govern), has
/// the result's size already, or holds fewer pixels than its size says.
fn point_sampled(image: &ImageData, width: u32, height: u32) -> Option<Vec<u8>> {
    let (w, h) = (image.width as usize, image.height as usize);
    let (width, height) = (width as usize, height as usize);
    // The sizes are compared first: the products below then fit a 32-bit `usize` (the browser).
    if !image.nearest
        || w == 0
        || h == 0
        || w > width
        || h > height
        || (w == width && h == height)
        || image.rgba8.len() < w * h * 4
    {
        return None;
    }
    let columns: Vec<usize> = (0..width).map(|x| (2 * x + 1) * w / (2 * width)).collect();
    let mut out = Vec::with_capacity(width * height * 4);
    let mut above = usize::MAX;
    for y in 0..height {
        let row = (2 * y + 1) * h / (2 * height);
        if row == above {
            // The same row of the image as the row above: a copy of that row.
            out.extend_from_within(out.len() - width * 4..);
            continue;
        }
        above = row;
        let texels = &image.rgba8[row * w * 4..(row + 1) * w * 4];
        for &c in &columns {
            out.extend_from_slice(&texels[c * 4..c * 4 + 4]);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A marked image whose texel (x, y) holds x in its first two bytes and y in its last two.
    fn art(width: u32, height: u32) -> ImageData {
        let mut rgba8 = Vec::new();
        for y in 0..height {
            for x in 0..width {
                rgba8.extend_from_slice(&(x as u16).to_le_bytes());
                rgba8.extend_from_slice(&(y as u16).to_le_bytes());
            }
        }
        ImageData {
            name: "art".into(),
            width,
            height,
            rgba8,
            srgb: true,
            nearest: true,
        }
    }

    /// The image texel that texel (`x`, `y`) of `pixels`, `width` wide, holds.
    fn source(pixels: &[u8], width: u32, x: u32, y: u32) -> (u32, u32) {
        let p = &pixels[((y * width + x) * 4) as usize..][..4];
        (
            u32::from(u16::from_le_bytes([p[0], p[1]])),
            u32::from(u16::from_le_bytes([p[2], p[3]])),
        )
    }

    #[test]
    fn a_size_that_divides_the_layers_repeats_each_texel() {
        // blockcraft's art: 16 x 16 into a 1024 x 1024 layer, each texel 64 x 64 times.
        let pixels = point_sampled(&art(16, 16), 1024, 1024).expect("a marked image");
        assert_eq!(pixels.len(), 1024 * 1024 * 4);
        for y in 0..1024 {
            for x in 0..1024 {
                assert_eq!(source(&pixels, 1024, x, y), (x / 64, y / 64), "({x}, {y})");
            }
        }
        // Only the smaller side is resized.
        let pixels = point_sampled(&art(1024, 16), 1024, 1024).expect("a marked image");
        for y in (0..1024).step_by(7) {
            for x in 0..1024 {
                assert_eq!(source(&pixels, 1024, x, y), (x, y / 64), "({x}, {y})");
            }
        }
    }

    #[test]
    fn any_other_size_takes_the_texel_under_each_centre() {
        // 24 x 12 into 1024 x 1024: 42 2/3 layer texels a texel across, 85 1/3 down.
        let (w, h, size) = (24u32, 12u32, 1024u32);
        let pixels = point_sampled(&art(w, h), size, size).expect("a marked image");
        assert_eq!(pixels.len(), (size * size * 4) as usize);
        for y in 0..size {
            for x in 0..size {
                // The image's texel (sx, sy) covers [sx, sx + 1) * size / w of the layer, and the
                // layer texel's centre x + 1/2 lies there (the inequalities doubled and scaled).
                let (sx, sy) = source(&pixels, size, x, y);
                assert!(2 * sx * size <= (2 * x + 1) * w && (2 * x + 1) * w < 2 * (sx + 1) * size);
                assert!(2 * sy * size <= (2 * y + 1) * h && (2 * y + 1) * h < 2 * (sy + 1) * size);
            }
        }
        // Every texel is there, in order, in runs that differ by one layer texel.
        let runs = |along: &dyn Fn(u32) -> u32| {
            let mut runs: Vec<(u32, u32)> = Vec::new();
            for i in 0..size {
                match runs.last_mut() {
                    Some((texel, n)) if *texel == along(i) => *n += 1,
                    _ => runs.push((along(i), 1)),
                }
            }
            runs
        };
        let across = runs(&|x| source(&pixels, size, x, 500).0);
        assert_eq!(
            across.iter().map(|r| r.0).collect::<Vec<_>>(),
            (0..w).collect::<Vec<_>>()
        );
        assert!(across.iter().all(|r| r.1 == 42 || r.1 == 43), "{across:?}");
        let down = runs(&|y| source(&pixels, size, 500, y).1);
        assert_eq!(
            down.iter().map(|r| r.0).collect::<Vec<_>>(),
            (0..h).collect::<Vec<_>>()
        );
        assert!(down.iter().all(|r| r.1 == 85 || r.1 == 86), "{down:?}");
    }

    #[test]
    fn the_sampler_resizes_every_other_image() {
        let smooth = ImageData {
            nearest: false,
            ..art(16, 16)
        };
        assert_eq!(point_sampled(&smooth, 1024, 1024), None, "not marked");
        // A minification along either axis is the linear sampler's.
        assert_eq!(point_sampled(&art(2048, 16), 1024, 1024), None, "wider");
        assert_eq!(point_sampled(&art(16, 2048), 1024, 1024), None, "taller");
        assert_eq!(
            point_sampled(&art(1024, 1024), 1024, 1024),
            None,
            "the layer's size"
        );
        let short = ImageData {
            rgba8: vec![255; 16 * 16 * 4 - 1],
            ..art(16, 16)
        };
        assert_eq!(point_sampled(&short, 1024, 1024), None, "too few pixels");
        assert_eq!(point_sampled(&art(0, 16), 1024, 1024), None, "no pixels");
        assert_eq!(point_sampled(&art(16, 0), 1024, 1024), None, "no pixels");
    }
}
