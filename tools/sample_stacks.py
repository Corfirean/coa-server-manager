"""Poor man's sampling profiler for a running worldserver (Windows, 64-bit, needs worldserver.pdb next to the exe).

    python tools/sample_stacks.py <pid> [seconds=30] [hz=100] [--threads N] [--depth D] [--out report.txt]

Every 1/hz seconds each busy thread is suspended for a moment, its call stack is walked with dbghelp and symbolised from
the PDB, then the thread is resumed. Prints, per thread, the functions that were on the stack most often (inclusive) and
the ones that were running (self). Only for diagnosis: sampling slows the process a little.
"""
import argparse
import collections
import ctypes
import ctypes.wintypes as wt
import os
import sys
import time

k32 = ctypes.WinDLL("kernel32", use_last_error=True)
dbg = ctypes.WinDLL("dbghelp")

THREAD_ALL = 0x1FFFFF
PROCESS_ALL = 0x1FFFFF
CONTEXT_AMD64 = 0x100000
CONTEXT_CONTROL = CONTEXT_AMD64 | 0x1
CONTEXT_INTEGER = CONTEXT_AMD64 | 0x2
TH32CS_SNAPTHREAD = 0x4


class THREADENTRY32(ctypes.Structure):
    _fields_ = [("dwSize", wt.DWORD), ("cntUsage", wt.DWORD), ("th32ThreadID", wt.DWORD), ("th32OwnerProcessID", wt.DWORD),
                ("tpBasePri", wt.LONG), ("tpDeltaPri", wt.LONG), ("dwFlags", wt.DWORD)]


class M128A(ctypes.Structure):
    _fields_ = [("Low", ctypes.c_ulonglong), ("High", ctypes.c_longlong)]


class CONTEXT(ctypes.Structure):
    _pack_ = 16
    _fields_ = [("P1Home", ctypes.c_ulonglong), ("P2Home", ctypes.c_ulonglong), ("P3Home", ctypes.c_ulonglong),
                ("P4Home", ctypes.c_ulonglong), ("P5Home", ctypes.c_ulonglong), ("P6Home", ctypes.c_ulonglong),
                ("ContextFlags", wt.DWORD), ("MxCsr", wt.DWORD),
                ("SegCs", wt.WORD), ("SegDs", wt.WORD), ("SegEs", wt.WORD), ("SegFs", wt.WORD), ("SegGs", wt.WORD), ("SegSs", wt.WORD),
                ("EFlags", wt.DWORD),
                ("Dr0", ctypes.c_ulonglong), ("Dr1", ctypes.c_ulonglong), ("Dr2", ctypes.c_ulonglong), ("Dr3", ctypes.c_ulonglong),
                ("Dr6", ctypes.c_ulonglong), ("Dr7", ctypes.c_ulonglong),
                ("Rax", ctypes.c_ulonglong), ("Rcx", ctypes.c_ulonglong), ("Rdx", ctypes.c_ulonglong), ("Rbx", ctypes.c_ulonglong),
                ("Rsp", ctypes.c_ulonglong), ("Rbp", ctypes.c_ulonglong), ("Rsi", ctypes.c_ulonglong), ("Rdi", ctypes.c_ulonglong),
                ("R8", ctypes.c_ulonglong), ("R9", ctypes.c_ulonglong), ("R10", ctypes.c_ulonglong), ("R11", ctypes.c_ulonglong),
                ("R12", ctypes.c_ulonglong), ("R13", ctypes.c_ulonglong), ("R14", ctypes.c_ulonglong), ("R15", ctypes.c_ulonglong),
                ("Rip", ctypes.c_ulonglong),
                ("FltSave", ctypes.c_ubyte * 512),
                ("VectorRegister", M128A * 26), ("VectorControl", ctypes.c_ulonglong),
                ("DebugControl", ctypes.c_ulonglong), ("LastBranchToRip", ctypes.c_ulonglong), ("LastBranchFromRip", ctypes.c_ulonglong),
                ("LastExceptionToRip", ctypes.c_ulonglong), ("LastExceptionFromRip", ctypes.c_ulonglong)]


class ADDRESS64(ctypes.Structure):
    _fields_ = [("Offset", ctypes.c_ulonglong), ("Segment", wt.WORD), ("Mode", wt.DWORD)]


class KDHELP64(ctypes.Structure):
    _fields_ = [("Thread", ctypes.c_ulonglong), ("ThCallbackStack", wt.DWORD), ("ThCallbackBStore", wt.DWORD),
                ("NextCallback", wt.DWORD), ("FramePointer", wt.DWORD), ("KiCallUserMode", ctypes.c_ulonglong),
                ("KeUserCallbackDispatcher", ctypes.c_ulonglong), ("SystemRangeStart", ctypes.c_ulonglong),
                ("KiUserExceptionDispatcher", ctypes.c_ulonglong), ("StackBase", ctypes.c_ulonglong),
                ("StackLimit", ctypes.c_ulonglong), ("Reserved", ctypes.c_ulonglong * 5)]


class STACKFRAME64(ctypes.Structure):
    _fields_ = [("AddrPC", ADDRESS64), ("AddrReturn", ADDRESS64), ("AddrFrame", ADDRESS64), ("AddrStack", ADDRESS64),
                ("AddrBStore", ADDRESS64), ("FuncTableEntry", ctypes.c_void_p), ("Params", ctypes.c_ulonglong * 4),
                ("Far", wt.BOOL), ("Virtual", wt.BOOL), ("Reserved", ctypes.c_ulonglong * 3), ("KdHelp", KDHELP64)]


MAX_NAME = 512


class SYMBOL_INFO(ctypes.Structure):
    _fields_ = [("SizeOfStruct", wt.ULONG), ("TypeIndex", wt.ULONG), ("Reserved", ctypes.c_ulonglong * 2), ("Index", wt.ULONG),
                ("Size", wt.ULONG), ("ModBase", ctypes.c_ulonglong), ("Flags", wt.ULONG), ("Value", ctypes.c_ulonglong),
                ("Address", ctypes.c_ulonglong), ("Register", wt.ULONG), ("Scope", wt.ULONG), ("Tag", wt.ULONG),
                ("NameLen", wt.ULONG), ("MaxNameLen", wt.ULONG), ("Name", ctypes.c_char * MAX_NAME)]


k32.OpenProcess.restype = wt.HANDLE
k32.OpenThread.restype = wt.HANDLE
k32.GetCurrentProcess.restype = wt.HANDLE
dbg.SymFunctionTableAccess64.restype = ctypes.c_void_p
dbg.SymGetModuleBase64.restype = ctypes.c_ulonglong
dbg.SymFunctionTableAccess64.argtypes = [wt.HANDLE, ctypes.c_ulonglong]
dbg.SymGetModuleBase64.argtypes = [wt.HANDLE, ctypes.c_ulonglong]
dbg.StackWalk64.argtypes = [wt.DWORD, wt.HANDLE, wt.HANDLE, ctypes.POINTER(STACKFRAME64), ctypes.c_void_p, ctypes.c_void_p,
                            ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p]
dbg.SymFromAddr.argtypes = [wt.HANDLE, ctypes.c_ulonglong, ctypes.POINTER(ctypes.c_ulonglong), ctypes.POINTER(SYMBOL_INFO)]
dbg.SymInitialize.argtypes = [wt.HANDLE, ctypes.c_char_p, wt.BOOL]
dbg.SymSetOptions.argtypes = [wt.DWORD]

_name_cache = {}


def symbol(proc, addr):
    # cache by 64-byte block: good enough to group samples, saves a lookup per frame
    key = addr >> 4
    hit = _name_cache.get(key)
    if hit is not None:
        return hit
    info = SYMBOL_INFO()
    info.SizeOfStruct = 88  # sizeof(SYMBOL_INFO) in the Windows headers (Name[1])
    info.MaxNameLen = MAX_NAME - 1
    disp = ctypes.c_ulonglong(0)
    if dbg.SymFromAddr(proc, addr, ctypes.byref(disp), ctypes.byref(info)):
        name = info.Name[: info.NameLen].decode("utf-8", "replace")
    else:
        name = f"?{addr:#x}"
    _name_cache[key] = name
    return name


def threads_of(pid):
    snap = k32.CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)
    out = []
    te = THREADENTRY32()
    te.dwSize = ctypes.sizeof(te)
    ok = k32.Thread32First(snap, ctypes.byref(te))
    while ok:
        if te.th32OwnerProcessID == pid:
            out.append(te.th32ThreadID)
        ok = k32.Thread32Next(snap, ctypes.byref(te))
    k32.CloseHandle(snap)
    return out


def cpu_time(h):
    c, e, kt, ut = (wt.FILETIME() for _ in range(4))
    k32.GetThreadTimes(h, ctypes.byref(c), ctypes.byref(e), ctypes.byref(kt), ctypes.byref(ut))
    f = lambda x: (x.dwHighDateTime << 32 | x.dwLowDateTime) / 1e7
    return f(kt) + f(ut)


def aligned_context():
    raw = ctypes.create_string_buffer(ctypes.sizeof(CONTEXT) + 32)
    addr = ctypes.addressof(raw)
    off = (-addr) % 16
    ctx = CONTEXT.from_buffer(raw, off)
    return raw, ctx


def stack(proc, th, depth):
    raw, ctx = aligned_context()
    ctx.ContextFlags = CONTEXT_CONTROL | CONTEXT_INTEGER
    if k32.SuspendThread(th) == 0xFFFFFFFF:
        return None
    try:
        if not k32.GetThreadContext(th, ctypes.byref(ctx)):
            return None
        frame = STACKFRAME64()
        frame.AddrPC.Offset, frame.AddrPC.Mode = ctx.Rip, 3
        frame.AddrFrame.Offset, frame.AddrFrame.Mode = ctx.Rsp, 3
        frame.AddrStack.Offset, frame.AddrStack.Mode = ctx.Rsp, 3
        addrs = [ctx.Rip]
        for _ in range(depth):
            ok = dbg.StackWalk64(0x8664, proc, th, ctypes.byref(frame), ctypes.byref(ctx), None,
                                 dbg.SymFunctionTableAccess64, dbg.SymGetModuleBase64, None)
            if not ok or frame.AddrPC.Offset == 0:
                break
            addrs.append(frame.AddrPC.Offset)
    finally:
        k32.ResumeThread(th)
    return addrs


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("pid", type=int)
    ap.add_argument("seconds", type=float, nargs="?", default=30)
    ap.add_argument("hz", type=float, nargs="?", default=100)
    ap.add_argument("--threads", type=int, default=5)
    ap.add_argument("--depth", type=int, default=40)
    ap.add_argument("--out")
    ap.add_argument("--symbols", help="symbol path (folder with worldserver.pdb)")
    ap.add_argument("--under", action="append", default=[], help="function name: also report what it spends its time in (its direct callees); repeatable")
    a = ap.parse_args()

    proc = k32.OpenProcess(PROCESS_ALL, False, a.pid)
    if not proc:
        sys.exit("cannot open the process (run as the same user)")
    dbg.SymSetOptions(0x2 | 0x4 | 0x10 | 0x200)  # UNDNAME | DEFERRED_LOADS | LOAD_LINES | FAIL_CRITICAL_ERRORS off
    if not dbg.SymInitialize(proc, a.symbols.encode() if a.symbols else None, True):
        sys.exit("SymInitialize failed")

    tids = threads_of(a.pid)
    handles = {t: k32.OpenThread(THREAD_ALL, False, t) for t in tids}
    t0 = {t: cpu_time(h) for t, h in handles.items()}
    time.sleep(2)
    busy = sorted(((cpu_time(h) - t0[t]) / 2 * 100, t) for t, h in handles.items())[::-1]
    chosen = [t for pct, t in busy[: a.threads] if pct > 3]
    print("sampling threads:", ", ".join(f"{t} ({pct:.0f}%)" for pct, t in busy[: a.threads] if pct > 3), flush=True)

    incl = {t: collections.Counter() for t in chosen}
    selfc = {t: collections.Counter() for t in chosen}
    samples = {t: 0 for t in chosen}
    callees = {t: {u: collections.Counter() for u in a.under} for t in chosen}
    period = 1.0 / a.hz
    end = time.time() + a.seconds
    first = True
    while time.time() < end:
        for t in chosen:
            addrs = stack(proc, handles[t], a.depth)
            if not addrs:
                continue
            names = [symbol(proc, ad) for ad in addrs]
            samples[t] += 1
            selfc[t][names[0]] += 1
            for n in set(names):
                incl[t][n] += 1
            for u in a.under:
                for i, n in enumerate(names):
                    if n == u and i > 0:
                        callees[t][u][names[i - 1]] += 1
                        break
        if first:
            print("first samples taken", flush=True)
            first = False
        time.sleep(period)

    lines = []
    for t in chosen:
        n = max(1, samples[t])
        lines.append(f"\n=== thread {t}: {samples[t]} samples")
        lines.append("-- running (self) --")
        for name, c in selfc[t].most_common(25):
            lines.append(f"{c * 100 / n:5.1f}%  {name[:150]}")
        lines.append("-- on the stack (inclusive) --")
        for name, c in incl[t].most_common(45):
            lines.append(f"{c * 100 / n:5.1f}%  {name[:150]}")
        for u in a.under:
            lines.append(f"-- inside {u}: where its time goes (share of ALL samples) --")
            for name, c in callees[t][u].most_common(15):
                lines.append(f"{c * 100 / n:5.1f}%  {name[:150]}")
    text = "\n".join(lines)
    print(text)
    if a.out:
        open(a.out, "w", encoding="utf-8").write(text)


if __name__ == "__main__":
    main()
