#!/usr/bin/env python3
"""Drive the original 1602.exe under Frida and record a per-tick state dump.

Launches (or attaches to) the game with the capture.js instrumentation,
pins the RNG seed, and writes the emitted JSON lines to a JSONL file whose
schema matches the headless Rust driver
(crates/anno-game/src/bin/headless.rs). tools/compare then diffs the two.

This requires the copyrighted 1602.exe (not in this repo) and a Windows or
Wine environment with Frida installed (`pip install frida-tools`). It is a
scaffold: the address map in capture.js is from static analysis and the
player-record field offsets are placeholders to be calibrated (see README).

Usage:
    # Launch the game under instrumentation and capture 3000 ticks:
    python capture.py --exe 'C:/Anno1602/1602.exe' --seed 1 \
        --ticks 3000 --dump-every 10 --out original.jsonl

    # Attach to an already-running instance by PID:
    python capture.py --attach 1234 --out original.jsonl
"""

import argparse
import json
import sys
import time


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--exe", help="path to 1602.exe to launch")
    source.add_argument("--attach", type=int, help="PID of a running 1602.exe")
    parser.add_argument("--seed", type=int, default=1, help="RNG seed to pin")
    parser.add_argument("--ticks", type=int, default=3000, help="sim ticks to capture (0 = until closed)")
    parser.add_argument("--dump-every", type=int, default=10, help="emit a dump every N ticks")
    parser.add_argument("--holdrand", help="hex address of the CRT holdrand word, if known (e.g. 0x005c1234)")
    parser.add_argument("--out", default="original.jsonl", help="output JSONL path")
    parser.add_argument("--script", default=None, help="path to capture.js (default: alongside this file)")
    args = parser.parse_args()

    try:
        import frida
    except ImportError:
        print("frida is not installed. Run: pip install frida-tools", file=sys.stderr)
        return 2

    import os

    script_path = args.script or os.path.join(os.path.dirname(os.path.abspath(__file__)), "capture.js")
    with open(script_path, "r", encoding="utf-8") as handle:
        script_source = handle.read()

    out = open(args.out, "w", encoding="utf-8")
    done = {"flag": False}

    def on_message(message, _data):
        if message["type"] != "send":
            print(f"[frida] {message}", file=sys.stderr)
            return
        payload = message["payload"]
        kind = payload.get("kind")
        if kind == "log":
            print(f"[capture] {payload['message']}", file=sys.stderr)
        elif kind == "dump":
            out.write(json.dumps(payload) + "\n")
            out.flush()
        elif kind == "done":
            print(f"[capture] reached {payload['tick']} ticks", file=sys.stderr)
            done["flag"] = True

    if args.exe:
        pid = frida.spawn([args.exe])
        session = frida.attach(pid)
        resume = pid
    else:
        session = frida.attach(args.attach)
        resume = None

    script = session.create_script(script_source)
    script.on("message", on_message)
    script.load()

    script.exports_sync.configure(
        {"seed": args.seed, "dumpEvery": args.dump_every, "maxTicks": args.ticks}
    )
    if args.holdrand:
        script.exports_sync.set_holdrand(args.holdrand)
    script.exports_sync.install()

    if resume is not None:
        frida.resume(resume)

    print("[capture] running; Ctrl-C to stop", file=sys.stderr)
    try:
        while not done["flag"]:
            time.sleep(0.2)
    except KeyboardInterrupt:
        print("[capture] interrupted", file=sys.stderr)
    finally:
        out.close()
        try:
            session.detach()
        except Exception:
            pass

    print(f"[capture] wrote {args.out}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
