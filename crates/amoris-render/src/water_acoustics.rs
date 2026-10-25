//! What the water sounds like from the camera (water.md 59). The spray's emitters already find
//! where the water makes noise: a fall leaving its lip, a rapid, a surface rising fast, a breaking
//! crest's lip, a body entering, a drop landing (water_spray_compute.wgsl). When asked to listen
//! they also tally, per kind, the frame's events and the mechanical power they dissipate over the
//! square of their distance, split between the ears; a small buffer carries the tally back a frame
//! or two later. The audio presenter turns the events into bubbles and the power into their
//! loudness (amoris-audio's `water`). Presentation only.
use std::sync::{Arc, Mutex};

/// The kinds of source, in the tally's order.
pub const KINDS: usize = 6;
pub const NAMES: [&str; KINDS] = ["falls", "rapids", "bursts", "breaking", "entries", "drops"];
/// The tally: three words a kind (events, then joules over r^2 at the left and the right), in
/// 1/256ths, padded.
pub const TALLY_BYTES: u64 = 128;
const SCALE: f32 = 256.0;

/// One kind of source over the frame: its events a second, and the power they dissipate (W) over
/// the square of their distance from 1 m (so W/m^2), split by their direction across the camera.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize)]
pub struct AcousticSource {
    pub rate: f32,
    pub left: f32,
    pub right: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize)]
pub struct WaterAcoustics {
    /// The simulated seconds the tally covers (0 while the water stands still).
    pub dt: f32,
    pub kinds: [AcousticSource; KINDS],
}

impl WaterAcoustics {
    fn decode(words: &[u32], dt: f32) -> WaterAcoustics {
        let per_second = if dt > 0.0 { 1.0 / (dt * SCALE) } else { 0.0 };
        WaterAcoustics {
            dt,
            kinds: std::array::from_fn(|k| AcousticSource {
                rate: words[3 * k] as f32 * per_second,
                left: words[3 * k + 1] as f32 * per_second,
                right: words[3 * k + 2] as f32 * per_second,
            }),
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Slot {
    #[default]
    Idle,
    /// Copied in this frame's encoder; mapped once it is submitted.
    Copied {
        dt: f32,
        frame: u64,
    },
    Mapping {
        dt: f32,
        frame: u64,
    },
    Ready {
        dt: f32,
        frame: u64,
    },
    Failed,
}

/// Two staging buffers taking turns, so a tally can be read while the next is copied.
pub(crate) struct Acoustics {
    stages: [wgpu::Buffer; 2],
    slots: Arc<Mutex<[Slot; 2]>>,
    frame: u64,
}

impl Acoustics {
    pub fn new(device: &wgpu::Device) -> Acoustics {
        Acoustics {
            stages: std::array::from_fn(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("water acoustics readback"),
                    size: TALLY_BYTES,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                })
            }),
            slots: Arc::new(Mutex::new([Slot::Idle; 2])),
            frame: 0,
        }
    }

    /// Copies the frame's tally into an idle staging buffer, if one is idle; else the frame goes
    /// unheard.
    pub fn copy(&mut self, encoder: &mut wgpu::CommandEncoder, tally: &wgpu::Buffer, dt: f32) {
        self.frame += 1;
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = slots.iter().position(|s| *s == Slot::Idle) {
            encoder.copy_buffer_to_buffer(tally, 0, &self.stages[i], 0, TALLY_BYTES);
            slots[i] = Slot::Copied {
                dt,
                frame: self.frame,
            };
        }
    }

    /// After the frame's submit: maps what it copied.
    pub fn after_submit(&mut self) {
        for i in 0..2 {
            let (dt, frame) = {
                let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
                let Slot::Copied { dt, frame } = slots[i] else {
                    continue;
                };
                slots[i] = Slot::Mapping { dt, frame };
                (dt, frame)
            };
            // No lock held: a refused map may call back at once.
            let slots = self.slots.clone();
            self.stages[i]
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let mut s = slots.lock().unwrap_or_else(|e| e.into_inner());
                    s[i] = if result.is_ok() {
                        Slot::Ready { dt, frame }
                    } else {
                        Slot::Failed
                    };
                });
        }
    }

    /// The newest tally read back since the last call, if any.
    pub fn take(&mut self) -> Option<WaterAcoustics> {
        let mut newest: Option<(u64, WaterAcoustics)> = None;
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        for i in 0..2 {
            match slots[i] {
                Slot::Ready { dt, frame } => {
                    // Mapped memory in the browser carries no alignment guarantee: decode.
                    let words: Option<Vec<u32>> = self.stages[i]
                        .slice(..)
                        .get_mapped_range()
                        .ok()
                        .map(|data| {
                            data.as_chunks::<4>()
                                .0
                                .iter()
                                .map(|b| u32::from_le_bytes(*b))
                                .collect()
                        });
                    self.stages[i].unmap();
                    slots[i] = Slot::Idle;
                    if let Some(words) = words
                        && newest.is_none_or(|(f, _)| frame > f)
                    {
                        newest = Some((frame, WaterAcoustics::decode(&words, dt)));
                    }
                }
                Slot::Failed => slots[i] = Slot::Idle,
                _ => {}
            }
        }
        newest.map(|(_, a)| a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tally_decodes_to_rates_and_powers() {
        let mut words = [0u32; 32];
        words[9] = 256 * 30; // breaking: 30 events in the frame
        words[10] = 128;
        words[11] = 384;
        let a = WaterAcoustics::decode(&words, 0.5);
        assert_eq!(a.kinds[3].rate, 60.0);
        assert_eq!(a.kinds[3].left, 1.0);
        assert_eq!(a.kinds[3].right, 3.0);
        assert_eq!(a.kinds[0], AcousticSource::default());
        assert_eq!(WaterAcoustics::decode(&words, 0.0).kinds[3].rate, 0.0);
    }
}
