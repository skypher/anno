# Running & driving the original 1602.exe headless

This documents how to run the shipping `extracted/1602.exe` headless under
Wine and drive its menus with XTEST. Menu automation is userspace and
unblocked. Per-tick Frida capture is still blocked on this wow64 Wine: the
Linux Frida agent cannot inject into `wine-preloader`. It complements
`docs/lockstep.md` (the Rust side) and `tools/capture/` (the Frida harness).

Everything below runs as a normal user — no root. `ptrace_scope` is already
`0` here; the Frida failure is architectural, not a permissions problem.

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
| Name hall | click a **player colour flag** | red 140, 250 |
| → in-game tutorial | | |

```sh
# One-shot (title screen already up):
python tools/capture/xinput.py :140 start-tutorial

# Or the same clicks by hand:
python tools/capture/xinput.py :140 click 215 315 && sleep 2
python tools/capture/xinput.py :140 click 430 316 && sleep 2
python tools/capture/xinput.py :140 click 220 305 && sleep 2
python tools/capture/xinput.py :140 click 200 487 && sleep 12
python tools/capture/xinput.py :140 flag 0
```

The name hall is `gaddata/farbwahl.gad` (`FUN_0048b590` case 7). There is no
OK button. Clicking flag IDs `0x88CD`–`0x88D0` posts event `0x4E/0x39` with
the colour index and starts the scenario. Synthesized Return only commits the
name text gadget (`FUN_00409630` mode 5): it copies the buffer into
`DAT_005b6fa0` and clears `DAT_006c304c`, then the hall stays up. Clicking
the name field first is optional (default is "Anonymous") and actually
*delays* flag enable, because flags stay `Nosel` while `DAT_005b1bf4 != 0`
and that latch is cleared only when `DAT_006c304c == 0`.

Mouse navigation is reliable. Character typing into the name field works
once the field is clicked (`xinput.py … key <char>`), then `flag` to start.

Flag clicks in the 800×600 window (1024×768 GAD × 800/1024):

| Colour | Coord |
|---|---|
| 0 red | 140, 250 |
| 1 blue | 273, 357 |
| 2 white | 541, 359 |
| 3 yellow | 700, 313 |

## Frida on this Wine (still blocked)

`tools/capture/capture.py` is written for a Windows Frida session (or Frida's
Wine/`winealbin` bridge). Linux `pip install frida-tools` against this
Kron4ek `amd64-wow64` prefix does **not** instrument `1602.exe`. Verified
with Frida 17.17.0, Wine 11.15, `ptrace_scope=0`:

| Attempt | Result |
|---|---|
| `frida.attach(<wine-preloader pid>)` on `explorer.exe` / `start.exe` | `ProcessNotRespondingError: refused to load frida-agent, or terminated during injection`. The target PID is gone afterwards. |
| `frida.spawn(['./1602.exe'])` | `ExecutableNotSupportedError: unable to parse executable` — Linux spawn only understands ELF. |
| `frida.spawn([wine, './1602.exe'])` | Attach succeeds, but `Process.arch` is `x64 linux` and the module list is `wine`, `libc.so.6`, `ld-linux-x86-64.so.2`, … The 32-bit PE then starts as a **separate** `wine-preloader` child. Hooks never land on `FUN_00489670`. |

Why: `1602.exe` is PE32. This Wine has `x86_64-unix` + `i386-windows` and
**no** `i386-unix`, so the game runs as 32-bit PE code inside a 64-bit
`wine-preloader`. Linux Frida injects a 64-bit ELF `frida-agent.so` via
ptrace/`dlopen`. That is the wrong injector: wine-preloader owns the
address space, syscalls, and signals, so the agent kills the process
(same class as [frida#3339](https://github.com/frida/frida/issues/3339)).
Root would not change that. A classic multiarch 32-bit Wine is a different
process shape and still often fatal for the Linux agent.

Paths that do not crash the game:

- **`winedbg`** — Wine's own debugger talks through wineserver, not
  ptrace+`dlopen`. Viable but clunky for per-tick dumps; needs the Wine
  internal PID.
- **Windows-side Frida** — `frida-server.exe` / gadget DLL injected as a
  PE (the README's "Windows Python + Frida / winealbin" path). That is a
  different tool than Linux `frida.attach(pid)`.

Until one of those is wired into `capture.py`, the Rust side of the
lockstep harness is fully usable on its own (`docs/lockstep.md`). See
`tools/capture/README.md`.
