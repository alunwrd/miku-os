//@ pragma UseQApplication
//@ pragma Env QS_NO_RELOAD_POPUP=1
//@ pragma Env QT_QUICK_CONTROLS_STYLE=Basic
//@ pragma Env QT_QUICK_FLICKABLE_WHEEL_DECELERATION=10000
// Remove two slashes below and adjust the value to change the UI scale
////@ pragma Env QT_SCALE_FACTOR=1
import "modules/common"
import "services"
import "panelFamilies"
import QtQuick
import QtQuick.Window
import Quickshell
import Quickshell.Io
import Quickshell.Hyprland

ShellRoot {
    id: root

    ReloadPopup {}

    Process {
        id: autostartProc
        command: ["python3", `${Directories.scriptPath}/hyprland/autostart.py`]
    }

    Connections {
        target: Config
        function onReadyChanged() {
            if (!Config.ready) return

            if (WM.compositor === "niri") {
                Config.options.background.lockWall = ""
                Config.options.overview.enable = false
            }

            if (Config.options.hyprland.autostartApps.enable &&
                Config.options.hyprland.autostartApps.apps.length > 0) {
                autostartProc.running = true
            }
        }
    }

    // MikuOS: what starts programs MikuOS does not have is not started -
    // hyprsunset and hyprctl, killall for KDE's and mako's daemons, cliphist,
    // pacman's checkupdates, the lyrics fetcher in python
    Component.onCompleted: {
        MaterialThemeLoader.reapplyTheme()
        FirstRunExperience.load()
        Wallpapers.load()
    }
    
    PanelFamilyLoader {
        identifier: "ii"
        // MikuOS: the family is negi's, and grows a panel at a time
        component: NegiFamily {}
    }

    component PanelFamilyLoader: LazyLoader {
        required property string identifier
        active: Config.ready && Config.options.panelFamily === identifier
    }
}