//! Audio presentation (charter 5.2: a presenter, never in the tick). The world's `AudioSource`s play
//! as long as they exist, heard from their entities' positions by a listener at the camera; sound
//! events (`sound` with `{clip, volume?, pitch?}`, or any event named in the project's `[sounds]`
//! table) play once at their subject. Clips are project files (wav, ogg, mp3, flac) or synthesized
//! `sfx:` presets. The water's own sound comes from the bubbles its breaking, falling and
//! splashing traps (`water`). Without an audio device everything is silently skipped.
//!
//! The playback (`Audio`, on kira) is the `playback` feature, which the native window takes. Without
//! it the crate is the synthesis alone, `synth` and `water`'s `WaterSynth`, which the browser
//! viewport builds for the page's WebAudio (architecture.md 4.19).

pub mod synth;
pub mod water;

#[cfg(feature = "playback")]
mod playback;
#[cfg(feature = "playback")]
pub use playback::Audio;
