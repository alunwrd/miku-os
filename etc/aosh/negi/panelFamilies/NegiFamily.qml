import QtQuick
import Quickshell

import qs.modules.common
import qs.modules.ii.background
import qs.modules.ii.bar
// import qs.modules.ii.dock
// import qs.modules.ii.equalizer
// import qs.modules.ii.lock
// import qs.modules.ii.mediaControls
// import qs.modules.ii.notificationPopup
// import qs.modules.ii.onScreenDisplay
// import qs.modules.ii.onScreenKeyboard
import qs.modules.ii.overview
// import qs.modules.ii.polkit
// import qs.modules.ii.settings
// import qs.modules.ii.regionSelector
import qs.modules.ii.screenCorners
// import qs.modules.ii.screenTranslator
// import qs.modules.ii.sessionScreen
// import qs.modules.ii.sidebarLeft
// import qs.modules.ii.sidebarRight
// import qs.modules.ii.overlay
// import qs.modules.ii.verticalBar
// import qs.modules.ii.wallpaperSelector
// import qs.modules.ii.desktopMenu
// import qs.modules.ii.dropover
// import qs.modules.ii.frame

// MikuOS: the panels arrive one at a time. Background, Bar, Overview and
// ScreenCorners run on aosh; each line below is uncommented when its panel
// does, so `Config.ready` never makes the ones that need things this machine
// does not have yet.
// Upstream's list, in upstream's order, is otherwise unchanged
Scope {
    PanelLoader { extraCondition: !Config.options.bar.vertical; component: Bar {} }
    PanelLoader { component: Background {} }
    // PanelLoader { extraCondition: Config.options.dock.enable; component: Dock {} }
    // PanelLoader { component: EqualizerPopup {} }
    // PanelLoader { component: Lock {} }
    // PanelLoader { component: MediaControls {} }
    // PanelLoader { component: NotificationPopup {} }
    // PanelLoader { component: OnScreenDisplay {} }
    // PanelLoader { component: OnScreenKeyboard {} }
    // PanelLoader { component: Overlay {} }
    PanelLoader { component: Overview {} }
    // PanelLoader { component: Polkit {} }
    // PanelLoader { component: RegionSelector {} }
    PanelLoader { component: ScreenCorners {} }
    // PanelLoader { component: ScreenTranslator {} }
    // PanelLoader { component: SessionScreen {} }
    // PanelLoader { component: SidebarLeft {} }
    // PanelLoader { component: SidebarRight {} }
    // PanelLoader { extraCondition: Config.options.bar.vertical; component: VerticalBar {} }
    // PanelLoader { component: WallpaperSelector {} }
    // PanelLoader { component: Settings {} }
    // PanelLoader { component: DesktopMenu {} }
    // PanelLoader { component: DropShelfPanel {} }
    // PanelLoader { component: NiriBackdrop {} }
    // PanelLoader { component: ScreenFrame {} }
}
