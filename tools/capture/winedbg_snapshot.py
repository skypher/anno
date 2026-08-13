#!/usr/bin/env python3
"""Read-only per-interval state snapshot of the running original 1602.exe.

Frida cannot instrument this wow64 Wine (see docs/original-capture.md), but
Wine's own debugger `winedbg` talks through wineserver rather than
ptrace/dlopen, so it CAN attach to the 32-bit PE and read its memory without
crashing it. This snapshots the player-data table `DAT_005b7680` (0x280 B × 7
players; the module loads at its preferred base 0x400000, no ASLR under Wine)
and prints each player's gold (record offset +4, verified live == the on-screen
50000 at tutorial start) and name (+8).

IMPORTANT: attach + read only. Do NOT set a code breakpoint — an INT3 at e.g.
FUN_00489670 page-faults in wow64 32-bit code, which triggers Wine's *auto*
`winedbg --auto` crash handler; that handler then holds the debug attachment
(subsequent attaches fail with "error 5"), and killing it takes the game down.
So this tool takes interval snapshots (attach → read → detach → repeat), which
is enough to compare the original's deterministic economy/population evolution
against a headless Rust run (`crates/anno-game/src/bin/headless.rs`). It is NOT
per-tick and cannot pin the RNG seed (that needs code injection, which is
blocked here), so RNG-dependent state will diverge — compare the deterministic
early-game economy.

Usage:
    python winedbg_snapshot.py :150               # one snapshot
    python winedbg_snapshot.py :150 --loop 8 --every 5   # 8 snapshots, 5s apart

Env: WINE (default ~/wine/bin/wine), WINEPREFIX (default ~/.wine-anno).
"""
import os
import re
import subprocess
import sys
import time

PLAYER_TABLE = 0x005B7680
# BYTE stride is 0x280: the binary indexes `(int*)&DAT_005b7680 + slot*0xa0`,
# i.e. 0xa0*4 bytes. (Slot 0 reads correctly at any stride since it is at
# offset 0; slots 1..6 need 0x280 — the trader's 1M gold write _DAT_005b8084
# is at 005b7680 + 4*0x280 + 4.)
PLAYER_STRIDE = 0x280
PLAYER_COUNT = 7
GOLD_OFFSET = 0x04
WINE = os.environ.get("WINE", os.path.expanduser("~/wine/bin/wine"))
PREFIX = os.environ.get("WINEPREFIX", os.path.expanduser("~/.wine-anno"))


def winedbg(display, script):
    env = dict(os.environ, DISPLAY=display, WINEPREFIX=PREFIX, WINEDEBUG="-all")
    return subprocess.run(
        [WINE, "winedbg"], input=script, capture_output=True, text=True,
        env=env, timeout=45,
    ).stdout


def find_pid(display):
    out = winedbg(display, "info process\nquit\n")
    for line in out.splitlines():
        if "1602.exe" in line:
            m = re.match(r"\s*([0-9a-f]{8})", line)
            if m:
                return m.group(1)
    return None


def snapshot(display):
    pid = find_pid(display)
    if not pid:
        return None
    words = PLAYER_COUNT * PLAYER_STRIDE // 4
    out = winedbg(
        display,
        f"attach 0x{pid}\ninfo process\nx/{words}x 0x{PLAYER_TABLE:08x}\ndetach\nquit\n",
    )
    # parse "1602+0x1b76..:  w0 w1 w2 w3" lines into a flat dword list keyed by addr
    mem = {}
    for line in out.splitlines():
        m = re.search(r"0x0*([0-9a-f]+)\s+1602\+0x[0-9a-f]+:\s+(.*)", line)
        if not m:
            continue
        addr = int(m.group(1), 16)
        for i, w in enumerate(re.findall(r"[0-9a-f]{8}", m.group(2))):
            mem[addr + i * 4] = int(w, 16)
    if not mem:
        return None
    players = []
    for p in range(PLAYER_COUNT):
        base = PLAYER_TABLE + p * PLAYER_STRIDE
        gold = mem.get(base + GOLD_OFFSET)
        state = mem.get(base)
        players.append({"slot": p, "state": state, "gold": gold})
    return players


def main():
    display = sys.argv[1] if len(sys.argv) > 1 else ":150"
    loops = 1
    every = 5.0
    if "--loop" in sys.argv:
        loops = int(sys.argv[sys.argv.index("--loop") + 1])
    if "--every" in sys.argv:
        every = float(sys.argv[sys.argv.index("--every") + 1])
    for n in range(loops):
        players = snapshot(display)
        if players is None:
            print(f"[{n}] 1602.exe not attachable")
        else:
            active = [f"p{p['slot']}={p['gold']}" for p in players
                      if p["gold"] not in (None, 0)]
            print(f"[{n}] gold: {'  '.join(active) or '(none)'}")
        if n + 1 < loops:
            time.sleep(every)


if __name__ == "__main__":
    main()
