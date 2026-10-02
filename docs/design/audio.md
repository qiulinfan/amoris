# Audio

Sound in Pocket follows the same rule as everything else: the state an agent can read is the state
the game runs on, and it exists without hardware.

## Model

- **Clips** are project-relative WAV, Ogg Vorbis, MP3 or FLAC files (`assets/beep.wav`,
  `assets/chime.ogg`, `assets/blip.mp3`, `assets/tone.flac`), decoded once (WAV through SDL, Vorbis
  through stb_vorbis, MP3 through dr_mp3, FLAC through dr_flac) and converted to the mixer format
  (float, stereo, 48 kHz). An Ogg, MP3 or FLAC clip at least ten seconds long
  (`[audio] stream_seconds` in `project.toml`; 0 streams every one) is not decoded whole: it
  streams, its compressed bytes staying in memory and each voice decoding its own way through them a
  chunk at a time, a few thousand converted frames ahead of the mixer, so a music track costs its
  file size rather than its decoded size (a three-minute track: a few megabytes rather than thirty).
  A streamed voice loops without a seam and plays at any pitch like a decoded one; `audio.clips`
  reports `streamed` and each clip's `bytes`, `audio.stats` the streamed clips and streaming voices.
  WAV clips decode whole.
- **Voices** are playing clips with volume, pitch (rate), pan, loop, a low-pass (`lowpass`, 1 for
  the clip as it is, small values muffling it through a one-pole filter per voice: underwater,
  behind a door), a send to the room's reverb (`reverb`, The room below), an optional owning entity,
  a tag and a bus (Buses below). A voice's logical position advances by the simulation tick
  (`dt * rate * pitch`), so a headless run, a test and a window agree on which voices are playing,
  where they are, when one loops and when one finishes. Finished voices disappear; loops report each
  wrap.
- **Events**: `audio.started` (subject: the entity if any; clip, voice, loop), `audio.looped`,
  `audio.finished` go through the causal event log like collisions, so the transcript groups them
  and `events.since` streams them.
- **Playback** is a software mixer rendering the same voices into an SDL3 audio stream bound to the
  default device, about 80 ms ahead, with linear resampling for pitch and constant-power-free linear
  panning. Headless sessions never open a device; `audio.stats.device` says `none`.

## Component

`AudioSource { clip, volume, pitch, loop, autoplay, bus, spatial, near, range, playing, voice }`:
put it in a scene with `autoplay` and the engine starts the clip the first tick it sees the
component, then keeps `playing` and `voice` current (a finished non-looping clip clears them);
`spatial` makes it heard from where the entity is (below). One-shots come from scripts.

## Where a sound is

A source with `spatial` is heard from where its entity is: every tick the engine takes the entity's
world position against the listener, an entity with an enabled `AudioListener` (its world position
and facing; the first by id when there are several, so a third-person game puts one on the player
and sounds are placed around the player, not the camera) or else the camera of the last frame, and
sets the voice's volume to the source's, full within `near` of the camera and falling in a straight
line to nothing at `range`, and its pan toward the side the entity is on (most of the way, so a
sound straight to the right still reaches the left ear). A one-shot follows its entity the same way:
`audio.play {clip, entity, spatial: true, near?, range?}` places the voice now and every tick until
it ends. The placement goes through `audio.set`, so `audio.list` shows the volume and pan a spatial
voice has right now, and a headless run places its voices the same way a window does (the mixer
never runs, the numbers do). A source without `spatial` keeps the volume and pan it was given.

A wall counts when the source's `occlusion` is above 0 (`AudioSource.occlusion`, or
`audio.play {occlusion}` for a one-shot): each tick one ray runs from the listener to the entity,
and when a collider of another entity that is not a trigger crosses it (the listener's own collider
and the source's are passed over, so a player carrying the `AudioListener` does not block their own
ears), the volume is scaled by `1 - occlusion` and the voice's `lowpass` is set to the source's
times the same, so a hum behind a door at `occlusion: 0.7` is at three tenths of its volume and
muffled; with nothing in the way the source's own `lowpass` is restored, so a source with occlusion
owns its voice's low-pass as a spatial one owns its volume and pan. `AudioSource.occluded` says
whether it is blocked right now and `audio.occluded {clip, voice, blocked, by}` is emitted when that
changes, so a script can swap a room's ambience when the door shuts. The test is one straight line:
a doorway a step to the side still muffles, and nothing bends around a corner.

A moving source also changes pitch (the Doppler effect): each tick the engine takes how far the
source's entity and the listener moved since the last tick, their speeds along the line between
them, and sets the voice's pitch to its own times
`(343 + listener's speed toward the source) / (343 + source's speed away)`, sound at 343 m/s, held
between half and twice; `AudioSource.doppler` (or `audio.play {doppler}`) scales both speeds, 1 as
in air, 0 leaving the pitch alone. A car passing at thirty units a second is about 9.6 percent high
coming and 8 percent low going (`runtime_tests` `[doppler]`).

## The room

`audio.reverb {room, damping, mix}` (the SDK's `audio.reverb`, or a `[audio.reverb]` table in
`project.toml`) is the room every voice plays in: a reverb tail, four comb filters in parallel into
two all-pass filters in series (Freeverb's structure and lengths, scaled to the mixer's rate, the
right channel's lines a little longer so the tail has width), fed by each voice's `reverb` send (1
by default; `audio.play {reverb: 0}`, `audio.set {reverb}` or `AudioSource.reverb` keep a voice dry,
which is what interface clicks and music want). `room` is how long the tail rings (0 is no reverb
and costs nothing, 0.5 a room, 0.9 a hall), `damping` how fast its high end dies (a carpeted room
damps more than a tiled one), `mix` the tail's level against the dry voices. The tail keeps ringing
after the voices stop until it dies away, and `audio.stats` reports the room and whether it is
`ringing`; a headless run renders it the same way, so a test can hear a cave: `audio_tests`
(`[reverb]`) has a 0.3-second beep in a room of 0.8 still at about a fifth of its peak a tenth of a
second after it ends and fading, a voice with its send at 0 leaving the room silent, and room 0
dropping the tail at once. One room per runtime: a door between two rooms is a script changing the
room as the player crosses it.

## Buses

Every voice plays on a bus, `main` unless `audio.play {bus}`, `AudioSource.bus` or `audio.set {bus}`
names another (any name: `music`, `fx`, `dialogue`), and a bus is set as one:
`audio.bus {name, volume, muted, lowpass}` (the SDK's `audio.bus(name, settings)`) is a settings
menu's music slider, a mute for effects, or a pause menu that muffles the game under it with one
call, whatever voices come and go. The mixer sums a bus's voices first, then gives the sum the bus's
gain and low-pass (a one-pole filter over the whole group, like a voice's) and the master; a change
of gain is ramped across the next slice so a slider never clicks, and a bus's reverb send is scaled
with its gain but stays open under its low-pass, so a muffled bus still fills the room.

A bus has effects over its whole mix, after its low-pass: a **high-pass** (`highpass`, 0 for the
sound as it is, larger values thinning it out: a radio, a phone, a voice through a wall of the other
kind; the same one-pole filter taken away from the sound), an **echo** (`echo` seconds between
repeats, up to two, 0 for none; `echo_feedback` how much of each repeat comes back again, up to 0.9;
`echo_mix` how loud the repeats are: a canyon, a stairwell, a dream), and its share of the **room**
(`reverb`, 0..2, scaling its voices' sends to the reverb below: 0 keeps a bus's sounds out of the
room, the interface's clicks, say). An echo rings on after the bus's voices have ended, until what
went into its delay line has all come out; setting `echo` to 0 stops it at once. `audio_tests`
(`[busfx]`) thin a hum's body to under three tenths with a high-pass of 0.3 while its peaks stay,
repeat a click a quarter second apart at its own height, half and a quarter (feedback 0.5, full mix)
after the click ended, silence it with `echo` 0, and keep the room silent after a beep on a bus with
`reverb` 0.

A bus can duck under another: with `duck_by: "dialogue"`, `duck_amount: 0.3` and
`duck_seconds: 0.25`, the music falls to three tenths over a quarter of a second while any voice
plays on the dialogue bus and comes back the same way after the last one ends. The ducking moves on
the tick clock, so a headless run ducks at the same ticks as a window;
`audio.ducked {bus, ducked, by}` is emitted each way, and `audio.buses` lists every bus with its
settings, its ducking gain right now (`duck`), whether it is `ducked` and its voices.
`[audio.buses]` in `project.toml` sets a project's
(`music = { volume = 0.8, duck_by = "dialogue" }`), the editor's Audio tab sets them live and saves
them to `audio.json` beside it (applied over the TOML's, `docs/editor.md`), and `audio.stop {bus}`
stops a bus's voices. The audio sample puts its hum on an `ambience` bus that ducks to half under
the `fx` bus its beeps and clicks play on; `audio_tests` (`[buses]`) holds a bus at half volume to
half the peak, a muted one silent while its voice plays on, a low-passed one smoother, and music
under a line of dialogue half way down a quarter second in, at a quarter after half a second and
back in full half a second after the line stops.

## Sounds from a recipe: sound effects without files

A `.sfx` file is a sound written as JSON, which the engine renders to samples (at the mixer's rate)
the first time it is played or loaded, and which `audio.play`, `AudioSource.clip` and everything
else then treat as a clip. A model that cannot record a sound can write one:

```json
{ "wave": "square", "frequency": 260, "to": 640, "attack": 0.004, "hold": 0.06, "decay": 0.16, "volume": 0.4 }
```

A voice is a `wave` (`sine`, `square` with a `duty`, `triangle`, `saw`, or `noise`, held for one
period of the pitch so a high pitch hisses and a low one rumbles) whose pitch slides exponentially
from `frequency` to `to` over the sound and changes by `steps` (`[{at: 0.06, times: 1.335}]`: from
0.06 s on, a fourth higher; arpeggios and the two notes of a coin), with `vibrato`
(`{depth, rate}`), under an envelope that rises over `attack`, stays for `hold` and falls away over
`decay` (seconds), at `volume`, through a one-pole `lowpass` (Hz), starting after `delay`. `layers`
mixes several voices (an explosion's noise, its low thud and a crack), each taking the recipe's
other fields as defaults. `preset` starts from a common game sound, `coin`, `jump`, `hit`,
`explosion`, `laser`, `powerup`, `blip`, `hurt`, `select`, `shot` (a gun's crack and thump), `step`
(a footfall), `click`, or a bed to loop under a scene, `rain` and `wind` (four seconds of noise
without an attack or a decay, so `AudioSource.loop` plays them without a seam), with the recipe's
other fields over its first voice (`{"preset": "jump", "volume": 0.2}`); `seed` varies a preset's
pitches and times a little, so ten coins need not sound alike. The same recipe renders the same
samples everywhere (the noise comes from a generator of its own), and a sound that would pass full
scale is brought down to it. A recipe that cannot be read is refused with what is wrong in it, a
field a voice does not have included (`'reverb' is not a field of a voice`: reverb, echo and pan
belong to the bus, the room or `audio.play`). A preset needs no file at all: a clip named
`sfx:<preset>` is that recipe, with the recipe's fields after `?` (`"sfx:coin"`,
`"sfx:jump?volume=0.2&seed=3"`, wherever a clip goes: `audio.play`, `AudioSource.clip`).
`samples/audio` plays `assets/coin.sfx` (`{"preset": "coin"}`) on C and `assets/blast.sfx` (three
layers) on B. `audio_tests` (`[synth]`): half a second of a 440 Hz sine lasts half a second and
crosses zero 440 times, a preset renders the same samples in two sessions and other ones with a
seed, `sfx:coin` and `sfx:coin?seed=7` render as their files do, and a wave that does not exist is
named.

## Music from a score

A `.song` file is music written as JSON: instruments are recipes (any voice above, or a preset), and
tracks are notes on a grid of steps, `steps_per_beat` to a beat at `bpm` (2 and 120 by default:
eighth notes). A note is a name and an octave (`C4` is middle C, `F#5`, `Bb2`, `A4` is 440 Hz), `.`
is a rest, `-` holds the note before it through that step, and `x` plays the instrument at its own
pitch (a drum); `|` and spaces only separate. A track shorter than the song repeats (a drum bar
under a four-bar tune), the song is as long as its longest track (or `steps`), and with `loop` (on
by default) the tails of its last notes ring round into its start, so
`audio.play(song, { loop: true })` plays on without a seam. A note that is not one is refused with
its track.

```json
{ "bpm": 132, "instruments": { "lead": { "wave": "square", "duty": 0.25, "hold": 0.08, "decay": 0.12, "volume": 0.22 }, "kick": { "wave": "sine", "frequency": 120, "to": 40, "decay": 0.15 } },
  "tracks": [ { "instrument": "lead", "notes": "E5 - G5 - C6 - B5 A5 | G5 - E5 - C5 - D5 E5" }, { "instrument": "kick", "notes": "x . . . x . . ." } ] }
```

`samples/audio/assets/theme.song` (four bars: a square lead, a triangle bass, a kick and a hat)
plays on M. `audio_tests` (`[song]`): a second-long A4 crosses zero 880 times and the rests after it
are silent, the tail of a looping song's last note is heard at its start and not when it plays once,
and `H4` is refused.

## Listening without ears

A model that writes a recipe or a score cannot hear what it made; `audio.analyze {clip}`
(`audio.analyze(clip)` in scripts) tells it, for any clip: `seconds` and `audible_seconds` (to where
it falls under a hundredth of its loudest), `peak` and `loudness_db`, the envelope's `attack_ms` (to
nine tenths of its loudest) and `decay_ms` (from there to a tenth), `onsets` and `onsets_per_second`
(notes struck), `pitch` as segments over time `{from, to, hz, note}` with the note named
(`"E6 +11c"`: eleven cents sharp), `tonal` (the share of the sound that has a pitch),
`brightness_hz` (the spectral centroid) and `character` in words:
`"short, bright, tonal, rising, sharp attack"` for the coin preset, `"medium, warm, noisy"` for the
blast. The pitch of 30 ms frames every 10 ms comes from YIN's normalised difference on a 16 kHz
copy, taking the first lag nearly as deep as the deepest (the period, not a multiple of it) and
refining it between samples by a parabola; segments of fewer than three frames are left out (a frame
across two notes hears their common period). A score is measured as the mix: the strongest line at
each moment, a kick's slide included, and its melody's notes where the lead is loudest. The first
ten seconds are listened to; a clip long enough to stream reports only its length. `assets.reload`
makes sounds read again on their next play (a voice playing the old one plays it out), so an edited
recipe can be analysed and heard at once. `audio_tests` (`[analyze]`): a 440 Hz sine is A4 within 3
Hz and tonal, the coin is B5 then E6 and rising, noise is noisy, and an edited recipe is read again
after it is forgotten.

## Commands and SDK

| Command | SDK | Purpose |
|---|---|---|
| `audio.play {clip, volume?, pitch?, pan?, lowpass?, loop?, entity?, tag?, bus?, spatial?, near?, range?, occlusion?}` | `audio.play(clip, options)` | Start a voice; returns its id. A spatial one follows its entity; with `occlusion` a wall between muffles it. |
| `audio.stop {voice | clip | tag | bus | all}` | `audio.stop(id | {clip} | {tag} | {bus})` | Stop voices; returns how many. |
| `audio.set {voice, volume?, pitch?, pan?, lowpass?, reverb?, loop?, bus?}` | `audio.set(id, params)` | Change a playing voice (`lowpass` muffles it, for a door closing or a dive; `reverb` is its send to the room; `bus` moves it). |
| `audio.bus {name, volume?, muted?, lowpass?, highpass?, echo?, echo_feedback?, echo_mix?, reverb?, duck_by?, duck_amount?, duck_seconds?}`, `audio.buses` | `audio.bus(name, settings)`, `audio.buses()` | A bus's settings and every bus's state (Buses above). |
| `audio.reverb {room?, damping?, mix?}` | `audio.reverb(settings)` | The room: the reverb tail every voice's send feeds (The room above). |
| `audio.list` | `audio.voices()` | Every voice with position and duration in seconds. |
| `audio.clips`, `audio.stats`, `audio.master {volume?, muted?}` | `audio.clips()`, `audio.stats()`, `audio.setMasterVolume()`, `audio.mute()` | Loaded clips, device and counters, master gain. |

`samples/audio` plays a looping hum from the scene, a beep every second at rising pitches and a
click when a crate lands; `pocket run audio -- --headless --frames 300 --json` reports `beeps`,
`voices` and the hum's position, and the event histogram counts `audio.started` / `audio.finished`
without a sound card. WAV clips come from `tools/scripts/make_sample_assets.py --sounds`.

## Not yet

Compressed formats beyond Ogg Vorbis, MP3 and FLAC (Opus), more than one room at a time (a door
between two rooms is a script changing the room) and early reflections that follow the walls (the
reverb is one tail for the whole room; occlusion is a straight ray, so nothing bends around a corner
either), a reverb of a bus's own (every bus sends to the one room's), streaming WAV (a long WAV
decodes whole; encode music as Ogg), and audio in the state hash (voices are deterministic but kept
out of the hash like the interface).
