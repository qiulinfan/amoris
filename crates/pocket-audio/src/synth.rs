//! Synthesized sound effects (`sfx:<preset>[:<variant>]`), in the spirit of sfxr: a game gets a
//! coin, a jump or an explosion without an audio file, which is what an agent making a game can
//! use at once. Each preset is a short program of an oscillator, an envelope and a pitch slide; the
//! variant number reseeds the noise and nudges the pitch, so `sfx:hit:2` differs from `sfx:hit:1`.

use std::f32::consts::TAU;

pub const RATE: u32 = 44_100;

/// The presets an `sfx:` clip may name.
pub const PRESETS: [&str; 11] = [
    "coin", "jump", "hit", "explosion", "laser", "powerup", "blip", "click", "wind", "surf", "chime",
];

#[derive(Clone, Copy)]
enum Wave {
    Square(f32),
    Saw,
    Sine,
    Noise,
}

struct Program {
    wave: Wave,
    freq: f32,
    /// Frequency multiplier per second (a slide; 1 = steady).
    slide: f32,
    /// A step up after `jump_at` seconds (the coin's second note).
    jump: f32,
    jump_at: f32,
    attack: f32,
    sustain: f32,
    decay: f32,
    /// Low-pass smoothing 0 (none) to 1.
    lowpass: f32,
    volume: f32,
    vibrato: (f32, f32),
}

fn program(preset: &str) -> Option<Program> {
    let p = |wave, freq, slide, attack, sustain, decay| Program {
        wave,
        freq,
        slide,
        jump: 1.0,
        jump_at: 1e9,
        attack,
        sustain,
        decay,
        lowpass: 0.0,
        volume: 0.5,
        vibrato: (0.0, 0.0),
    };
    Some(match preset {
        "coin" => Program { jump: 1.5, jump_at: 0.07, ..p(Wave::Square(0.5), 990.0, 1.0, 0.0, 0.07, 0.25) },
        "jump" => p(Wave::Square(0.4), 330.0, 3.2, 0.0, 0.08, 0.18),
        "hit" => Program { lowpass: 0.4, ..p(Wave::Noise, 180.0, 0.3, 0.0, 0.02, 0.16) },
        "explosion" => Program { lowpass: 0.85, volume: 0.8, ..p(Wave::Noise, 90.0, 0.4, 0.0, 0.12, 0.9) },
        "laser" => p(Wave::Saw, 1300.0, 0.08, 0.0, 0.05, 0.2),
        "powerup" => Program { vibrato: (12.0, 0.08), ..p(Wave::Square(0.3), 300.0, 5.0, 0.0, 0.25, 0.3) },
        "blip" => p(Wave::Square(0.5), 660.0, 1.0, 0.0, 0.03, 0.05),
        "click" => Program { lowpass: 0.2, volume: 0.35, ..p(Wave::Noise, 2000.0, 1.0, 0.0, 0.004, 0.02) },
        "wind" => Program { lowpass: 0.97, volume: 0.6, ..p(Wave::Noise, 60.0, 1.0, 0.6, 2.0, 1.2) },
        "surf" => Program { lowpass: 0.93, volume: 0.6, vibrato: (0.25, 0.6), ..p(Wave::Noise, 80.0, 1.0, 0.9, 1.6, 1.8) },
        "chime" => Program { jump: 1.5, jump_at: 0.12, ..p(Wave::Sine, 880.0, 1.0, 0.005, 0.1, 0.7) },
        _ => return None,
    })
}

/// A tiny deterministic noise source.
struct Lcg(u32);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    }
}

/// Mono samples at `RATE` for `sfx:<preset>[:<variant>]`, or `None` for an unknown preset.
pub fn synth(spec: &str) -> Option<Vec<f32>> {
    let mut parts = spec.split(':');
    let preset = parts.next()?;
    let variant: u32 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let pr = program(preset)?;
    let total = pr.attack + pr.sustain + pr.decay;
    let n = (total * RATE as f32) as usize;
    let mut out = Vec::with_capacity(n);
    let mut rng = Lcg(0x9e37_79b9 ^ variant.wrapping_mul(2_654_435_761));
    let detune = 1.0 + (variant % 7) as f32 * 0.03 - 0.09 * f32::from(u8::from(variant > 0));
    let mut phase = 0.0f32;
    let mut held = 0.0f32;
    let mut smooth = 0.0f32;
    for i in 0..n {
        let t = i as f32 / RATE as f32;
        let mut f = pr.freq * detune * pr.slide.powf(t);
        if t >= pr.jump_at {
            f *= pr.jump;
        }
        if pr.vibrato.1 > 0.0 {
            f *= 1.0 + pr.vibrato.1 * (TAU * pr.vibrato.0 * t).sin();
        }
        let prev = phase;
        phase = (phase + f / RATE as f32).fract();
        let s = match pr.wave {
            Wave::Square(duty) => {
                if phase < duty {
                    1.0
                } else {
                    -1.0
                }
            }
            Wave::Saw => phase * 2.0 - 1.0,
            Wave::Sine => (TAU * phase).sin(),
            Wave::Noise => {
                // A new random value each oscillator period: lower frequencies rumble.
                if phase < prev {
                    held = rng.next();
                }
                held
            }
        };
        smooth = smooth * pr.lowpass + s * (1.0 - pr.lowpass);
        let env = if t < pr.attack {
            t / pr.attack.max(1e-5)
        } else if t < pr.attack + pr.sustain {
            1.0
        } else {
            (1.0 - (t - pr.attack - pr.sustain) / pr.decay.max(1e-5)).max(0.0).powi(2)
        };
        out.push(smooth * env * pr.volume);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_synthesizes_audible_bounded_samples() {
        for p in PRESETS {
            let s = synth(p).unwrap();
            assert!(s.len() > 100, "{p}");
            let peak = s.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!(peak > 0.05 && peak <= 1.0, "{p}: {peak}");
        }
        assert!(synth("nothing").is_none());
        assert_ne!(synth("hit:1"), synth("hit:2"));
    }
}
