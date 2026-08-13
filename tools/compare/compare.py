#!/usr/bin/env python3
"""Diff two per-tick state dumps: the Rust reimplementation vs the original.

Consumes the JSONL emitted by the headless Rust driver
(`crates/anno-game/src/bin/headless.rs`) and by the Frida capture of the
original `1602.exe` (`tools/capture/capture.py`). Both share one schema, one
object per line:

    {"tick": N, "rng_state": U32, "state_hash": "hex", "game_clock": N,
     "players": [{"slot": S, "gold": G, "population": [..5..]}, ...],
     "units": [...], "ships": [...], "fired": ["production", ...]}

The two engines cannot match bit-for-bit on everything: entity motion uses
f32 in Rust vs 80-bit x87 in the 1996 build, so positions drift. The
comparison therefore holds integer / fixed-point fields to EXACT equality
and float fields (unit/ship positions) to a tolerance band, and reports the
FIRST tick where an exact field diverges — the localizing signal.

It also compares the scheduler trace (which subsystem tickers fired each
tick), the cheap first-signal that survives even when field offsets on the
capture side are still being calibrated.

Usage:
    python compare.py rust.jsonl original.jsonl
    python compare.py rust.jsonl original.jsonl --pos-tol 1.5 --max-report 20
"""

import argparse
import json
import sys
from typing import Any, Dict, List, Optional, Tuple


def load_jsonl(path: str) -> List[Dict[str, Any]]:
    rows = []
    with open(path, "r", encoding="utf-8") as handle:
        for line_no, line in enumerate(handle, 1):
            line = line.strip()
            if not line:
                continue
            try:
                rows.append(json.loads(line))
            except json.JSONDecodeError as error:
                print(f"{path}:{line_no}: bad JSON: {error}", file=sys.stderr)
    return rows


def index_by_tick(rows: List[Dict[str, Any]]) -> Dict[int, Dict[str, Any]]:
    return {row["tick"]: row for row in rows if "tick" in row}


def compare_exact_fields(
    tick: int, rust: Dict[str, Any], orig: Dict[str, Any]
) -> List[str]:
    """Integer / fixed-point fields that must match exactly."""
    diffs = []

    # RNG word: the strongest single check. -1 means the capture side has
    # not located holdrand yet, so skip rather than false-alarm.
    r_rng = rust.get("rng_state", -1)
    o_rng = orig.get("rng_state", -1)
    if r_rng != -1 and o_rng != -1 and r_rng != o_rng:
        diffs.append(f"rng_state: rust={r_rng} orig={o_rng}")

    # Per-player economy. Match by slot; tolerate a side omitting players
    # it doesn't model (the capture may report fewer during calibration).
    r_players = {p["slot"]: p for p in rust.get("players", [])}
    o_players = {p["slot"]: p for p in orig.get("players", [])}
    for slot in sorted(set(r_players) & set(o_players)):
        rp, op = r_players[slot], o_players[slot]
        if "gold" in rp and "gold" in op and rp["gold"] != op["gold"]:
            diffs.append(f"player{slot}.gold: rust={rp['gold']} orig={op['gold']}")
        rpop, opop = rp.get("population"), op.get("population")
        if rpop is not None and opop is not None and rpop != opop:
            diffs.append(f"player{slot}.population: rust={rpop} orig={opop}")

    return diffs


def compare_float_fields(
    tick: int, rust: Dict[str, Any], orig: Dict[str, Any], tol: float
) -> List[str]:
    """Position fields compared within a tolerance band (x87 vs f32 drift)."""
    diffs = []
    for key in ("units", "ships"):
        r_list = rust.get(key, [])
        o_list = orig.get(key, [])
        if len(r_list) != len(o_list):
            diffs.append(f"{key}: count rust={len(r_list)} orig={len(o_list)}")
            continue
        for i, (r_item, o_item) in enumerate(zip(r_list, o_list)):
            for axis in ("x", "y"):
                if axis in r_item and axis in o_item:
                    delta = abs(float(r_item[axis]) - float(o_item[axis]))
                    if delta > tol:
                        diffs.append(
                            f"{key}[{i}].{axis}: rust={r_item[axis]} "
                            f"orig={o_item[axis]} (Δ={delta:.3f} > {tol})"
                        )
    return diffs


def compare_trace(tick: int, rust: Dict[str, Any], orig: Dict[str, Any]) -> Optional[str]:
    """Which subsystem tickers fired — order-insensitive set comparison.

    The two engines dispatch subsystems in a fixed order, but the capture's
    ticker hooks and the Rust scheduler may enumerate them differently, so
    compare the set of names that fired this tick, not the sequence.
    """
    r_fired = rust.get("fired")
    o_fired = orig.get("fired")
    if r_fired is None or o_fired is None:
        return None
    r_set, o_set = set(r_fired), set(o_fired)
    if r_set != o_set:
        only_rust = sorted(r_set - o_set)
        only_orig = sorted(o_set - r_set)
        parts = []
        if only_rust:
            parts.append(f"only-rust={only_rust}")
        if only_orig:
            parts.append(f"only-orig={only_orig}")
        return "fired: " + " ".join(parts)
    return None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("rust", help="JSONL dump from the headless Rust driver")
    parser.add_argument("original", help="JSONL dump from the Frida capture")
    parser.add_argument(
        "--pos-tol",
        type=float,
        default=1.0,
        help="tolerance (engine tiles) for float position fields",
    )
    parser.add_argument(
        "--max-report",
        type=int,
        default=20,
        help="stop after this many diverging ticks",
    )
    parser.add_argument(
        "--ignore-trace",
        action="store_true",
        help="skip the subsystem-fired comparison",
    )
    args = parser.parse_args()

    rust_rows = index_by_tick(load_jsonl(args.rust))
    orig_rows = index_by_tick(load_jsonl(args.original))

    common = sorted(set(rust_rows) & set(orig_rows))
    if not common:
        print("no common ticks between the two dumps", file=sys.stderr)
        return 2

    only_rust = sorted(set(rust_rows) - set(orig_rows))
    only_orig = sorted(set(orig_rows) - set(rust_rows))
    if only_rust:
        print(f"note: {len(only_rust)} tick(s) only in rust dump (e.g. {only_rust[:5]})")
    if only_orig:
        print(f"note: {len(only_orig)} tick(s) only in original dump (e.g. {only_orig[:5]})")

    first_exact_divergence: Optional[int] = None
    reported = 0
    exact_diverged = 0
    float_diverged = 0
    trace_diverged = 0

    for tick in common:
        rust, orig = rust_rows[tick], orig_rows[tick]
        exact = compare_exact_fields(tick, rust, orig)
        floats = compare_float_fields(tick, rust, orig, args.pos_tol)
        trace = None if args.ignore_trace else compare_trace(tick, rust, orig)

        if exact:
            exact_diverged += 1
            if first_exact_divergence is None:
                first_exact_divergence = tick
        if floats:
            float_diverged += 1
        if trace:
            trace_diverged += 1

        messages = []
        messages.extend(f"  [exact] {d}" for d in exact)
        if trace:
            messages.append(f"  [trace] {trace}")
        messages.extend(f"  [float] {d}" for d in floats)

        if messages and reported < args.max_report:
            print(f"tick {tick} (game_clock rust={rust.get('game_clock')} "
                  f"orig={orig.get('game_clock')}):")
            for message in messages:
                print(message)
            reported += 1

    print()
    print(f"compared {len(common)} common ticks")
    print(f"  exact-field divergences: {exact_diverged} tick(s)")
    print(f"  float-field divergences: {float_diverged} tick(s) (tol={args.pos_tol})")
    if not args.ignore_trace:
        print(f"  scheduler-trace divergences: {trace_diverged} tick(s)")
    if first_exact_divergence is not None:
        print(f"  FIRST exact divergence at tick {first_exact_divergence}")
        return 1
    print("  no exact-field divergence — integer/fixed-point state matches")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
