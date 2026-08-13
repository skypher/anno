#!/usr/bin/env python3
"""Read the original's runtime city records: owner, pops, ware stocks."""
import os, re, subprocess, sys

WINE = os.path.expanduser("~/wine/bin/wine")
PREFIX = os.path.expanduser("~/.wine-anno")
DISPLAY = sys.argv[1] if len(sys.argv) > 1 else ":151"

CITY_TABLE = 0x005DBAE0
CITY_SIZE = 600
CITY_SLOTS = 0x4B

WARES = {0x0e: "NAHRUNG", 0x0f: "TABAKW", 0x10: "GEWUERZE", 0x11: "KAKAO",
         0x12: "ALKOHOL", 0x13: "STOFFE", 0x14: "KLEIDUNG", 0x15: "SCHMUCK",
         0x16: "WERKZEUG", 0x17: "HOLZ", 0x18: "ZIEGEL"}

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

# read first 12 city slots (7200 bytes = 1800 dwords) + clock
words = 12 * CITY_SIZE // 4
script = (f"attach 0x{pid}\n"
          f"x/{words}x 0x{CITY_TABLE:08x}\n"
          "x/1x 0x005b6040\n"
          "detach\nquit\n")
out = winedbg(script)
mem = {}
for line in out.splitlines():
    m = re.search(r"0x0*([0-9a-f]+)\s+1602\+0x[0-9a-f]+:\s+(.*)", line)
    if not m: continue
    addr = int(m.group(1), 16)
    for i, w in enumerate(re.findall(r"[0-9a-f]{8}", m.group(2))):
        mem[addr + i*4] = int(w, 16)

def u8(a): return (mem.get(a & ~3, 0) >> ((a & 3) * 8)) & 0xFF
def u16(a):
    if a & 3 == 3:  # crosses dword boundary
        return u8(a) | (u8(a+1) << 8)
    return (mem.get(a & ~3, 0) >> ((a & 3) * 8)) & 0xFFFF
def u32(a): return mem.get(a, 0)

if not mem:
    print("read failed"); print(out[-400:]); sys.exit(1)

print(f"clock: {u32(0x5b6040)}")
for slot in range(12):
    base = CITY_TABLE + slot * CITY_SIZE
    island = u8(base + 0x18)
    if island == 0xFF:
        continue
    owner = u8(base + 0x1a)
    pops = [u32(base + 0x220 + t*4) for t in range(5)]
    sats = [u8(base + 0x248 + t) for t in range(5)]
    taxes = [u8(base + 0x24d + t) for t in range(5)]
    print(f"city[{slot}] island={island} owner={owner} pop={pops} sat={sats} tax={taxes}")
    stocks = []
    for w, name in WARES.items():
        rec = base + 0x24 + w * 0x0c
        stocks.append(f"{name}={u16(rec+6)}/r{u16(rec)}")
    print("   wares: " + " ".join(stocks))
    dem = [u32(base + 0x150 + s*0xc) for s in range(8)]
    sup = [u32(base + 0x154 + s*0xc) for s in range(8)]
    ful = [u8(base + 0x158 + s*0xc) for s in range(8)]
    print(f"   demand={dem}")
    print(f"   supply={sup}")
    print(f"   fulfil={ful}")
