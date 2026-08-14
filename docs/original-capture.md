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
- **AI players diverge** (slots 1, 3): the original's AI is actively building
  and paying upkeep (gold already drawn down to 6831 / 6115 by clock 255),
  while the Rust AI controller is an acknowledged work-in-progress and its
  decisions are RNG-gated. Expected.

The initial wrong-stride read (which showed non-human slots at 0 / the trader
at a spurious 10979) was a reader bug, not an engine difference — corrected in
`econ_snapshot.py` / `winedbg_snapshot.py`.

#### Confirmed finding: the human economy *trajectory* diverges

The **initial** human economy matches, but the trajectory does not:

| clock | original human gold | Rust headless human gold |
|--|--|--|
| 0 | 10000 | 10000 |
| 255 | **10008** | ~9850 (interp.) |
| 640 | **10024** | (declining) |

The original's human city is net-flat/slightly-positive (two snapshots, both
~10000+), while the Rust city *was* running a **deficit** and declining (10000
→ 9928 → 9855 → 9827 over clocks 0–300).

**Root cause (found by instrumented diagnosis, FIXED):** the Rust charged
building **maintenance on natural terrain**. The invented per-type maintenance
table gave forests `WALD → Wood → 5` and ore rocks `FELS → Ore → 60`, so
player 0's 66 forest tiles + 3 ore rocks alone contributed 510 of 667
maintenance/tick — a net −72/tick regardless of population. The original
charges operating cost only for *constructed* buildings (instance flag `0x10`
in `FUN_0047f6b0`/`FUN_00463140` reading `def+0x2a`); terrain has no such flag.
Gating maintenance on `category != 0` (terrain/nature) in `data_bridge.rs`
removes the invented charge. After the fix the Rust human gold reads
`10000 → 10014 → 10026 → 10052` over clocks 0–300 — flat/positive, matching the
original's `10008 @255` / `10024 @640` in both sign and magnitude.

(My first hypothesis — that the `population.rs` consumption/satisfaction
approximation drove emigration — was **refuted** by experiment: forcing
satisfaction to full still bled gold, while excluding terrain maintenance fixed
it even with population unchanged. Population *does* still decline in Exile — a
separate production→warehouse food-delivery gap — but that is RNG-gated and does
not drive the gold trajectory.)

### Static config tables read live (no scenario needed)

The `.cod`-compiled tables populate during startup, so a winedbg read **at the
title screen** recovers them bit-exactly — no menu driving, no timing. Read
2026-08-14 (`BGRUPPE` table at `DAT_0061fa40`, 0x48-byte records ×5):

| field | values | meaning |
|--|--|--|
| `+0x00` | 0, 60, 90, 99, 107 | `Prozent × 128/100` fulfillment target |
| `+0x04` | 0, 2, 4, 5, 6 | demanded-ware count (post-pass `FUN_00462d50`) |
| `+0x08` | 0, 120, 360, 495, 642 | target × count — `FUN_0047f3b0` scale |
| `+0x0c` | 128, 384, 960, 1600, 2560 | `Maxwohn << 6` |
| `+0x10` | 7, 8, 11, 12, 13 | `Steuer` per-capita income (confirms Rust) |
| `+0x18..` | see below | 8 ware demand weights (NAHRUNG..SCHMUCK) |

Ware weights (`Ware:` floats stored ×8192 by the parser, `/600` at load; so
`0.2/0.5/0.6/0.7/0.8 → 2/6/8/9/10`): settler `[ALKOHOL 6, STOFFE 8]`; citizen
`[TABAKW 6, GEWUERZE 6, ALKOHOL 8, STOFFE 9]`; merchant `[8, 8, KAKAO 9, 9,
10]`; aristocrat `[8, 8, 8, ALKOHOL 10, KLEIDUNG 6, SCHMUCK 2]`. The global
food rate `DAT_0049af2c = ftol(1.3 × 8192)/600 = 17` per resident per cycle —
at the 15/16-decay equilibrium that pulls 1.3 t per 100 residents per minute,
matching the cod comment "Verbrauch je 100 Einwohner". The satisfaction curve
`DAT_0055e780` matched the Rust port at every sampled entry (including
interpolated ones: `[0x20]=81`, `[0x40]=86`, `[0x60]=99`, `[0xd0]=426`).

### Exact consumption engine: bit-exact cross-engine agreement

With the `FUN_0047f8a0` demand cycle ported
(`SourceCityRecord::source_ware_economy_cycle`), the original's live Exile city
records (read via winedbg at clock 0, one cycle in) match the Rust engine's
hand-computed accumulators **bit-exactly**: the human city (156 settlers) shows
`demand = [2486, 0, 0, 0, 877, 1170, 0, 0]` = `⌊17·156·15/16⌋ /
⌊6·156·15/16⌋ / ⌊8·156·15/16⌋` in exactly the ported slot layout
(NAHRUNG/ALKOHOL/STOFFE), supplies 0, fulfillment bytes 0x80, tax bytes 0x80.

### Remaining divergence: initial city stores (KONTOR2)

The original's Exile cities **start stocked**: the human city holds NAHRUNG
800, TABAKW 800, ALKOHOL 509, STOFFE 509, WERKZEUG 1127, HOLZ 982, ZIEGEL 982
(1/32-t units) at clock 0, while the Rust loader creates every Kontor empty —
so the exact engine correctly starves the Rust city (settler satisfaction 0)
and the population approximation drains it. The authored stocks live in the
scenario's `KONTOR2` chunks (1004 B = 4-byte header + 50 × 0x14 records;
loader at `0x484230`, in a decompiled-dump gap, recovered by disassembly):
record `+0x0c` u16 = initial stock, `+0x10` u16 = def number (+20000) whose
def byte `+0x21` selects the ware slot, `+0x00`/`+0x08` merge into the runtime
ware entry's trade-slider fields. `cargo run -p anno-formats --example
audit_kontor2_bytes <szs>` dumps them (Exile: 800/800/509/509/1127 confirmed
authored). **Now parsed and seeded** (`SzsFile::kontors` →
`Warehouse::seed_city_stock_fixed` in the scenario loader): the Rust t0 city
store matches the original bit-exactly, settler satisfaction holds at 128, and
the food drain follows the exact 1.3 t/min/100-residents rate.

With consumption exact and stores seeded, the dominant remaining divergence
was the **population growth approximation**: `update_population_growth` grew a
satisfied tier ~5 %/economy-tick and the house-tier gate upgraded every
satisfied house each tick, so the Rust Exile population exploded (156 → 844
settlers+ in 400 s) where the original is bounded by houses (26 × Maxwohn 6 =
exactly the starting 156).

**Fixed via SIEDLER seeding:** the authored residences ship in the per-island
`SIEDLER` chunks (16-byte records; loader `0x483ee0` / saver both in dump
gaps, recovered by disassembly — see `SzsFile::settler_houses` for the full
bit map). Exile island 0 carries 26 houses × amount `0x180` (= `Maxwohn <<
6`, 156 residents, matching STADT4). Seeding them into the kind-13 location
table wires the already-ported transfer machinery (`FUN_0047b410` deltas from
city satisfaction, capacity-clamped increase, promotion reservations): the
Rust Exile human population now holds **exactly 156** indefinitely while the
stocked store drains at the exact consumption rate. Mirrored owners take
`player.population` from the city records (as `FUN_0047f740` reads `+0x220`)
and both invented growth approximations stay off for them.

Remaining smaller divergence: the gold slope (Rust ≈ +0.3/s vs the original's
≈ +0.03/s at clock 255/640) suggests the building-maintenance side still
undercounts operating costs; and the city *event block* (rand-gated
`FUN_0047b540` immigration enqueue, `FUN_0047f510` house upgrades, the
`+0x1e0` re-arm) is not yet ported — it only matters once houses are below
capacity or satisfaction moves, which the current comparisons don't exercise.

### Promotion pipeline agreement (post-SIEDLER fixes)

Two follow-up fixes brought the Rust kind-13 pipeline into the same
intermediate state the live original shows at +150 s:

- **AI-city satisfaction defaults**: every live city (human, AI, trader,
  native, pirate) reads `0x80` per group at clock 0; the Rust default was
  zero, so the kind-13 decay path immediately drained and downgraded every
  AI city. `SourceCityRecord` now initializes satisfied.
- **Replacement commands are sim-owned**: `FUN_0047c080`'s map replacement is
  synchronous in the source; the Rust queued it for the game frontend only,
  so headless runs left the map def stale while city/location tables had
  already changed group. `Simulation::drain_source_kind13_replacements`
  applies the static half everywhere; the game frontend keeps only its
  renderer-overlay patch.

With those, the Rust human city develops `promotion_reservations[2] = 6`
(one settler house reserved toward citizens) and holds — matching the
original's observed reservation-driven TABAKWAREN/GEWUERZE demand at
constant `[0,156,0,0,0]`.

### The FUN_00482120 house-coverage scan (ported)

The infrastructure lifecycle bits are set by a per-island rescan
(`FUN_00482120`, dirty-flag driven) that walks the residence list against
the island's coverage buildings, matched by production kind code:
marketplace (7) sets state bit `0x80` and takes the **minimum distance
class** into the variant nibble (feeding the `FUN_0047b410` growth curves
— houses near a market grow faster); Kontor (8) sets `0x0400`; tavern
`0x0010`, chapel `0x0004`, church `0x0008`, bathhouse `0x0040`, theater
`0x0080`, doctor `0x0200`, school `0x0020`, college `0x0100`, gallows
`0x0800`, well `0x1000`. The radius test is `dx ≤ row[dy]` over the
compiled circle rows (`FUN_00404d70` integer midpoint fill, live-verified)
measured from the building's centered footprint with even-size asymmetry;
the distance class is `trunc(sqrt(dx²+dy²)·0.375 + 0.5)` (rdata constants
`0x496458`/`0x496310`, grid live-verified). State bit 6 is re-evaluated
from the `FUN_0047bfa0` transition predicate afterwards.

Validation: the scan reproduces Exile's authored SIEDLER flags **exactly
for all 26 houses** (state, lifecycle, and every distance-class variant) —
the authored records are frozen output of the same scan (they predate the
scenario's well, whose `0x1000` bit the runtime adds back). The fit also
exposed that the compiled service radii are `RADIUS_MARKT = RADIUS_HQ =
0x10` (`1602_exe.c:66467-66468`) — the previously recovered 30/22 were
wrong, and the warehouse-coverage radius has been corrected to 16.

Exile's settlers still cannot mature to citizens — the scenario has no
school or college — so the promotion reservation pends indefinitely,
matching the live original. The remaining growth-chain gaps are the
rand-gated event block itself (`FUN_0047b540` immigration enqueue and the
`FUN_0047a020` pending-wave processor, which matter once houses sit below
capacity) and the `FUN_0047f510` marketplace-upgrade scan.

Wine stability note: the original dies within ~5 minutes of entering Exile on
this setup even with zero winedbg attaches (reproduced three times; the crash
log shows `wine: Unhandled page fault on read access to FFF210EC` — a wild
computed pointer, timing/RNG dependent, likely aggravated by the absent ALSA
device). Trajectory captures should snapshot early and often.

#### Live trajectory point (+150 s into Exile)

One clean mid-run read before the Wine crash (2026-08-14): human city
population still exactly `[0, 156, 0, 0, 0]` — **flat, house-bounded**,
confirming the SIEDLER-seeded Rust behavior. Store drains inside the exact
consumption band (NAHRUNG 800 → 727, STOFFE 509 → 444), production deposits
run (HOLZ 982 → 1142, ZIEGEL 982 → 1110), and — notably — the original is
already **upgrading houses**: demand slots for TABAKWAREN/GEWUERZE light up at
315 via promotion reservations (GEWUERZE starving at fulfillment 0 since the
store has none). So the `FUN_0047f510`/event-block house-upgrade path shifts
the tier mix (and tax income) even at constant total population — it is the
next port target, not merely a below-capacity concern. Also learned:
`DAT_005b6040` read 0 throughout while cycles clearly ran — it is not a
wall-time game clock; use host timestamps as the time axis for captures.

Caveat for long captures: each winedbg attach is one-shot-ish — after a few
attach/detach cycles this Wine build sometimes takes the game down. Space
snapshots minutes apart and expect to relaunch between measurement campaigns.
