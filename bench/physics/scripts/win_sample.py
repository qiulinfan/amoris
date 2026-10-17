#!/usr/bin/env python3
"""A sampling profiler for one Windows program that needs no elevation (docs/bench/physics-r1.md,
"Where R1's step time goes"). Kernel CPU sampling (WPR, xperf) needs an administrator's
SeSystemProfilePrivilege; this suspends the program's running threads about every millisecond,
walks their stacks with dbghelp's StackWalk64 and resumes them, the way a debugger would.

    python bench/physics/scripts/win_sample.py --out DIR [--interval-ms 1] [--skip-s 1]
        -- PROGRAM ARGS...

A thread is sampled only when its cycle count (QueryThreadCycleTime) advanced by --min-cycles
since the previous tick, so waiting threads are not counted (suspending a waiting thread makes it
run its suspend APC, a few thousand cycles). Symbols come from the program's PDB (the bench's release
profile keeps line tables) and, for system DLLs, from their exports only: no symbol server is
contacted, so a leaf inside ntdll is named after the nearest export before it and only the module
is exact. Writes DIR/profile.json (the module list and every stack, resolved) and prints the
profile: samples by leaf module, by leaf function, and inclusive by function.
"""
import argparse
import ctypes
import json
import subprocess
import sys
import time
from collections import Counter
from ctypes import wintypes as w
from pathlib import Path

k32 = ctypes.WinDLL("kernel32", use_last_error=True)
psapi = ctypes.WinDLL("psapi", use_last_error=True)
dbghelp = ctypes.WinDLL("dbghelp", use_last_error=True)

PROCESS_ALL_ACCESS = 0x1F0FFF
THREAD_ACCESS = 0x0002 | 0x0008 | 0x0040  # suspend/resume, get context, query information
TH32CS_SNAPTHREAD = 0x4
CONTEXT_FULL = 0x10000B
IMAGE_FILE_MACHINE_AMD64 = 0x8664
ADDR_MODE_FLAT = 3
LIST_MODULES_ALL = 3
SYMOPT_UNDNAME, SYMOPT_FAIL_CRITICAL_ERRORS, SYMOPT_NO_PROMPTS = 0x2, 0x200, 0x80000
CONTEXT_SIZE = 1232  # x64 CONTEXT; Rip at 0xF8, Rsp at 0x98, Rbp at 0xA0, ContextFlags at 0x30
MAX_FRAMES = 96


class THREADENTRY32(ctypes.Structure):
    _fields_ = [("dwSize", w.DWORD), ("cntUsage", w.DWORD), ("th32ThreadID", w.DWORD),
                ("th32OwnerProcessID", w.DWORD), ("tpBasePri", w.LONG), ("tpDeltaPri", w.LONG),
                ("dwFlags", w.DWORD)]


class ADDRESS64(ctypes.Structure):
    _fields_ = [("Offset", ctypes.c_uint64), ("Segment", w.WORD), ("Mode", ctypes.c_int)]


class STACKFRAME64(ctypes.Structure):
    # KdHelp (KDHELP64, 152 bytes in current SDKs) is left as an oversized zeroed buffer.
    _fields_ = [("AddrPC", ADDRESS64), ("AddrReturn", ADDRESS64), ("AddrFrame", ADDRESS64),
                ("AddrStack", ADDRESS64), ("AddrBStore", ADDRESS64),
                ("FuncTableEntry", ctypes.c_void_p), ("Params", ctypes.c_uint64 * 4),
                ("Far", w.BOOL), ("Virtual", w.BOOL), ("Reserved", ctypes.c_uint64 * 3),
                ("KdHelp", ctypes.c_ubyte * 256)]


class SYMBOL_INFO(ctypes.Structure):
    _fields_ = [("SizeOfStruct", w.ULONG), ("TypeIndex", w.ULONG),
                ("Reserved", ctypes.c_uint64 * 2), ("Index", w.ULONG), ("Size", w.ULONG),
                ("ModBase", ctypes.c_uint64), ("Flags", w.ULONG), ("Value", ctypes.c_uint64),
                ("Address", ctypes.c_uint64), ("Register", w.ULONG), ("Scope", w.ULONG),
                ("Tag", w.ULONG), ("NameLen", w.ULONG), ("MaxNameLen", w.ULONG),
                ("Name", ctypes.c_char * 1)]


class MODULEINFO(ctypes.Structure):
    _fields_ = [("lpBaseOfDll", ctypes.c_void_p), ("SizeOfImage", w.DWORD),
                ("EntryPoint", ctypes.c_void_p)]


k32.OpenProcess.restype = w.HANDLE
k32.OpenThread.restype = w.HANDLE
k32.CreateToolhelp32Snapshot.restype = w.HANDLE
k32.SuspendThread.argtypes = [w.HANDLE]
k32.ResumeThread.argtypes = [w.HANDLE]
k32.GetThreadContext.argtypes = [w.HANDLE, ctypes.c_void_p]
k32.QueryThreadCycleTime.argtypes = [w.HANDLE, ctypes.POINTER(ctypes.c_uint64)]
dbghelp.SymInitializeW.argtypes = [w.HANDLE, w.LPCWSTR, w.BOOL]
dbghelp.SymFromAddr.argtypes = [w.HANDLE, ctypes.c_uint64, ctypes.POINTER(ctypes.c_uint64),
                                ctypes.c_void_p]
dbghelp.StackWalk64.argtypes = [w.DWORD, w.HANDLE, w.HANDLE, ctypes.POINTER(STACKFRAME64),
                                ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p,
                                ctypes.c_void_p, ctypes.c_void_p]
FUNCTION_TABLE_ACCESS = ctypes.cast(dbghelp.SymFunctionTableAccess64, ctypes.c_void_p)
GET_MODULE_BASE = ctypes.cast(dbghelp.SymGetModuleBase64, ctypes.c_void_p)


def threads_of(pid):
    snap = k32.CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)
    e = THREADENTRY32(dwSize=ctypes.sizeof(THREADENTRY32))
    ids = []
    ok = k32.Thread32First(snap, ctypes.byref(e))
    while ok:
        if e.th32OwnerProcessID == pid:
            ids.append(e.th32ThreadID)
        ok = k32.Thread32Next(snap, ctypes.byref(e))
    k32.CloseHandle(snap)
    return ids


def modules_of(hproc):
    arr = (ctypes.c_void_p * 1024)()
    needed = w.DWORD()
    psapi.EnumProcessModulesEx(hproc, arr, ctypes.sizeof(arr), ctypes.byref(needed),
                               LIST_MODULES_ALL)
    mods = []
    for i in range(needed.value // ctypes.sizeof(ctypes.c_void_p)):
        info = MODULEINFO()
        psapi.GetModuleInformation(hproc, ctypes.c_void_p(arr[i]), ctypes.byref(info),
                                   ctypes.sizeof(info))
        name = ctypes.create_unicode_buffer(260)
        psapi.GetModuleBaseNameW(hproc, ctypes.c_void_p(arr[i]), name, 260)
        mods.append({"name": name.value, "base": info.lpBaseOfDll or 0,
                     "size": info.SizeOfImage})
    return mods


def walk(hproc, hthread, ctx):
    """The thread's return addresses, leaf first (the thread is suspended)."""
    ctypes.memmove(ctx, b"\0" * CONTEXT_SIZE, CONTEXT_SIZE)
    ctypes.c_uint32.from_address(ctx + 0x30).value = CONTEXT_FULL
    if not k32.GetThreadContext(hthread, ctx):
        return []
    f = STACKFRAME64()
    for field, off in (("AddrPC", 0xF8), ("AddrStack", 0x98), ("AddrFrame", 0xA0)):
        a = getattr(f, field)
        a.Offset = ctypes.c_uint64.from_address(ctx + off).value
        a.Mode = ADDR_MODE_FLAT
    pcs = []
    while len(pcs) < MAX_FRAMES and dbghelp.StackWalk64(
            IMAGE_FILE_MACHINE_AMD64, hproc, hthread, ctypes.byref(f), ctx, None,
            FUNCTION_TABLE_ACCESS, GET_MODULE_BASE, None):
        if f.AddrPC.Offset == 0:
            break
        pcs.append(f.AddrPC.Offset)
    return pcs


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--out", required=True)
    ap.add_argument("--interval-ms", type=float, default=1.0)
    ap.add_argument("--skip-s", type=float, default=1.0, help="start sampling after this long")
    ap.add_argument("--top", type=int, default=30)
    ap.add_argument("--min-cycles", type=int, default=200_000,
                    help="cycles a thread must have run since the last tick to be sampled "
                         "(suspending a waiting thread makes it run a little)")
    ap.add_argument("cmd", nargs=argparse.REMAINDER)
    a = ap.parse_args()
    cmd = a.cmd[1:] if a.cmd and a.cmd[0] == "--" else a.cmd
    if Path(cmd[0]).exists():
        cmd[0] = str(Path(cmd[0]).resolve())
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)

    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    hproc = k32.OpenProcess(PROCESS_ALL_ACCESS, False, proc.pid)
    time.sleep(a.skip_s)
    dbghelp.SymSetOptions(SYMOPT_UNDNAME | SYMOPT_FAIL_CRITICAL_ERRORS | SYMOPT_NO_PROMPTS)
    # An explicit search path (the program's directory) keeps _NT_SYMBOL_PATH and any symbol
    # server out; every module's symbols load now, before the process can exit.
    if not dbghelp.SymInitializeW(hproc, str(Path(cmd[0]).resolve().parent), True):
        sys.exit(f"SymInitialize failed: {ctypes.get_last_error()}")
    mods = modules_of(hproc)
    raw = (ctypes.c_ubyte * (CONTEXT_SIZE + 16))()
    ctx = (ctypes.addressof(raw) + 15) & ~15
    handles, cycles, stacks = {}, {}, []
    t_start = time.perf_counter()
    ticks, listed = 0, -1.0
    while proc.poll() is None:
        # A thread snapshot lists the whole system's threads (tens of milliseconds): refresh the
        # program's thread list a few times a second, not every tick.
        if time.perf_counter() - listed > 0.25:
            listed = time.perf_counter()
            for tid in threads_of(proc.pid):
                if tid not in handles:
                    handles[tid] = k32.OpenThread(THREAD_ACCESS, False, tid)
        for tid, h in handles.items():
            c = ctypes.c_uint64()
            if not h or not k32.QueryThreadCycleTime(h, ctypes.byref(c)):
                continue
            ran = c.value - cycles.get(tid, c.value) >= a.min_cycles
            cycles[tid] = c.value
            if not ran or k32.SuspendThread(h) == 0xFFFFFFFF:
                continue
            try:
                pcs = walk(hproc, h, ctx)
            finally:
                k32.ResumeThread(h)
            if pcs:
                stacks.append((tid, pcs))
        ticks += 1
        time.sleep(a.interval_ms / 1000.0)
    elapsed = time.perf_counter() - t_start
    stdout, stderr = proc.communicate()

    def module(pc):
        for m in mods:
            if m["base"] <= pc < m["base"] + m["size"]:
                return m["name"]
        return "?"

    buf = ctypes.create_string_buffer(ctypes.sizeof(SYMBOL_INFO) + 1024)
    sym = ctypes.cast(buf, ctypes.POINTER(SYMBOL_INFO)).contents
    names = {}

    def name(pc):
        if pc not in names:
            sym.SizeOfStruct = ctypes.sizeof(SYMBOL_INFO)
            sym.MaxNameLen = 1024
            disp = ctypes.c_uint64()
            if dbghelp.SymFromAddr(hproc, pc, ctypes.byref(disp), buf):
                n = ctypes.string_at(ctypes.addressof(sym) + SYMBOL_INFO.Name.offset,
                                     sym.NameLen).decode("utf-8", "replace")
            else:
                n = f"0x{pc:x}"
            names[pc] = f"{module(pc)}!{n}"
        return names[pc]

    resolved = [(tid, [name(pc) for pc in pcs]) for tid, pcs in stacks]
    dbghelp.SymCleanup(hproc)
    (out / "profile.json").write_text(json.dumps({
        "cmd": cmd, "interval_ms": a.interval_ms, "skip_s": a.skip_s, "ticks": ticks,
        "elapsed_s": round(elapsed, 2), "modules": mods, "stdout": stdout, "stderr": stderr,
        "stacks": resolved}) + "\n", encoding="utf-8")

    n = len(resolved)
    if not n:
        sys.exit("no samples")
    print(f"{n} samples from {len({t for t, _ in resolved})} thread(s) over {elapsed:.1f} s "
          f"({ticks} ticks); program exit {proc.returncode}")
    print(stdout.strip().splitlines()[-1] if stdout.strip() else "")
    per_thread = Counter(t for t, _ in resolved)
    print("Samples per thread: " + ", ".join(f"{t}: {v}" for t, v in per_thread.most_common()))
    leaf_mod = Counter(s[0].split("!", 1)[0] for _, s in resolved)
    leaf_fn = Counter(s[0] for _, s in resolved)
    incl = Counter(f for _, s in resolved for f in set(s))
    for title, c in (("Leaf module", leaf_mod), ("Leaf function", leaf_fn),
                     ("Inclusive function", incl)):
        print(f"\n{title}: % of samples")
        for k, v in c.most_common(a.top):
            print(f"  {100.0 * v / n:5.1f}  {k[:160]}")


if __name__ == "__main__":
    main()
