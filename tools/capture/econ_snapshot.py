#!/usr/bin/env python3
"""Read the original 1602.exe's live economy for cross-engine comparison.

One-shot winedbg read (attach read-only, no breakpoints) of:
  - the game clock DAT_005b6040 (source_time_ticks), and
  - per-player gold DAT_005b7680 + 0xA0*slot + 4 (7 slots).

Prints `clock=<n>  gold=[...]` so the same scenario run headless
(`crates/anno-game/src/bin/headless.rs`) can be diffed by clock value. See
docs/original-capture.md for the capture recipe and the known constraints
(interval snapshots only; disable Wine AeDebug so a fault can't wedge the
attach; RNG seed cannot be pinned so compare the deterministic economy).

Usage:
    WINE=~/wine/bin/wine python econ_snapshot.py :151
Env: WINE (default ~/wine/bin/wine), WINEPREFIX (default ~/.wine-anno).
"""
import os
import re
import subprocess
import sys

DISPLAY = sys.argv[1] if len(sys.argv) > 1 else ":151"
WINE = os.environ.get("WINE", os.path.expanduser("~/wine/bin/wine"))
PREFIX = os.environ.get("WINEPREFIX", os.path.expanduser("~/.wine-anno"))
CLOCK = 0x005B6040
PLAYER_TABLE = 0x005B7680
STRIDE = 0xA0
COUNT = 7
GOLD_OFF = 4


def winedbg(script):
    env = dict(os.environ, DISPLAY=DISPLAY, WINEPREFIX=PREFIX, WINEDEBUG="-all")
    return subprocess.run(
        [WINE, "winedbg"], input=script, capture_output=True, text=True,
        env=env, timeout=45,
    ).stdout


def find_pid():
    for line in winedbg("info process\nquit\n").splitlines():
        if "1602.exe" in line:
            m = re.match(r"\s*([0-9a-f]{8})", line)
            if m:
                return m.group(1)
    return None


def parse_words(out):
    mem = {}
    for line in out.splitlines():
        m = re.search(r"0x0*([0-9a-f]+)\s+1602\+0x[0-9a-f]+:\s+(.*)", line)
        if not m:
            continue
        addr = int(m.group(1), 16)
        for i, w in enumerate(re.findall(r"[0-9a-f]{8}", m.group(2))):
            mem[addr + i * 4] = int(w, 16)
    return mem


def sgold(g):
    if g is None:
        return None
    return g - (1 << 32) if g >= (1 << 31) else g


def main():
    pid = find_pid()
    if not pid:
        print("1602.exe not attachable")
        return 1
    words = COUNT * STRIDE // 4
    out = winedbg(
        f"attach 0x{pid}\nx/4x 0x{CLOCK:08x}\n"
        f"x/{words}x 0x{PLAYER_TABLE:08x}\ndetach\nquit\n"
    )
    mem = parse_words(out)
    clock = mem.get(CLOCK)
    golds = [sgold(mem.get(PLAYER_TABLE + p * STRIDE + GOLD_OFF)) for p in range(COUNT)]
    print(f"clock={clock}  gold={golds}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
