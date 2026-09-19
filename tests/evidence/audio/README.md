# Audio evidence (2026-09-19)

`pocket run audio -- --headless --frames 300 --json --size 64x64` (no sound card involved):

```
state   {"beeps": 5, "voices": 1, "hum.position": 0}
audio   {"device": "none", "sample_rate": 48000, "voices": 1, "clips": 3, "plays": 9, "master_volume": 1.0, "muted": false, "frames_rendered": 0}
events  {"audio.finished": 8, "audio.looped": 5, "audio.started": 9, "entity.spawned": 5}
log     [info] audio: loaded assets/hum.wav (1.000 s, 22050 Hz, 1 ch)
        [info] audio: loaded assets/beep.wav (0.300 s, 22050 Hz, 1 ch)
        [info] audio: loaded assets/click.wav (0.030 s, 22050 Hz, 1 ch)
```

Five seconds of simulation: the scene's `AudioSource` started the 1 s hum on tick 0 and it looped five times (`hum.position` is back at 0 on tick 300); the script played a beep every second (five started, five finished) and three clicks when the crate bounced; one voice (the hum) is still playing. The same run in a window reports the playback device by name and `frames_rendered` growing, with identical `state` and event counts.

Reproduce: `pocket test --filter audio`; the WAV clips come from `python3 tools/scripts/make_sample_assets.py --sounds`.
