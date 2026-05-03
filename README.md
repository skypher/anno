# anno

A faithful reverse-engineering of Anno 1602 (Creation of a New World) in
Rust. The intent is to reproduce the original game's behaviour, formulas,
data formats, and feel — **not** to build a remix, remake, or
modernised variant with added features.

## Project goal

This is a **faithful reverse engineer**. Mechanics, balance, UI flow,
and progression should match the original 1602.exe. When in doubt, the
decompiled binary in `decompiled/1602_exe.c` and the original game data
under `extracted/` are the source of truth. Modern strategy-game
conveniences (dynamic markets, auto-pause on menus, day/night, console
commands, floating damage numbers, flat diagnostic panels, etc.) are
out of scope unless they exist in the original — and have been
explicitly removed when introduced by mistake (see `STATUS.md`
authenticity-audit sections).

The codebase is *not* a vehicle for new gameplay ideas. New code must
pass the test "would the 1996 original have done this?". If the answer
is no, it doesn't belong here.

## Layout

- `crates/anno-formats` — file format parsers (BSH, COL, COD, SZS).
- `crates/anno-render` — sprite manager and SDL2 rendering helpers.
- `crates/anno-audio` — wave and stream playback wrappers.
- `crates/anno-sim` — pure simulation: production, population,
  economy, AI, combat, diplomacy, trade.
- `crates/anno-net` — TCP-based replacement for the original
  DirectPlay / Maxnet.dll multiplayer protocol.
- `crates/anno-game` — the SDL2 game binary plus diagnostic tools.
- `extracted/` — original game data files.
- `decompiled/` — Ghidra decompilation of `1602.exe`,
  `Maxsound.dll`, and `Maxnet.dll`.
- `docs/` — protocol notes, format notes, design references.
- `STATUS.md` — running implementation status and authenticity audits.

## Build

```
cargo build --workspace
cargo test --workspace
cargo run --bin game
```

Sprite-viewer and island-viewer binaries are also available; see
`STATUS.md` for the current feature list.
