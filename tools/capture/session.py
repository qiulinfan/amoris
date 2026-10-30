#!/usr/bin/env python3
"""Continuous per-window recording on Windows: one video track per bound window.

Each track is an ffmpeg process reading the window through Windows Graphics Capture
(ffmpeg's `gfxcapture` source), so a window keeps recording while other windows cover it and
nothing is brought to the foreground. A detached supervisor runs the tracks, restarts one whose
ffmpeg exits while its window still exists, writes `status.json` every two seconds and stops at the
bound ceiling or when `stop` asks it to. See AGENTS.md beside this file.

    python tools/capture/session.py windows [--title REGEX]
    python tools/capture/session.py launch-browser --url URL --profile DIR [--size 1600,1000]
    python tools/capture/session.py bind --window amoris=HWND --window blender=HWND ... --config FILE
    python tools/capture/session.py start --config FILE --out DIR
    python tools/capture/session.py status --out DIR
    python tools/capture/session.py stop --out DIR
    python tools/capture/session.py check --out DIR
    python tools/capture/session.py stitch --out DIR --role ROLE --file FILE
    python tools/capture/session.py recover --out DIR --dest FRESH_DIR
"""
from __future__ import annotations

import argparse
import csv
import ctypes
import ctypes.wintypes as wt
import datetime as dt
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import threading
import time

ROOT = Path(__file__).resolve().parents[2]
STATUS_EVERY = 2.0
STALE_PROGRESS = 15.0  # ffmpeg reports progress twice a second even when the window is still
STALE_SUPERVISOR = 10.0
LOW_DISK_GB = 10.0
NO_WINDOW = 0x08000000  # CREATE_NO_WINDOW
DETACHED = 0x00000008  # DETACHED_PROCESS
NEW_GROUP = 0x00000200  # CREATE_NEW_PROCESS_GROUP
BREAKAWAY = 0x01000000  # CREATE_BREAKAWAY_FROM_JOB

user32 = ctypes.windll.user32 if sys.platform == 'win32' else None
kernel32 = ctypes.windll.kernel32 if sys.platform == 'win32' else None


def utc() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat(timespec='milliseconds')


# ---- Windows ---------------------------------------------------------------------------------

def _text(hwnd: int) -> str:
    n = user32.GetWindowTextLengthW(hwnd)
    buf = ctypes.create_unicode_buffer(n + 1)
    user32.GetWindowTextW(hwnd, buf, n + 1)
    return buf.value


def _class(hwnd: int) -> str:
    buf = ctypes.create_unicode_buffer(256)
    user32.GetClassNameW(hwnd, buf, 256)
    return buf.value


def _pid(hwnd: int) -> int:
    pid = wt.DWORD()
    user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
    return pid.value


def _exe(pid: int) -> str:
    handle = kernel32.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
    if not handle:
        return ''
    try:
        buf = ctypes.create_unicode_buffer(1024)
        size = wt.DWORD(1024)
        return buf.value if kernel32.QueryFullProcessImageNameW(handle, 0, buf, ctypes.byref(size)) else ''
    finally:
        kernel32.CloseHandle(handle)


def window_info(hwnd: int) -> dict | None:
    if not user32.IsWindow(hwnd):
        return None
    rect = wt.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(rect))
    pid = _pid(hwnd)
    return {'hwnd': hwnd, 'pid': pid, 'exe': Path(_exe(pid)).name, 'title': _text(hwnd),
            'class': _class(hwnd), 'visible': bool(user32.IsWindowVisible(hwnd)),
            'minimized': bool(user32.IsIconic(hwnd)),
            'foreground': user32.GetForegroundWindow() == hwnd,
            'rect': [rect.left, rect.top, rect.right - rect.left, rect.bottom - rect.top]}


def top_windows() -> list[dict]:
    found: list[int] = []
    proc = ctypes.WINFUNCTYPE(ctypes.c_bool, wt.HWND, wt.LPARAM)

    def add(hwnd, _):
        if user32.IsWindowVisible(hwnd) and user32.GetWindowTextLengthW(hwnd):
            found.append(hwnd)
        return True
    user32.EnumWindows(proc(add), 0)
    return [w for w in map(window_info, found) if w]


# ---- Commands --------------------------------------------------------------------------------

def cmd_windows(a) -> int:
    pattern = re.compile(a.title, re.I) if a.title else None
    for w in top_windows():
        if pattern and not pattern.search(w['title']):
            continue
        flags = ''.join(f for f, on in (('M', w['minimized']), ('F', w['foreground'])) if on)
        print(f"{w['hwnd']:>10}  pid={w['pid']:<6} {w['exe']:<28} {str(w['rect']):<26} {flags:<2} {w['title']}")
    return 0


def chrome_for_testing() -> Path:
    if os.environ.get('AMORIS_CHROME'):
        return Path(os.environ['AMORIS_CHROME'])
    root = Path(os.environ.get('PUPPETEER_CACHE_DIR', Path.home()/'.cache'/'puppeteer'))/'chrome'
    versions = sorted(root.glob('win64-*'), key=lambda p: [int(x) for x in p.name[6:].split('.')], reverse=True)
    for v in versions:
        exe = v/'chrome-win64'/'chrome.exe'
        if exe.is_file():
            return exe
    raise SystemExit('No Chrome for Testing found (npx @puppeteer/browsers install chrome@stable, or AMORIS_CHROME)')


def cmd_launch_browser(a) -> int:
    # The switches keep a covered window painting; without them Chrome stops drawing a window
    # that Windows reports occluded and the track would hold its last frame.
    w, h = (int(x) for x in a.size.split(','))
    args = [str(chrome_for_testing()), f'--user-data-dir={Path(a.profile).resolve()}', '--no-first-run',
            '--no-default-browser-check', '--disable-backgrounding-occluded-windows',
            '--disable-background-timer-throttling', '--disable-renderer-backgrounding',
            '--disable-features=CalculateNativeWinOcclusion', f'--window-size={w},{h}',
            # Chrome picks a hybrid laptop's integrated GPU for WebGPU unless told otherwise.
            '--enable-unsafe-webgpu', '--force_high_performance_gpu',
            *(f'--remote-debugging-port={a.cdp_port}',) * bool(a.cdp_port),
            '--new-window', a.url]
    subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                     creationflags=DETACHED | NEW_GROUP)
    print(json.dumps({'launched': args[0], 'url': a.url, 'cdp_port': a.cdp_port}))
    return 0


def cmd_launch(a) -> int:
    # SW_SHOWNOACTIVATE asks for the first window shown without activation; Windows' foreground
    # lock also keeps a process started from the background from taking the foreground.
    info = subprocess.STARTUPINFO()
    info.dwFlags |= subprocess.STARTF_USESHOWWINDOW
    info.wShowWindow = 4  # SW_SHOWNOACTIVATE
    env = dict(os.environ, **dict(kv.split('=', 1) for kv in a.env))
    log = open(a.log, 'ab') if a.log else subprocess.DEVNULL
    proc = subprocess.Popen(a.cmd, startupinfo=info, env=env, stdin=subprocess.DEVNULL, stdout=log,
                            stderr=log, creationflags=DETACHED | NEW_GROUP)
    print(json.dumps({'pid': proc.pid, 'cmd': a.cmd}))
    return 0


def cmd_bind(a) -> int:
    tracks = {}
    for item in a.window:
        role, _, value = item.partition('=')
        info = window_info(int(value))
        if not role or not info:
            raise SystemExit(f'No window for {item!r}')
        if info['minimized']:
            raise SystemExit(f'{role}: window {value} is minimized; restore it first')
        tracks[role] = {k: info[k] for k in ('hwnd', 'pid', 'exe', 'title', 'class')}
    config = {'tracks': tracks, 'seconds': a.seconds, 'segment': a.segment, 'fps': a.fps,
              'max_width': a.max_width, 'encoder': a.encoder, 'bound_utc': utc()}
    Path(a.config).parent.mkdir(parents=True, exist_ok=True)
    Path(a.config).write_text(json.dumps(config, indent=2), encoding='utf-8')
    print(json.dumps(config, indent=2))
    return 0


def detached(args: list[str], **kw) -> subprocess.Popen:
    for flags in (DETACHED | NEW_GROUP | BREAKAWAY, DETACHED | NEW_GROUP):
        try:
            return subprocess.Popen(args, creationflags=flags, **kw)
        except OSError:
            continue
    raise SystemExit('Could not start the detached supervisor')


def cmd_start(a) -> int:
    out = Path(a.out).resolve()
    if out.exists() and any(out.iterdir()):
        raise SystemExit(f'{out} is not empty: every session starts in a fresh directory')
    out.mkdir(parents=True, exist_ok=True)
    shutil.copy(a.config, out/'selection.json')
    log = open(out/'supervisor.log', 'ab')
    pythonw = Path(sys.executable).with_name('pythonw.exe')
    exe = str(pythonw if pythonw.is_file() else sys.executable)
    detached([exe, str(Path(__file__).resolve()), '_supervise', '--out', str(out)],
             stdin=subprocess.DEVNULL, stdout=log, stderr=log)
    deadline = time.time() + 45
    while time.time() < deadline:
        time.sleep(1)
        s = read_status(out)
        if s and s['state'] == 'recording' and s['healthy'] and all(t['frames'] > 0 for t in s['tracks'].values()):
            break
    return print_status(out)


def read_status(out: Path) -> dict | None:
    try:
        s = json.loads((out/'status.json').read_text(encoding='utf-8'))
    except (OSError, ValueError):
        return None
    age = time.time() - (out/'status.json').stat().st_mtime
    if s['state'] in ('starting', 'recording', 'stopping') and age > STALE_SUPERVISOR:
        s['state'], s['healthy'] = 'dead', False
        s.setdefault('problems', []).append(f'supervisor silent for {age:.0f} s')
    return s


def print_status(out: Path) -> int:
    s = read_status(Path(out).resolve())
    if not s:
        print('no status yet')
        return 1
    print(f"state={s['state']} healthy={str(s['healthy']).lower()} since={s['started_utc']} "
          f"elapsed={s['elapsed_s']:.0f}s left={s['left_s']:.0f}s disk_free={s['disk_free_gb']:.1f}GB")
    for role, t in s['tracks'].items():
        print(f"  {role:<8} frames={t['frames']:<7} fps={t['fps']:<5} segments={t['segments']:<3} "
              f"MB={t['bytes'] / 1e6:<8.1f} idle={t['idle_s']:<6.1f} restarts={t['restarts']} "
              f"window={'ok' if t['window'] else 'MISSING'}{' MINIMIZED' if t.get('minimized') else ''} "
              f"preview={t['preview']}")
    for p in s.get('problems', []):
        print(f'  problem: {p}')
    return 0 if s['healthy'] or s['state'] in ('stopped', 'expired') else 2


def cmd_status(a) -> int:
    return print_status(Path(a.out))


def cmd_stop(a) -> int:
    out = Path(a.out).resolve()
    (out/'stop').write_text(utc(), encoding='utf-8')
    deadline = time.time() + 60
    while time.time() < deadline:
        s = read_status(out)
        if s and s['state'] in ('stopped', 'expired', 'dead'):
            break
        time.sleep(1)
    return print_status(out)


# ---- Supervisor ------------------------------------------------------------------------------

class Track:
    def __init__(self, role: str, spec: dict, config: dict, out: Path):
        self.role, self.spec, self.config = role, spec, config
        self.dir = out/role
        self.dir.mkdir(parents=True, exist_ok=True)
        self.proc: subprocess.Popen | None = None
        self.preview_proc: subprocess.Popen | None = None
        self.runs: list[dict] = []
        self.frames = self.base_frames = 0
        self.fps = '0'
        self.last_progress = self.last_frame_change = time.time()
        self.lock = threading.Lock()

    def command(self, run_dir: Path) -> list[str]:
        c = self.config
        width = c['max_width']
        # No second output for the preview: a split keeps a second set of frame buffers and
        # doubles the track's memory (about 0.9 GB each at 2880 x 1800); `preview` reads the
        # newest segment instead.
        graph = (f"gfxcapture=hwnd={self.spec['hwnd']}:max_framerate={c['fps']}:capture_cursor=0:"
                 f"resize_mode=scale_aspect,hwdownload,format=bgra,fps={c['fps']},"
                 f"scale='trunc(min({width},iw)/2)*2':-2,format=yuv420p[v]")
        enc = c['encoder']
        quality = ['-preset', 'p4', '-rc', 'vbr', '-cq', '24'] if 'nvenc' in enc else ['-preset', 'veryfast', '-crf', '23']
        return ['ffmpeg', '-hide_banner', '-nostats', '-loglevel', 'warning', '-progress', 'pipe:1',
                '-filter_complex', graph,
                '-map', '[v]', '-c:v', enc, *quality, '-g', str(c['fps'] * 2),
                '-f', 'segment', '-segment_time', str(c['segment']), '-reset_timestamps', '1',
                '-segment_format', 'mp4',
                '-segment_format_options', 'movflags=+frag_keyframe+empty_moov+default_base_moof',
                '-segment_list', str(run_dir/'segments.csv'), '-segment_list_type', 'csv',
                str(run_dir/'seg-%05d.mp4')]

    def launch(self) -> None:
        run_dir = self.dir/f'run-{len(self.runs) + 1:02d}'
        run_dir.mkdir()
        log = open(run_dir/'ffmpeg.log', 'ab')
        self.proc = subprocess.Popen(self.command(run_dir), stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=log, creationflags=NO_WINDOW)
        self.base_frames = self.frames
        self.runs.append({'dir': run_dir.name, 'start_utc': utc(), 'start_unix': time.time(), 'exit': None})
        threading.Thread(target=self.read_progress, args=(self.proc,), daemon=True).start()

    def read_progress(self, proc: subprocess.Popen) -> None:
        for raw in proc.stdout:
            key, _, value = raw.decode(errors='replace').strip().partition('=')
            with self.lock:
                if key == 'frame':
                    frames = self.base_frames + int(value or 0)
                    if frames != self.frames:
                        self.last_frame_change = time.time()
                    self.frames = frames
                elif key == 'fps':
                    self.fps = value
                elif key == 'progress':
                    self.last_progress = time.time()

    def preview(self) -> None:
        """Writes preview.jpg from the newest segment's last second, in the background."""
        if self.preview_proc is not None and self.preview_proc.poll() is None:
            return
        files = sorted(self.dir.glob('run-*/seg-*.mp4'), key=lambda f: f.stat().st_mtime)
        if not files:
            return
        self.preview_proc = subprocess.Popen(
            ['ffmpeg', '-hide_banner', '-loglevel', 'error', '-y', '-sseof', '-1.5', '-i', str(files[-1]),
             '-frames:v', '1', '-vf', 'scale=640:-2', '-q:v', '5', str(self.dir/'preview.jpg')],
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            creationflags=NO_WINDOW)

    def stop(self) -> None:
        if self.proc and self.proc.poll() is None:
            try:
                self.proc.stdin.write(b'q')
                self.proc.stdin.flush()
            except OSError:
                pass

    def finish(self, timeout: float) -> None:
        if self.proc:
            try:
                self.proc.wait(timeout)
            except subprocess.TimeoutExpired:
                self.proc.kill()
            self.runs[-1]['exit'] = self.proc.returncode
            self.runs[-1]['end_utc'] = utc()

    def state(self, now: float) -> dict:
        info = window_info(self.spec['hwnd'])
        files = sorted(self.dir.glob('run-*/seg-*.mp4'))
        with self.lock:
            return {'hwnd': self.spec['hwnd'], 'title': info['title'] if info else self.spec['title'],
                    'window': bool(info), 'minimized': bool(info and info['minimized']),
                    'running': bool(self.proc and self.proc.poll() is None),
                    'frames': self.frames, 'fps': self.fps, 'segments': len(files),
                    'bytes': sum(f.stat().st_size for f in files),
                    'progress_age_s': round(now - self.last_progress, 1),
                    'idle_s': round(now - self.last_frame_change, 1), 'restarts': len(self.runs) - 1,
                    'runs': self.runs, 'preview': str(self.dir/'preview.jpg')}


def supervise(a) -> int:
    out = Path(a.out)
    config = json.loads((out/'selection.json').read_text(encoding='utf-8'))
    started = time.time()
    deadline = started + config['seconds']
    tracks = {role: Track(role, spec, config, out) for role, spec in config['tracks'].items()}
    status = {'state': 'starting', 'started_utc': utc(), 'started_unix': started, 'pid': os.getpid(),
              'deadline_utc': dt.datetime.fromtimestamp(deadline, dt.timezone.utc).isoformat(timespec='seconds')}

    def write(state: str) -> None:
        now = time.time()
        states = {r: t.state(now) for r, t in tracks.items()}
        problems = []
        for r, t in states.items():
            if not t['window']:
                problems.append(f'{r}: window {t["hwnd"]} is gone')
            elif t['minimized']:
                problems.append(f'{r}: window is minimized and records nothing')
            if state == 'recording' and not t['running']:
                problems.append(f'{r}: ffmpeg is not running')
            elif state == 'recording' and t['progress_age_s'] > STALE_PROGRESS:
                problems.append(f'{r}: no ffmpeg progress for {t["progress_age_s"]} s')
        free = shutil.disk_usage(out).free / 1e9
        if free < LOW_DISK_GB:
            problems.append(f'low disk: {free:.1f} GB free')
        status.update({'state': state, 'healthy': state == 'recording' and not problems,
                       'problems': problems, 'updated_utc': utc(), 'elapsed_s': now - started,
                       'left_s': max(0.0, deadline - now), 'disk_free_gb': free, 'tracks': states})
        tmp = out/'status.json.tmp'
        tmp.write_text(json.dumps(status, indent=2), encoding='utf-8')
        # Windows refuses to replace a file another process has open (a reader of status.json):
        # try for a second, then skip this write rather than die; the next one tries again.
        for _ in range(10):
            try:
                os.replace(tmp, out/'status.json')
                break
            except PermissionError:
                time.sleep(0.1)

    for t in tracks.values():
        t.launch()
    write('recording')
    final = 'stopped'
    beats = 0
    while True:
        time.sleep(STATUS_EVERY)
        beats += 1
        if beats % 5 == 0:
            for t in tracks.values():
                t.preview()
        if (out/'stop').exists():
            break
        if time.time() >= deadline:
            final = 'expired'
            break
        for t in tracks.values():
            # A track whose ffmpeg ended while its window still exists starts a new run; a gone
            # window stays reported until the session is stopped and a fresh one bound.
            if t.proc.poll() is not None:
                t.runs[-1]['exit'], t.runs[-1]['end_utc'] = t.proc.returncode, utc()
                if window_info(t.spec['hwnd']) and len(t.runs) < 50:
                    t.launch()
        write('recording')
    write('stopping')
    for t in tracks.values():
        t.stop()
    for t in tracks.values():
        t.finish(30)
        write('stopping')
    write(final)
    return 0


# ---- After a session -------------------------------------------------------------------------

def probe(path: Path) -> dict:
    r = subprocess.run(['ffprobe', '-v', 'error', '-count_packets', '-show_entries',
                        'format=duration:stream=width,height,nb_read_packets', '-of', 'json', str(path)],
                       capture_output=True, text=True)
    if r.returncode:
        return {'error': r.stderr.strip()}
    j = json.loads(r.stdout)
    s = j['streams'][0] if j.get('streams') else {}
    return {'duration': float(j['format'].get('duration', 0)), 'packets': int(s.get('nb_read_packets', 0)),
            'size': f"{s.get('width')}x{s.get('height')}"}


def segments(out: Path, role: str) -> list[Path]:
    return sorted((out/role).glob('run-*/seg-*.mp4'))


def cmd_check(a) -> int:
    out = Path(a.out).resolve()
    s = read_status(out) or {}
    bad = 0
    for role in sorted(p.name for p in out.iterdir() if p.is_dir()):
        total, packets = 0.0, 0
        for seg in segments(out, role):
            p = probe(seg)
            if 'error' in p or not p['packets']:
                bad += 1
                print(f'  BAD {seg.relative_to(out)}: {p.get("error", "no packets")}')
                continue
            total += p['duration']
            packets += p['packets']
        listed = sum(1 for csv_file in (out/role).glob('run-*/segments.csv')
                     for _ in csv.reader(open(csv_file, encoding='utf-8')))
        runs = (s.get('tracks', {}).get(role) or {}).get('runs', [])
        print(f'{role:<8} segments={len(segments(out, role))} listed={listed} runs={len(runs)} '
              f'duration={total:.1f}s packets={packets}')
    print(f"session state={s.get('state')} elapsed={s.get('elapsed_s', 0):.0f}s")
    return 1 if bad else 0


def cmd_stitch(a) -> int:
    out = Path(a.out).resolve()
    files = segments(out, a.role)
    if not files:
        raise SystemExit(f'No segments for {a.role}')
    listing = Path(a.file).with_suffix('.concat.txt')
    listing.write_text(''.join(f"file '{f.as_posix()}'\n" for f in files), encoding='utf-8')
    r = subprocess.run(['ffmpeg', '-hide_banner', '-loglevel', 'error', '-y', '-f', 'concat', '-safe', '0',
                        '-i', str(listing), '-c', 'copy', '-movflags', '+faststart', str(a.file)])
    return r.returncode


def cmd_recover(a) -> int:
    # Fragmented MP4 segments stay readable after a killed recorder; remux what was written.
    out, dest = Path(a.out).resolve(), Path(a.dest).resolve()
    if dest.exists() and any(dest.iterdir()):
        raise SystemExit(f'{dest} is not empty')
    for seg in sorted(out.glob('*/run-*/seg-*.mp4')):
        target = dest/seg.relative_to(out)
        target.parent.mkdir(parents=True, exist_ok=True)
        r = subprocess.run(['ffmpeg', '-hide_banner', '-loglevel', 'error', '-i', str(seg), '-c', 'copy', str(target)])
        print(f"{'ok ' if r.returncode == 0 else 'BAD'} {seg.relative_to(out)}")
    return 0


def main() -> int:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest='command', required=True)
    w = sub.add_parser('windows')
    w.add_argument('--title')
    lb = sub.add_parser('launch-browser')
    lb.add_argument('--url', required=True)
    lb.add_argument('--profile', required=True)
    lb.add_argument('--size', default='1600,1000')
    lb.add_argument('--cdp-port', type=int, default=0)
    la = sub.add_parser('launch', help='start a GUI program without activating its window')
    la.add_argument('--env', action='append', default=[], help='NAME=VALUE')
    la.add_argument('--log')
    la.add_argument('cmd', nargs=argparse.REMAINDER)
    b = sub.add_parser('bind')
    b.add_argument('--window', action='append', required=True, help='ROLE=HWND')
    b.add_argument('--config', required=True)
    b.add_argument('--seconds', type=int, default=43200)
    b.add_argument('--segment', type=int, default=300)
    b.add_argument('--fps', type=int, default=30)
    b.add_argument('--max-width', type=int, default=2560)
    b.add_argument('--encoder', default='h264_nvenc')
    st = sub.add_parser('start')
    st.add_argument('--config', required=True)
    st.add_argument('--out', required=True)
    for name in ('status', 'stop', 'check', '_supervise'):
        sub.add_parser(name).add_argument('--out', required=True)
    sti = sub.add_parser('stitch')
    sti.add_argument('--out', required=True)
    sti.add_argument('--role', required=True)
    sti.add_argument('--file', required=True)
    rec = sub.add_parser('recover')
    rec.add_argument('--out', required=True)
    rec.add_argument('--dest', required=True)
    a = p.parse_args()
    if sys.platform != 'win32':
        raise SystemExit('This recorder uses Windows Graphics Capture; it runs on Windows only')
    if a.command == 'launch':
        a.cmd = a.cmd[1:] if a.cmd[:1] == ['--'] else a.cmd
    return {'windows': cmd_windows, 'launch-browser': cmd_launch_browser, 'launch': cmd_launch, 'bind': cmd_bind,
            'start': cmd_start, 'status': cmd_status, 'stop': cmd_stop, 'check': cmd_check,
            'stitch': cmd_stitch, 'recover': cmd_recover, '_supervise': supervise}[a.command](a)


if __name__ == '__main__':
    sys.exit(main())
