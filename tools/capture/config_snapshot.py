#!/usr/bin/env python3
"""One-shot read of the original's BGRUPPE config table + food rate + curve."""
import os, re, subprocess, sys

WINE = os.path.expanduser("~/wine/bin/wine")
PREFIX = os.path.expanduser("~/.wine-anno")
DISPLAY = sys.argv[1] if len(sys.argv) > 1 else ":151"

def winedbg(script):
    env = dict(os.environ, DISPLAY=DISPLAY, WINEPREFIX=PREFIX, WINEDEBUG="-all")
    return subprocess.run([WINE, "winedbg"], input=script, capture_output=True,
                          text=True, env=env, timeout=60).stdout

out = winedbg("info process\nquit\n")
pid = None
for line in out.splitlines():
    if "1602.exe" in line:
        m = re.match(r"\s*([0-9a-f]{8})", line)
        if m: pid = m.group(1)
if not pid:
    print("no 1602.exe"); sys.exit(1)

script = (f"attach 0x{pid}\n"
          "x/92x 0x0061fa40\n"     # BGRUPPE 5 records x 0x48 + 2 dwords slack
          "x/4x 0x0049af2c\n"      # food rate + neighbours
          "x/64x 0x0055e780\n"     # curve entries 0..63
          "x/64x 0x0055e880\n"     # curve entries 64..127
          "x/64x 0x0055e980\n"     # curve entries 128..191
          "x/64x 0x0055ea80\n"     # curve entries 192..255
          "detach\nquit\n")
out = winedbg(script)
mem = {}
for line in out.splitlines():
    m = re.search(r"0x0*([0-9a-f]+)\s+1602\+0x[0-9a-f]+:\s+(.*)", line)
    if not m: continue
    addr = int(m.group(1), 16)
    for i, w in enumerate(re.findall(r"[0-9a-f]{8}", m.group(2))):
        mem[addr + i*4] = int(w, 16)

if not mem:
    print("read failed"); print(out[-500:]); sys.exit(1)

BASE = 0x61fa40
names = ["prozent128(+0)", "count(+4)", "proz*cnt(+8)", "maxwohn64(+c)", "steuer(+10)", "hiscore(+14)"]
wares = ["NAHRUNG(0e)", "TABAKW(0f)", "GEWUERZE(10)", "KAKAO(11)", "ALKOHOL(12)", "STOFFE(13)", "KLEIDUNG(14)", "SCHMUCK(15)"]
for t in range(5):
    rec = BASE + t*0x48
    hdr = [mem.get(rec + i*4) for i in range(6)]
    w = [mem.get(rec + 0x18 + i*4) for i in range(8)]
    print(f"tier {t}: " + "  ".join(f"{n}={v}" for n, v in zip(names, hdr)))
    print(f"        weights: " + "  ".join(f"{n}={v}" for n, v in zip(wares, w)))
print(f"food rate DAT_0049af2c = {mem.get(0x49af2c)}")
curve = {}
for i in range(0x100):
    v = mem.get(0x55e780 + i*4)
    if v is not None: curve[i] = v
keys = (0, 0x20, 0x40, 0x4c, 0x60, 0x66, 0x73, 0x80, 0x8c, 0xa6, 0xb3, 0xc0, 0xd0, 0xe0, 0xf0, 0xfe)
print("curve:", " ".join(f"[{hex(k)}]={curve[k]}" for k in keys if k in curve))
