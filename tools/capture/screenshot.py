#!/usr/bin/env python3
"""Capture a true-color screenshot of a window on a headless X display.

The original 1602.exe demands an 8-bit (256-color) display. If Xvfb is run at
depth 8, `ffmpeg -f x11grab` captures the raw palette indices and the colors
come out garbled. Running Xvfb at depth 24 with a Wine *virtual desktop*
(`explorer /desktop=`) makes Wine emulate the 8-bit surface in software and
convert to true color — then a plain screenshot is correct and this script is
unnecessary. This helper remains for the depth-8 case: it reads the window's
X colormap and maps pixel indices to RGB, writing a PPM (convert with ffmpeg).

Usage:
    python screenshot.py :140 a00001 out.ppm     # window <hex id> -> PPM
    ffmpeg -y -i out.ppm out.png                 # then convert

Needs python-xlib. See docs/original-capture.md.
"""
import sys
from Xlib import display, X


def main():
    dname, wid_hex, outpath = sys.argv[1], sys.argv[2], sys.argv[3]
    d = display.Display(dname)
    w = d.create_resource_object("window", int(wid_hex, 16))
    geom = w.get_geometry()
    width, height = geom.width, geom.height
    cmap = w.get_attributes().colormap
    res = cmap.query_colors(list(range(256)))
    cols = res.colors if hasattr(res, "colors") else res
    palette = [((c.red >> 8) & 0xFF, (c.green >> 8) & 0xFF, (c.blue >> 8) & 0xFF)
               for c in cols]
    img = w.get_image(0, 0, width, height, X.ZPixmap, 0xFFFFFFFF)
    data = img.data if isinstance(img.data, (bytes, bytearray)) else bytes(img.data)
    out = bytearray(b"P6\n%d %d\n255\n" % (width, height))
    for idx in data[: width * height]:
        r, g, b = palette[idx]
        out += bytes((r, g, b))
    with open(outpath, "wb") as fh:
        fh.write(out)
    print(f"wrote {outpath} ({width}x{height})")


if __name__ == "__main__":
    main()
