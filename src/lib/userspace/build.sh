#!/bin/bash
set -e

DISK="${DISK:-$HOME/miku-os/miku-os/data.img}"
TARGET="x86_64-miku-app"
BIN_DIR="target/$TARGET/release"

BINS="${@:-test_full hello}"

echo "Build"

for bin in $BINS; do
    echo "[*] Building $bin..."
    cargo +nightly build --release \
        --target "$TARGET.json" \
        -Z json-target-spec \
        -Z build-std=core \
        -Z build-std-features=compiler-builtins-mem \
        --bin "$bin"

    SIZE=$(stat -c%s "$BIN_DIR/$bin" 2>/dev/null || echo "?")
    echo "[ok] $bin ($SIZE bytes)"

    if [ -f "$DISK" ]; then
        e2cp "$BIN_DIR/$bin" "$DISK:/"
        echo "[ok] -> $DISK"
    fi
done

echo "Done."
