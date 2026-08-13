#!/usr/bin/env python3
"""XTEST input driver for automating the original 1602.exe under Wine+Xvfb.

Sends mouse clicks and key events to a headless X display (no window manager
required for mouse; keyboard needs the game window to hold X input focus).
Uses python-xlib (`pip install python-xlib`) — no root, no xdotool.

Usage:
    python xinput.py :140 click 215 315          # left-click at (x,y)
    python xinput.py :140 move 400 535           # move pointer
    python xinput.py :140 key Return             # key to focused window
    python xinput.py :140 wkey a00001 Return     # focus window <hex id> then key
    python xinput.py :140 flag 0                 # click a FARBWAHL colour flag
    python xinput.py :140 start-tutorial         # title → Explore tutorial
    python xinput.py :140 windows                # list large mapped windows

The "Enter your name" hall (FARBWAHL.GAD) is confirmed by clicking a player
colour flag, not Return. Return only commits the name text gadget (mode 5)
and clears DAT_006c304c; it never starts the scenario. See
docs/original-capture.md.
"""
import sys
import time
from Xlib import display, X, XK
from Xlib.ext import xtest

# 800×600 window coords. FARBWAHL.GAD is authored at 1024×768 and scaled by
# 800/1024. Flag 0 is the large red banner (ID 35021 / 0x88CD).
FLAG_CLICKS = (
    (140, 250),  # red (left)
    (273, 357),  # blue
    (541, 359),  # white
    (700, 313),  # yellow (right)
)

# Title → Singleplayer → Tutorial → New Game → Start Game → red flag.
TUTORIAL_CLICKS = (
    (215, 315, 2.0),
    (430, 316, 2.0),
    (220, 305, 2.0),
    (200, 487, 12.0),  # hall + speech queue must enable the flags
    (140, 250, 2.0),
)


def connect(dname):
    return display.Display(dname)


def move(d, x, y):
    d.screen().root.warp_pointer(x, y)
    d.sync()
    time.sleep(0.15)


def click(d, x, y):
    move(d, x, y)
    xtest.fake_input(d, X.ButtonPress, 1)
    d.sync()
    time.sleep(0.06)
    xtest.fake_input(d, X.ButtonRelease, 1)
    d.sync()
    time.sleep(0.2)


def key(d, keyname, hold=0.05):
    ks = XK.string_to_keysym(keyname)
    kc = d.keysym_to_keycode(ks)
    xtest.fake_input(d, X.KeyPress, kc)
    d.sync()
    time.sleep(hold)
    xtest.fake_input(d, X.KeyRelease, kc)
    d.sync()
    time.sleep(0.1)


def focus_window(d, wid_hex):
    w = d.create_resource_object("window", int(wid_hex, 16))
    w.set_input_focus(X.RevertToParent, X.CurrentTime)
    d.sync()
    time.sleep(0.1)


def click_flag(d, index=0):
    if not 0 <= index < len(FLAG_CLICKS):
        sys.exit(f"flag index must be 0..{len(FLAG_CLICKS) - 1}")
    x, y = FLAG_CLICKS[index]
    click(d, x, y)


def start_tutorial(d):
    """Drive the title menu into the Explore tutorial (Anonymous / red)."""
    for x, y, pause in TUTORIAL_CLICKS:
        click(d, x, y)
        time.sleep(pause)
    # One retry on the red fabric in case the first hall click raced the
    # DAT_005b1bf4 speech-queue enable in FUN_0048af70.
    click(d, *FLAG_CLICKS[0])


def list_windows(d):
    root = d.screen().root

    def walk(w, depth=0):
        try:
            for c in w.query_tree().children:
                try:
                    g = c.get_geometry()
                    if g.width >= 300 and g.height >= 200 and \
                       c.get_attributes().map_state == X.IsViewable:
                        print(f"win=0x{c.id:x} {g.width}x{g.height}+{g.x}+{g.y} "
                              f"name={c.get_wm_name()!r}")
                except Exception:
                    pass
                walk(c, depth + 1)
        except Exception:
            pass

    walk(root)


def main():
    dname = sys.argv[1]
    cmd = sys.argv[2]
    d = connect(dname)
    if cmd == "click":
        click(d, int(sys.argv[3]), int(sys.argv[4]))
    elif cmd == "move":
        move(d, int(sys.argv[3]), int(sys.argv[4]))
    elif cmd == "key":
        key(d, sys.argv[3])
    elif cmd == "wkey":
        focus_window(d, sys.argv[3])
        for k in sys.argv[4:]:
            key(d, k)
    elif cmd == "windows":
        list_windows(d)
    elif cmd == "flag":
        index = int(sys.argv[3]) if len(sys.argv) > 3 else 0
        click_flag(d, index)
    elif cmd == "start-tutorial":
        start_tutorial(d)
    else:
        sys.exit(f"unknown command: {cmd}")


if __name__ == "__main__":
    main()
