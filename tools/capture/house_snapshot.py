#!/usr/bin/env python3
"""Read the runtime kind-13 house list DAT_005a77e8 (10-byte records)."""
import os, re, subprocess, sys

WINE = os.path.expanduser("~/wine/bin/wine")
PREFIX = os.path.expanduser("~/.wine-anno")
DISPLAY = sys.argv[1] if len(sys.argv) > 1 else ":151"

def winedbg(script):
    env = dict(os.environ, DISPLAY=DISPLAY, WINEPREFIX=PREFIX, WINEDEBUG="-all")
    return subprocess.run([WINE, "winedbg"], input=script, capture_output=True,
                          text=True, env=env, timeout=90).stdout

out = winedbg("info process\nquit\n")
pid = None
for line in out.splitlines():
    if "1602.exe" in line:
        m = re.match(r"\s*([0-9a-f]{8})", line)
        if m: pid = m.group(1)
if not pid:
    print("no 1602.exe"); sys.exit(1)

# 120 records x 10 bytes = 1200 bytes = 300 dwords
script = (f"attach 0x{pid}\nx/300x 0x005a77e8\ndetach\nquit\n")
out = winedbg(script)
mem = {}
for line in out.splitlines():
    m = re.search(r"0x0*([0-9a-f]+)\s+1602\+0x[0-9a-f]+:\s+(.*)", line)
    if not m: continue
    addr = int(m.group(1), 16)
    for i, w in enumerate(re.findall(r"[0-9a-f]{8}", m.group(2))):
        mem[addr + i*4] = int(w, 16)
if not mem:
    print("read failed"); sys.exit(1)

def u8(a): return (mem.get(a & ~3, 0) >> ((a & 3) * 8)) & 0xFF
def u16(a): return u8(a) | (u8(a+1) << 8)

BASE = 0x5a77e8
for n in range(120):
    r = BASE + n * 10
    island = u8(r)
    if island == 0xFF:
        continue
    print(f"[{n:3}] isl={island} x={u8(r+1):2} y={u8(r+2):2} r3={u8(r+3):#04x} "
          f"r4={u8(r+4):#04x} group={u8(r+5)} amount={u16(r+6):#06x} flags={u16(r+8):#06x}")
