#!/usr/bin/env bash

# Generate thumbnails for files using ImageMagick, following Freedesktop spec
# Usage:
#   ./generate-thumbnails-magick.sh --file <path>
#   ./generate-thumbnails-magick.sh --directory <path>

set -e

# Thumbnail sizes mapping
get_thumbnail_size() {
    case "$1" in
        normal) echo 128 ;;
        large) echo 256 ;;
        x-large) echo 512 ;;
        xx-large) echo 1024 ;;
        *) echo 128 ;;
    esac
}

usage() {
    echo "Usage: $0 --file <path> | --directory <path>"
    exit 1
}

md5() {
    # Calculate md5 hash of the file's absolute path
    echo -n "$1" | md5sum | awk '{print $1}'
}

urlencode() {
    # Percent-encode UTF-8 bytes like JS encodeURIComponent (what the shell's ThumbnailImage uses), keeping slashes
    python3 -c "import sys, urllib.parse; print(urllib.parse.quote(sys.argv[1], safe=\"/-_.!~*'()\"))" "$1"
}

generate_thumbnail() {
    local src="$1"
    local abs_path
    # -s: keep symlinks (wallpapers are often symlinks, and the shell hashes the unresolved path)
    abs_path="$(realpath -s "$src")"
    local kind="image"
    case "${abs_path,,}" in
        *.mp4|*.webm|*.mkv|*.avi|*.mov) kind="video" ;;
        *.gif) kind="gif" ;;
    esac
    local encoded_path
    encoded_path="$(urlencode "$abs_path")"
    local uri
    uri="file://$encoded_path"
    local hash
    hash="$(md5 "$uri")"
    local out="$CACHE_DIR/$hash.png"
    mkdir -p "$CACHE_DIR"
    if [ -f "$out" ]; then
        return
    fi
    case "$kind" in
        video)
            # One frame ~1s in (falls back to the first frame for very short clips)
            local tmp="$out.$$.tmp.png"
            ffmpeg -v error -y -ss 1 -i "$abs_path" -frames:v 1 \
                -vf "scale=${THUMBNAIL_SIZE}:${THUMBNAIL_SIZE}:force_original_aspect_ratio=decrease" "$tmp" 2>/dev/null \
            || ffmpeg -v error -y -i "$abs_path" -frames:v 1 \
                -vf "scale=${THUMBNAIL_SIZE}:${THUMBNAIL_SIZE}:force_original_aspect_ratio=decrease" "$tmp"
            [ -s "$tmp" ] && mv "$tmp" "$out" || rm -f "$tmp"
            ;;
        gif) magick "${abs_path}[0]" -resize "${THUMBNAIL_SIZE}x${THUMBNAIL_SIZE}" "$out" ;;
        *)   magick "$abs_path" -resize "${THUMBNAIL_SIZE}x${THUMBNAIL_SIZE}" "$out" ;;
    esac
}

# Parse arguments
SIZE_NAME="normal"
MODE=""
TARGET=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --file|-f)
            MODE="file"
            TARGET="$2"
            shift 2
            ;;
        --directory|-d)
            MODE="dir"
            TARGET="$2"
            shift 2
            ;;
        --size|-s)
            SIZE_NAME="$2"
            shift 2
            ;;
        *)
            usage
            ;;
    esac
    # Only one mode allowed
    [[ -n "$MODE" ]] && break
done

THUMBNAIL_SIZE="$(get_thumbnail_size "$SIZE_NAME")"
CACHE_DIR="$HOME/.cache/thumbnails/$SIZE_NAME"

if [ -z "$MODE" ] || [ -z "$TARGET" ]; then
    usage
fi

case "$MODE" in
    file)
        if [ ! -f "$TARGET" ]; then
            echo "File not found: $TARGET"
            exit 2
        fi
        generate_thumbnail "$TARGET"
        ;;
    dir)
        if [ ! -d "$TARGET" ]; then
            echo "Directory not found: $TARGET"
            exit 2
        fi
        for f in "$TARGET"/*; do
            [ -f "$f" ] || continue
            generate_thumbnail "$f" &
        done
        wait
        ;;
    *)
        usage
        ;;
esac

