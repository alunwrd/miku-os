#!/bin/sh
# Qt's renders of the fixtures, for aosh's tests: target/qtref/<name>.png and,
# where the fixture reports numbers, <name>.json. Run on the build host, which
# has Qt 6 and qt6-5compat; the tests that use them pass over a fixture whose
# render is not there.
#
# Qt renders them as quickshell does on Arch: through the RHI on OpenGL (an X
# server in memory, `xvfb-run`, gives it a GL surface without a window on the
# desktop; the offscreen platform has only the software renderer, which draws
# text over 64 px differently), with the KDE platform theme, whose application
# font is `kdeglobals`', and with the Controls style end4's shell asks
# quickshell for, Basic. Text drawn from distance fields (a Text's default
# `renderType`) is grey, as in quickshell's see-through windows; Qt grabbing a
# window here would give it coloured edges.
#
#     aosh/tests/qtref/make.sh            every fixture
#     aosh/tests/qtref/make.sh text       one
set -e
here=$(cd "$(dirname "$0")" && pwd)
out="$here/../../../target/qtref"
mkdir -p "$out"
names=${1:-$(cd "$here/fixtures" && ls *.qml | sed 's/\.qml$//')}
for n in $names; do
    xvfb-run -a -s "-screen 0 1920x1080x24" env QT_FORCE_STDERR_LOGGING=1 QT_QPA_PLATFORM=xcb QT_QUICK_CONTROLS_STYLE=Basic QSG_DISTANCEFIELD_ANTIALIASING=gray \
        QT_QPA_PLATFORMTHEME=kde QT_SCALE_FACTOR=1 \
        qml6 "$here/grab.qml" -- "$here/fixtures/$n.qml" "$out/$n.png" > "$out/$n.log" 2>&1 || {
        echo "qtref: $n failed, see $out/$n.log"; exit 1; }
    if grep -q '^qml: AOSHREF \|AOSHREF ' "$out/$n.log"; then
        grep -o 'AOSHREF .*' "$out/$n.log" | head -1 | sed 's/^AOSHREF //' > "$out/$n.json"
    fi
    echo "qtref: $n"
done
