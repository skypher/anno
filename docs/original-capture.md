# Running & driving the original 1602.exe headless

This documents how to run the shipping `extracted/1602.exe` headless under
Wine, drive its menus programmatically, and where the path to a fully
automated, instrumented run is currently blocked. It complements
`docs/lockstep.md` (the Rust side) and `tools/capture/` (the Frida harness).

Everything below runs **entirely as a normal user — no root** — except the
two blockers noted at the end, which need root on this kind of box.

## Prerequisites (userspace)

- **Wine** — a portable build works without root. The Kron4ek `amd64-wow64`
  build runs 32-bit Windows apps without i386 host libraries:
  ```sh
  curl -L -o wine.tar.xz https://github.com/Kron4ek/Wine-Builds/releases/download/11.15/wine-11.15-amd64-wow64.tar.xz
  mkdir -p ~/wine && tar -xf wine.tar.xz -C ~/wine --strip-components=1
  ```
- **Xvfb** — a headless X server (usually already present at `/usr/bin/Xvfb`).
- **ffmpeg** — for screenshots (`-f x11grab`).
- **python-xlib** — XTEST input injection: `pip install python-xlib`
  (a venv is fine; `tools/capture/xinput.py` uses it).
- **Frida** (optional, for instrumentation): `pip install frida-tools`.

## Launching the game — the two things that matter

1. **8-bit color.** The game aborts with *"SetDisplayMode failed! Please
   check if your Windows display is set on 256 colors!"* on a 16/24-bit
   screen with no virtual desktop.
2. **Wine virtual desktop.** `explorer /desktop=` makes DirectDraw render
   into a window instead of switching the real display mode — this both
   satisfies `SetDisplayMode` *and*, at **depth 24**, makes Wine emulate the
   8-bit surface in software and convert to correct true color. (At depth 8
   the palette comes out garbled through `x11grab`; depth 24 + virtual
   desktop is the clean combination.)

```sh
export WINE=~/wine/bin/wine WINEPREFIX=~/.wine-anno WINEDEBUG=-all
export WINEDLLOVERRIDES="mscoree,mshtml="   # skip mono/gecko install prompts

Xvfb :140 -screen 0 1024x768x24 -ac >/dev/null 2>&1 &
sleep 2
cd extracted
DISPLAY=:140 "$WINE" explorer /desktop=anno,1024x768 ./1602.exe &
sleep 25   # first prefix init + load is slow

# screenshot (colors are correct at depth 24):
ffmpeg -y -f x11grab -video_size 1024x768 -i :140 -frames:v 1 title.png
```

The game renders as an **800×600** window ("Anno 1602") in the top-left of
the 1024×768 virtual desktop ("anno - Wine Desktop").

> Note: `WINEDEBUG=+key`/`+msg` tracing makes the game window fail to map
> (black screen) on this setup — the trace breaks the target, so it can't be
> used to diagnose input here.

## Driving the menus

`tools/capture/xinput.py` sends clicks/keys via XTEST. Menu coordinates for
the 800×600 window (Tutorial → Explore path):

| Step | Action | Coord |
|---|---|---|
| Title | click **Singleplayer** | 215, 315 |
| Scenario tree | click **Tutorial** | 430, 316 |
| Left column | click **New Game** | 220, 305 |
| Tutorial submenu | click **Start Game** | 200, 487 |
| → "Enter your name" screen | *(blocked, see below)* | — |

```sh
python tools/capture/xinput.py :140 click 215 315 && sleep 2
python tools/capture/xinput.py :140 click 430 316 && sleep 2
python tools/capture/xinput.py :140 click 220 305 && sleep 2
python tools/capture/xinput.py :140 click 200 487 && sleep 12
```

Mouse navigation is reliable. Character typing into the name field also works
(click the field first to give it focus, then `xinput.py … key <char>`).

## Blockers (need root / deeper work)

1. **Name-entry confirm.** The "Enter your name" dialog (field defaults to
   "Anonymous") has **no OK button** — Enter is the only confirm. Synthesized
   Return does **not** confirm it through any XTEST variant (main/keypad,
   tap/hold, focused/unfocused, with/without a WM), even though Return maps
   correctly to VK_RETURN (keycode 36, not in Wine's layout-mismatch set) and
   the game reads keyboard via ordinary window messages (no DirectInput in the
   decompiled source). The likely unblock is **kernel-level input**
   (`ydotool`/uinput) — but `/dev/uinput` is root-only here — or a Wine
   message trace (which currently breaks rendering, see above).

2. **Instrumentation.** Frida injection into the wow64 Wine PE process
   crashes it ("terminated during injection"); attach-on-spawn reaches the
   Wine loader but the real PE runs in a separate child process. `winedbg`
   (Wine's native debugger) does **not** crash the game and is the viable
   non-destructive path, but needs the Wine internal PID and is clunky for
   high-frequency per-tick capture. See `tools/capture/README.md`.

Both blockers are tractable with root access (uinput input + unrestricted
Wine tracing / a true 32-bit Wine for cleaner Frida). Until then, the Rust
side of the lockstep harness is fully usable on its own (`docs/lockstep.md`).
