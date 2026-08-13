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

## winedbg state capture (works, read-only)

`winedbg` **can** read the running game's live memory without crashing it —
proven end to end. `tools/capture/winedbg_snapshot.py` drives it. The module
loads at its preferred base `0x400000` (no ASLR under Wine), so decompiled
addresses are usable directly.

```sh
# with the game already in-game (tools/capture/xinput.py start-tutorial + a flag click):
WINE=~/wine/bin/wine python tools/capture/winedbg_snapshot.py :150
```

Verified live at tutorial start: the player-data table `DAT_005b7680`
(160 B × 7) reads back player 0's **gold at +4 = `0xc350` = 50000** (matches
the on-screen value and the economy audit's RE) and its name at +8 =
`"Anonymous"`. So the whole capture chain works: headless launch → XTEST
menu drive → running sim → winedbg snapshot of the original's real state,
comparable against `crates/anno-game/src/bin/headless.rs`.

Two constraints, both real:

1. **No code breakpoints.** An INT3 at e.g. `FUN_00489670` page-faults in the
   wow64 32-bit code, which trips Wine's *auto* crash handler
   (`winedbg --auto <pid>`). That handler then holds the debug attachment
   (further attaches fail with "error 5"), and killing it takes the game
   down. So this is **attach → read → detach → repeat** (interval snapshots),
   not per-tick. Any fault in the game itself spawns the same blocking
   auto-debugger; disabling Wine's `AeDebug` handler is the next reliability
   step for long snapshot loops.
2. **No RNG seed pinning.** That needs code injection (blocked here), so
   RNG-dependent state diverges from a headless Rust run. Compare the
   deterministic early-game economy/population instead, where both sides
   evolve from the same fixed scenario start.

### First cross-engine agreement

Running the Rust headless on the *same* scenario the original's tutorial loads:

```sh
cargo run -q -p anno-game --bin headless -- \
    --scenario "extracted/Szenes/Tutorial0.szs" --ticks 30 --dump-every 10
```

reports player 0 **gold = 50000** at tick 0 — identical to the value winedbg
read out of the original's live `DAT_005b7680+4`. Both engines start from the
same economy state. The gold then stays flat through tick 30 (no buildings, no
population yet), which confirms the deterministic early-game economy is quiet
until something is built — so a *discriminating* dynamic comparison needs a
scenario that starts populated (below), not the empty tutorial.
(Full RNG-word parity is a separate, longer road — see
`docs/rng-dispatch-order.md`.)

### Populated-scenario economy comparison (proven end to end)

Empty scenarios don't move the economy. Many **mission scenarios start with a
fully-built, populated city**, so their economy evolves immediately from a
fixed start — no building automation needed. Scan for them:

```sh
./target/debug/headless --scenario "extracted/Szenes/<name>.szs" \
    --ticks 1 --dump-every 1     # look for buildings/population > 0 at tick 0
```

Driving the original into one (menu path, all XTEST via `xinput.py`):

1. Title → **Singleplayer** (215,315).
2. Scenario tree → pick a standalone **Additional Scenario** (e.g. *Exile* at
   ~430,548) — these avoid the campaign briefing chains.
3. The **Assignment** briefing appears → **Start Game** (200,487).
4. The FARBWAHL name hall appears → focus the window and press **Return** to
   commit the name (this clears the latch that keeps the colour flags
   `Nosel`), then click a flag. Return must have window focus:
   `xinput.py :151 wkey a00001 Return` then `xinput.py :151 click 130 320`.

Then read the economy each interval with `econ_snapshot.py` (clock + 7-slot
gold) and diff by clock against the headless run of the same `.szs`.

**Reliability:** disable Wine's auto crash-handler **before launching**, or a
fault spawns `winedbg --auto <pid>` which wedges every later attach (and
killing it kills the game):

```sh
"$WINE" reg add 'HKLM\Software\Microsoft\Windows NT\CurrentVersion\AeDebug' \
    /v Auto /t REG_SZ /d 0 /f
```

Even so, treat each attach as one-shot; relaunch if a read returns
`not attachable`.

#### First populated-scenario result (Exile)

> The player record byte stride is **0x280**, not 0xA0: the binary indexes
> `(int*)&DAT_005b7680 + slot*0xa0`, so the byte stride is `0xa0*4`. The
> trader's `1000000` gold write lands at `005b7680 + 4*0x280 + 4`. Slot 0 reads
> right at any stride (offset 0); slots 1..6 need 0x280.

Original read at clock 255 (correct stride), vs the same scenario run headless:

| slot | state | scenario gold | Rust headless | original @clock 255 |
|--|--|--|--|--|
| 0 human | 0x00 | 10000 | 10000 | **10008** ✓ |
| 1 AI | 0x0c | 10000 | 10000 | 6831 (AI spending) |
| 2 AI | 0x0c | 10000 | 10000 | **10000** ✓ |
| 3 AI | 0x0c | 10000 | 10000 | 6115 (AI spending) |
| 4 trader | 0x0d | 1000000 | 1000000 | **1000000** ✓ |
| 5 native | 0x0e | 50000 | 50000 | **50000** ✓ |
| 6 pirate | 0x0b | 5000 | 5000 | **5000** ✓ |

- **Scenario loading + fixed-faction economy are faithful.** The trader (1M),
  native (50k), pirate (5k) match **exactly**, and the runtime state bytes
  match the scenario faction types. The human matches (10008 vs 10000; the
  on-screen counter agreed, confirming the read).
- **The only divergence is the AI players** (slots 1, 3): the original's AI is
  actively building and paying upkeep (gold already drawn down to 6831 / 6115
  by clock 255), while the Rust AI controller is an acknowledged
  work-in-progress and its decisions are RNG-gated, so its spending differs.
  This is expected, not a scenario/economy bug.

The initial wrong-stride read (which showed non-human slots at 0 / the trader
at a spurious 10979) was a reader bug, not an engine difference — corrected in
`econ_snapshot.py` / `winedbg_snapshot.py`.
