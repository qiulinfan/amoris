# Audio

Sound in Pocket follows the same rule as everything else: the state an agent can read is the state the game runs on, and it exists without hardware.

## Model

- **Clips** are project-relative WAV files (`assets/beep.wav`), decoded through SDL and converted once to the mixer format (float, stereo, 48 kHz).
- **Voices** are playing clips with volume, pitch (rate), pan, loop, an optional owning entity and a tag. A voice's logical position advances by the simulation tick (`dt * rate * pitch`), so a headless run, a test and a window agree on which voices are playing, where they are, when one loops and when one finishes. Finished voices disappear; loops report each wrap.
- **Events**: `audio.started` (subject: the entity if any; clip, voice, loop), `audio.looped`, `audio.finished` go through the causal event log like collisions, so the transcript groups them and `events.since` streams them.
- **Playback** is a software mixer rendering the same voices into an SDL3 audio stream bound to the default device, about 80 ms ahead, with linear resampling for pitch and constant-power-free linear panning. Headless sessions never open a device; `audio.stats.device` says `none`.

## Component

`AudioSource { clip, volume, pitch, loop, autoplay, playing, voice }`: put it in a scene with `autoplay` and the engine starts the clip the first tick it sees the component, then keeps `playing` and `voice` current (a finished non-looping clip clears them). One-shots come from scripts.

## Commands and SDK

| Command | SDK | Purpose |
|---|---|---|
| `audio.play {clip, volume?, pitch?, pan?, loop?, entity?, tag?}` | `audio.play(clip, options)` | Start a voice; returns its id. |
| `audio.stop {voice | clip | tag | all}` | `audio.stop(id | {clip} | {tag})` | Stop voices; returns how many. |
| `audio.set {voice, volume?, pitch?, pan?, loop?}` | `audio.set(id, params)` | Change a playing voice. |
| `audio.list` | `audio.voices()` | Every voice with position and duration in seconds. |
| `audio.clips`, `audio.stats`, `audio.master {volume?, muted?}` | `audio.clips()`, `audio.stats()`, `audio.setMasterVolume()`, `audio.mute()` | Loaded clips, device and counters, master gain. |

`samples/audio` plays a looping hum from the scene, a beep every second at rising pitches and a click when a crate lands; `pocket run audio -- --headless --frames 300 --json` reports `beeps`, `voices` and the hum's position, and the event histogram counts `audio.started` / `audio.finished` without a sound card. WAV clips come from `tools/scripts/make_sample_assets.py --sounds`.

## Not yet

Compressed formats (OGG/MP3), 3D attenuation from entity positions (pan and volume are explicit today), effects buses, streaming long clips, and audio in the state hash (voices are deterministic but kept out of the hash like the interface).
