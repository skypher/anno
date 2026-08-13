# Original-binary capture harness

Instruments the shipping `1602.exe` with [Frida](https://frida.re) so its
per-tick simulation state can be recorded and diffed against the Rust
reimplementation. This is the "original side" of the lockstep-comparison
workflow described in `docs/lockstep.md`.

## What it does

- **Pins the RNG seed** (`srand` / `GetTickCount` hooks) so a capture is
  reproducible and lines up with a headless Rust run seeded identically.
  Both sides use the MSVC CRT LCG (`crates/anno-sim/src/source_rand.rs`).
- **Dumps state each sim tick** by hooking the simulation dispatcher
  `FUN_00489670`, emitting one JSON line per dump interval in the **same
  schema** the headless Rust driver emits.
- **Records the scheduler trace** — which of the twelve subsystem tickers
  inside `FUN_00489670` fired each tick — for `anno_sim::fidelity`
  comparison.

## Requirements

- The copyrighted `1602.exe` (not in this repo).
- **Windows Frida**, or Linux + Wine with a **Windows** Python + Frida, or
  Frida's Wine/`winealbin` gadget (a PE agent). Linux `frida.attach` on
  this wow64 Wine kills `wine-preloader` — see below.
- `pip install frida-tools` (on the Windows side, or for the winealbin
  bridge).

Menu driving under Wine+Xvfb does **not** need Frida:
`python tools/capture/xinput.py :140 start-tutorial`. That path is
documented in `docs/original-capture.md`.

## Usage

```sh
# Launch the game instrumented, capture 3000 ticks at seed 1:
python capture.py --exe 'C:/Anno1602/1602.exe' --seed 1 \
    --ticks 3000 --dump-every 10 --out original.jsonl

# Produce the matching Rust-side dump:
cargo run -p anno-game --bin headless -- \
    --scenario extracted/szenes/game00.szs --seed 1 \
    --ticks 3000 --dump-every 10 --out rust.jsonl

# Diff them:
cargo run -p anno-sim --example compare -- rust.jsonl original.jsonl
```

## Address map

`capture.js` uses the absolute PE addresses from
`docs/re-notes/architecture.md`, assuming the default image base
`0x400000`, and rebases them onto the loaded module (ASLR-safe). The key
hooks:

| Purpose | Address |
|---|---|
| Simulation dispatcher (one call = one tick) | `FUN_00489670` |
| Production ticker | `FUN_0047daf0` |
| Population/economy ticker | `FUN_0047f8a0` |
| Ship ticker | `FUN_004791b0` |
| Diplomacy ticker | `FUN_00476350` |
| AI controller | `FUN_0042b4b0` |
| Player data table | `DAT_005b7680` (160 B × 7) |
| Island data table | `DAT_005e6b20` (2816 B × 50) |
| Active buildings | `PTR_DAT_0049aebc` (20 B × 1037) |

## Calibrating field offsets

The **within-record field offsets** (player gold, per-tier population) in
`capture.js` (`PLAYER.goldOffset`, `PLAYER.populationOffset`) are
**placeholders**. The record *base* addresses and strides are documented,
but the exact byte offset of each field inside a 160-byte player record is
not, and must be confirmed before the economy comparison is meaningful:

1. Load a savegame with a known, distinctive gold value (e.g. 12345).
2. In Frida, scan the player record for that value:
   `Process.getModuleByName('1602.exe').base.add(0x1b7680)` (rebased
   `DAT_005b7680`), read 160 bytes, find the u32 == 12345 → that offset is
   `goldOffset`.
3. Repeat for a distinctive population and set `populationOffset`.
4. Pass them at runtime via the `setPlayerLayout` rpc, or edit the `PLAYER`
   table in `capture.js`.

The **RNG word** (`holdrand`) is CRT-build-specific and not in the address
map. If you locate it (scan for the LCG state after a known number of
`rand()` calls), pass `--holdrand 0x...`; otherwise the RNG column is
reported as `-1` and only tick-count and scheduler-trace comparison apply.

## Linux Frida + wow64 Wine

`capture.py` calls `frida.spawn` / `frida.attach` as a **Linux** session.
That does not reach the 32-bit PE on the Kron4ek `amd64-wow64` Wine used
in `docs/original-capture.md` (Wine 11.15, Frida 17.17.0, `ptrace_scope=0`):

- Attach to a live `wine-preloader` (`explorer.exe`, `start.exe`,
  `1602.exe`) raises `ProcessNotRespondingError` ("terminated during
  injection") and **kills** the target. wine-preloader is not a normal
  ELF process; ptrace+`dlopen` of 64-bit `frida-agent.so` is fatal
  ([frida#3339](https://github.com/frida/frida/issues/3339)).
- `frida.spawn(['1602.exe'])` fails: Linux Frida cannot parse a PE
  (`ExecutableNotSupportedError`).
- `frida.spawn([wine, '1602.exe'])` attaches to the Unix `wine` stub
  (`Process.arch == x64 linux`, modules are `wine`/`libc`). The real PE
  is a later `wine-preloader` child, so `FUN_00489670` is never hooked.

This Wine has `x86_64-unix` + `i386-windows` and no `i386-unix`. Root
does not fix the injector. Use a Windows Frida / winealbin gadget, or
`winedbg` through wineserver. Menu automation is independent and already
works via `xinput.py`.

## Trust boundary

None of this is exercisable in CI — there is no binary here. Treat the
offsets as the primary risk. Validate the very first dump line against a
hand-checked savegame before trusting a multi-thousand-tick capture.
