# Window recording (Windows)

`session.py` records chosen top-level windows continuously, one video track per window, for
recorded development sessions (the make-amoris-game workflow's three tracks: the Amoris editor,
Blender and the research browser). It runs on Windows only. Each track is an ffmpeg process with
the `gfxcapture` source (Windows Graphics Capture, FFmpeg 8 or newer; scoop's `ffmpeg` has it): it
reads the window's own surface from the compositor, so a track keeps recording while other windows
cover its window, and nothing is activated, moved or focused. A minimized window draws nothing and
records nothing; the status reports it.

## Commands

Run from the repository's root; every command prints what it did.

```sh
python tools/capture/session.py windows [--title REGEX]        # hwnd, pid, exe, rect, flags (M minimized, F foreground)
python tools/capture/session.py launch-browser --url URL --profile out/<task>/browser-profile [--cdp-port 9333]
python tools/capture/session.py launch --log FILE -- PROGRAM ARGS   # a GUI started without activating its window
python tools/capture/session.py bind --window amoris=HWND --window blender=HWND --window browser=HWND \
    --config out/<task>/selection.json --seconds 43200 --segment 300 --fps 30 --max-width 2560
python tools/capture/session.py start --config out/<task>/selection.json --out out/<task>/session-01
python tools/capture/session.py status --out out/<task>/session-01   # exit 0 healthy, 2 unhealthy
python tools/capture/session.py stop --out out/<task>/session-01
python tools/capture/session.py check --out out/<task>/session-01
python tools/capture/session.py stitch --out out/<task>/session-01 --role amoris --file out/<task>/amoris.mp4
python tools/capture/session.py recover --out out/<task>/session-01 --dest out/<task>/recovered-01
```

- `launch-browser` starts Chrome for Testing (`AMORIS_CHROME`, or the newest under
  `~/.cache/puppeteer`) with a dedicated profile and the switches that keep a covered window
  painting: `--disable-backgrounding-occluded-windows`, `--disable-background-timer-throttling`,
  `--disable-renderer-backgrounding` and `--disable-features=CalculateNativeWinOcclusion`. Without
  them Chrome stops drawing a window Windows reports occluded, and its track holds a still frame.
- `bind` saves each role's hwnd, pid, executable, class and title with the ceiling (`--seconds`,
  at most the planned session), the segment length, the frame rate and the width cap.
- `start` refuses a non-empty output directory, starts a detached supervisor (`pythonw`) and waits
  until every track has frames. The supervisor writes `status.json` every two seconds, restarts a
  track whose ffmpeg exits while its window exists (a new `run-NN` directory, recorded with its UTC
  start), stops every track with ffmpeg's `q` at the ceiling or on `stop`, and reports a gone or
  minimized window, a silent ffmpeg, less than 10 GB free disk, or itself silent for 10 s (`dead`).
- Output per role: `ROLE/run-NN/seg-NNNNN.mp4` (H.264, NVENC by default, `--encoder libx264` on a
  machine without it; fragmented MP4, so a killed recorder leaves readable segments),
  `segments.csv` (each segment's start and end in that run's time), `ffmpeg.log`, and
  `ROLE/preview.jpg`, refreshed every 10 s from the newest segment's last frames. A track holds
  about 0.9 GB of frame buffers at 2880 x 1800; a second output (a split for the preview) would
  double that.

## What the footage is

- Windows Graphics Capture delivers a frame when the window's content changes; the `fps` filter
  repeats the last frame to a constant rate, so a still window costs little, and the timeline stays
  true to wall time once the next change arrives. A still window's last stretch before `stop` is
  therefore shorter in the file than in wall time; `idle` in the status says how long it has been
  still. Align tracks by each run's `start_utc` in `status.json` plus the segment offsets.
- The cursor and the window border are not recorded. Video only: no system audio or microphone.
- Inspect `preview.jpg` and the segments' frames, not file growth, to know what was recorded. A
  live check: cover the window completely, change its content, and see the change in the preview.
- `tools/blender/gui_bridge.py` lets an agent run Python inside the recorded Blender GUI, so
  modeling happens in the window being recorded rather than in a separate background Blender.
