import QtQuick
import Quickshell
import Quickshell.Io
import qs.modules.common
import qs.modules.common.widgets
import qs.modules.common.functions

/**
 * Thumbnail image. It currently generates to the right place at the right size, but does not handle metadata/maintenance on modification.
 * See Freedesktop's spec: https://specifications.freedesktop.org/thumbnail-spec/thumbnail-spec-latest.html
 */
StyledImage {
    id: root

    property bool generateThumbnail: true
    required property string sourcePath
    property string thumbnailSizeName: Images.thumbnailSizeNameForDimensions(sourceSize.width, sourceSize.height)
    property string thumbnailPath: {
        if (sourcePath.length == 0) return;
        const resolvedUrlWithoutFileProtocol = FileUtils.trimFileProtocol(`${Qt.resolvedUrl(sourcePath)}`);
        const encodedUrlWithoutFileProtocol = resolvedUrlWithoutFileProtocol.split("/").map(part => encodeURIComponent(part)).join("/");
        const md5Hash = Qt.md5(`file://${encodedUrlWithoutFileProtocol}`);
        return `${Directories.genericCache}/thumbnails/${thumbnailSizeName}/${md5Hash}.png`;
    }
    source: thumbnailPath

    asynchronous: true
    smooth: true
    mipmap: false

    opacity: status === Image.Ready ? 1 : 0
    Behavior on opacity {
        animation: Appearance.animation.elementMoveFast.numberAnimation.createObject(this)
    }

    onSourceSizeChanged: {
        if (!root.generateThumbnail) return;
        thumbnailGeneration.running = false;
        thumbnailGeneration.running = true;
    }
    Process {
        id: thumbnailGeneration
        command: {
            const maxSize = Images.thumbnailSizes[root.thumbnailSizeName];
            const thumbPath = FileUtils.trimFileProtocol(root.thumbnailPath);
            return ["bash", "-c",
                `[ -f '${thumbPath}' ] && exit 0 || { mkdir -p "$(dirname '${thumbPath}')"; tmp='${thumbPath}'.$$.tmp.png; if [[ '${root.sourcePath}' =~ \\.(mp4|webm|mkv|avi|mov|MP4|WEBM|MKV|AVI|MOV)$ ]]; then ffmpeg -v error -y -ss 1 -i '${root.sourcePath}' -frames:v 1 -vf scale=${maxSize}:${maxSize}:force_original_aspect_ratio=decrease "\$tmp"; else magick '${root.sourcePath}' -resize ${maxSize}x${maxSize} "\$tmp"; fi && mv "\$tmp" '${thumbPath}' && exit 1; exit 2; }`
            ]
        }
        onExited: (exitCode, exitStatus) => {
            if (exitCode === 1) {
                root.source = "";
                root.source = root.thumbnailPath;
            }
        }
    }
}
