# Lockstep comparison: Rust engine vs original 1602.exe

This is the harness for verifying that the Rust reimplementation reproduces
the original game's behaviour, by running both from the same starting state
with the same inputs and diffing their state tick by tick.

Two engines cannot be aligned frame-for-frame — the Rust game loop is
variable-timestep and render-coupled, the original is a Win32 message pump.
Alignment is at **sim-tick granularity**: both are driven with a fixed
synthetic timestep and a pinned RNG seed, and their per-tick state is
compared. Integer and fixed-point fields (the economy) are held to exact
equality; float fields (entity positions) drift because the 1996 build uses
80-bit x87 intermediates and Rust uses IEEE-754 f32, so those are compared
within a tolerance band.

## The pieces

| Component | Path | Role |
|---|---|---|
| Headless driver | `crates/anno-game/src/bin/headless.rs` | Runs the real scenario sim with a fixed dt + pinned seed, emits per-tick JSONL. |
| Command recorder | `crates/anno-sim/src/replay.rs` + `game --record` | Captures a play session's commands so it can be replayed deterministically. |
| Capture harness | `tools/capture/` | Frida-instruments `1602.exe`: pins the seed, dumps per-tick state in the same JSONL schema, records the scheduler trace. |
| Comparison tool | `tools/compare/compare.py` | Diffs the two JSONL dumps: exact for ints, tolerance for floats, set-compare for the scheduler trace. |
| Scheduler fidelity | `anno_sim::fidelity` | `TickScheduler` produces the subsystem-fired trace; `compare_trace` validates it. |

## What makes it deterministic

- **One RNG.** `anno_sim::source_rand::SourceRand` is a bit-exact MSVC CRT
  `rand()` LCG. `Simulation::seed_source_rand(seed)` pins it; the capture
  harness pins the original's `srand` seed to the same value.
- **Phase-exact save/load.** `SaveState` (v138+) persists the subsystem
  timer accumulators and the fractional clock, so a run resumed from a
  snapshot fires every subsystem on the same tick as an uninterrupted run.
  This is what makes recorded-command replay retrace the original run.
- **Fixed timestep.** The headless driver calls `sim.tick(dt)` with a
  constant `dt`, so there is no wall-clock nondeterminism.
- **State hash.** `Simulation::state_hash()` (FNV-1a over the bincode
  snapshot) gives a single 64-bit digest per tick for a fast twin check.

## Running it

```sh
# 1. Rust side: fixed 100 ms steps, seed 1, 3000 ticks.
cargo run -p anno-game --bin headless -- \
    --scenario extracted/szenes/game00.szs \
    --seed 1 --dt 100 --ticks 3000 --dump-every 10 --out rust.jsonl

# 2. Original side (needs 1602.exe + a Windows-side Frida; Linux
#    frida.attach on wow64 Wine kills wine-preloader — see
#    tools/capture/README.md and docs/original-capture.md):
python tools/capture/capture.py --exe 'C:/Anno1602/1602.exe' \
    --seed 1 --ticks 3000 --dump-every 10 --out original.jsonl

# 3. Diff:
python tools/compare/compare.py rust.jsonl original.jsonl
```

`compare.py` exits non-zero and prints the first tick where an exact field
diverges — that tick localizes the behavioural difference to one subsystem
firing (from the `fired` list) and one changed field.

## Replaying a real session

```sh
# Record a play session (writes initial snapshot + timestamped commands):
cargo run -p anno-game --bin game -- extracted/szenes/game00.szs --record session.repl

# Re-run it headless, deterministically, and dump state:
cargo run -p anno-game --bin headless -- \
    --scenario extracted/szenes/game00.szs --seed 1 \
    --replay session.repl --ticks 3000 --out rust.jsonl
```

Player commands — tax, diplomacy, buy/sell, **building placement,
demolition, and unit move orders** — all flow through `anno_sim::commands::Command`
and (for the ones needing the compiled COD table) `anno_game::game_commands`,
so a recording captures the full session. `replay_advancing` re-applies them
at their recorded `game_clock` while ticking at a fixed dt.

## Current limitations

- **Float drift is expected.** Positions are compared with `--pos-tol`;
  they are not bit-exact and never will be without an x87-emulation layer.
- **Capture-side field offsets need calibration.** The within-record byte
  offsets for player gold/population in `tools/capture/capture.js` are
  placeholders (the record base addresses and strides are documented; the
  field offsets are not). Until calibrated, rely on the RNG-word and
  scheduler-trace comparisons, which don't depend on them. See the capture
  README.
- **No binary in this repo.** The capture side is untestable here; the Rust
  side is fully exercised by the workspace tests
  (`save::save_load_is_phase_exact`, `save::state_hash_*`,
  `replay::advancing_replay_retraces_a_recorded_run`).
