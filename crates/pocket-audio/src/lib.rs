//! Audio presentation (charter 5.2: a presenter, never in the tick). The world's `AudioSource`s play
//! as long as they exist, heard from their entities' positions by a listener at the camera; sound
//! events (`sound` with `{clip, volume?, pitch?}`, or any event named in the project's `[sounds]`
//! table) play once at their subject. Clips are project files (wav, ogg, mp3, flac) or synthesized
//! `sfx:` presets. Without an audio device everything is silently skipped.

pub mod synth;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use glam::{Quat, Vec3};
use kira::listener::ListenerHandle;
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle, StaticSoundSettings};
use kira::track::{SpatialTrackBuilder, SpatialTrackDistances, SpatialTrackHandle};
use kira::{AudioManager, AudioManagerSettings, Decibels, DefaultBackend, Frame, Tween};
use pocket_assets::frame::AudioView;

struct Playing {
    clip: String,
    looped: bool,
    sound: StaticSoundHandle,
    track: Option<SpatialTrackHandle>,
}

pub struct Audio {
    manager: AudioManager,
    listener: ListenerHandle,
    root: Option<PathBuf>,
    clips: HashMap<String, Option<StaticSoundData>>,
    sources: HashMap<u64, Playing>,
    one_shots: Vec<(StaticSoundHandle, Option<SpatialTrackHandle>)>,
}

fn gain_db(volume: f32) -> Decibels {
    if volume <= 0.0 {
        Decibels::SILENCE
    } else {
        Decibels(20.0 * volume.clamp(0.0, 4.0).log10())
    }
}

impl Audio {
    /// Opens the default output device; `None` without one.
    pub fn new(root: Option<PathBuf>) -> Option<Audio> {
        let mut manager = match AudioManager::<DefaultBackend>::new(AudioManagerSettings::default()) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("no audio output: {e}");
                return None;
            }
        };
        let listener = manager.add_listener(Vec3::ZERO, Quat::IDENTITY).ok()?;
        Some(Audio {
            manager,
            listener,
            root,
            clips: HashMap::new(),
            sources: HashMap::new(),
            one_shots: Vec::new(),
        })
    }

    fn clip(&mut self, name: &str) -> Option<StaticSoundData> {
        if let Some(c) = self.clips.get(name) {
            return c.clone();
        }
        let data = if let Some(spec) = name.strip_prefix("sfx:") {
            synth::synth(spec).map(|s| {
                let frames: Arc<[Frame]> = s.iter().map(|&x| Frame::from_mono(x)).collect();
                StaticSoundData {
                    sample_rate: synth::RATE,
                    frames,
                    settings: StaticSoundSettings::default(),
                    slice: None,
                }
            })
        } else {
            let path = self.root.as_ref().map_or_else(|| PathBuf::from(name), |r| r.join(name));
            match StaticSoundData::from_file(&path) {
                Ok(d) => Some(d),
                Err(e) => {
                    log::warn!("sound {name}: {e}");
                    None
                }
            }
        };
        self.clips.insert(name.to_owned(), data.clone());
        data
    }

    /// The listener follows the camera.
    pub fn set_listener(&mut self, position: Vec3, rotation: Quat) {
        self.listener.set_position(position, Tween::default());
        self.listener.set_orientation(rotation, Tween::default());
    }

    fn start(
        &mut self,
        clip: &str,
        volume: f32,
        pitch: f32,
        looped: bool,
        at: Option<Vec3>,
    ) -> Option<(StaticSoundHandle, Option<SpatialTrackHandle>)> {
        let mut data = self.clip(clip)?.volume(gain_db(volume)).playback_rate(f64::from(pitch.max(0.01)));
        if looped {
            data = data.loop_region(..);
        }
        match at {
            Some(p) => {
                let mut track = self
                    .manager
                    .add_spatial_sub_track(
                        self.listener.id(),
                        p,
                        SpatialTrackBuilder::new().distances(SpatialTrackDistances {
                            min_distance: 2.0,
                            max_distance: 120.0,
                        }),
                    )
                    .ok()?;
                let sound = track.play(data).ok()?;
                Some((sound, Some(track)))
            }
            None => Some((self.manager.play(data).ok()?, None)),
        }
    }

    /// Plays a clip once, at a point or everywhere.
    pub fn play_once(&mut self, clip: &str, volume: f32, pitch: f32, at: Option<Vec3>) {
        if let Some(s) = self.start(clip, volume, pitch, false, at) {
            self.one_shots.push(s);
        }
        // Forget finished one-shots (their tracks go with them).
        self.one_shots
            .retain(|(s, _)| s.state() != kira::sound::PlaybackState::Stopped);
    }

    /// Brings the playing sources in line with the world's.
    pub fn sync(&mut self, sources: &[AudioView]) {
        let live: HashMap<u64, &AudioView> = sources.iter().filter(|s| s.playing).map(|s| (s.id, s)).collect();
        let gone: Vec<u64> = self
            .sources
            .iter()
            .filter(|(id, p)| live.get(id).is_none_or(|s| s.clip != p.clip || s.looped != p.looped))
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            if let Some(mut p) = self.sources.remove(&id) {
                p.sound.stop(Tween::default());
            }
        }
        for (id, s) in live {
            if let Some(p) = self.sources.get_mut(&id) {
                p.sound.set_volume(gain_db(s.volume), Tween::default());
                p.sound.set_playback_rate(f64::from(s.pitch.max(0.01)), Tween::default());
                if let Some(t) = p.track.as_mut() {
                    t.set_position(Vec3::from(s.position), Tween::default());
                }
                continue;
            }
            let at = s.spatial.then(|| Vec3::from(s.position));
            if let Some((sound, track)) = self.start(&s.clip, s.volume, s.pitch, s.looped, at) {
                self.sources.insert(
                    id,
                    Playing {
                        clip: s.clip.clone(),
                        looped: s.looped,
                        sound,
                        track,
                    },
                );
            }
        }
    }
}
