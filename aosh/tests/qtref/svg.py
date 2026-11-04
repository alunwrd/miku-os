#!/usr/bin/env python3
# Qt's renders of the SVG fixtures, for aosh's SVG tests:
# target/qtref/svg/NAME@SIZE.png
#
# An icon theme's SVG reaches the screen through KDE's icon loader, which reads
# it with Qt's image reader asked for the icon's size: the `svg` image plugin,
# QSvgRenderer, painting the viewBox into a transparent picture that size
# (`QImageReader.setScaledSize`). This does the same, at the sizes end4 asks
# icons for. Run on the build host, which has PySide6 over the system's Qt 6.
import os
import sys

from PySide6.QtCore import QSize
from PySide6.QtGui import QGuiApplication, QImage, QImageReader

SIZES = [16, 20, 22, 24, 32, 48]

app = QGuiApplication(sys.argv)
here = os.path.dirname(os.path.abspath(__file__))
src = os.path.join(here, "fixtures", "svg")
out = os.path.join(here, "..", "..", "..", "target", "qtref", "svg")
os.makedirs(out, exist_ok=True)
for name in sorted(os.listdir(src)):
    if not name.endswith(".svg"):
        continue
    for s in SIZES:
        r = QImageReader(os.path.join(src, name), b"svg")
        r.setScaledSize(QSize(s, s))
        img = r.read()
        if img.isNull():
            sys.exit(f"qtref: {name} at {s}: {r.errorString()}")
        img.convertToFormat(QImage.Format_ARGB32).save(os.path.join(out, f"{name[:-4]}@{s}.png"))
    print(f"qtref: svg/{name}")
