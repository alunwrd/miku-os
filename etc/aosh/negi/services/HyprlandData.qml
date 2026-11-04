pragma Singleton
pragma ComponentBehavior: Bound

import QtQuick
import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import Quickshell.Hyprland
import qs.services

/**
 * Provides access to some Hyprland data not available in Quickshell.Hyprland.
 */
Singleton {
    id: root
    property var windowList: []
    property var addresses: []
    property var windowByAddress: ({})
    property var workspaces: []
    property var workspaceIds: []
    property var workspaceById: ({})
    property var activeWorkspace: null
    property var monitors: []
    property var layers: ({})

    // Convenient stuff

    function toplevelsForWorkspace(workspace) {
        return ToplevelManager.toplevels.values.filter(toplevel => {
            const address = `0x${toplevel.HyprlandToplevel?.address}`;
            var win = HyprlandData.windowByAddress[address];
            return win?.workspace?.id === workspace;
        })
    }

    function hyprlandClientsForWorkspace(workspace) {
        return root.windowList.filter(win => win.workspace.id === workspace);
    }

    function clientForToplevel(toplevel) {
        if (!toplevel || !toplevel.HyprlandToplevel) {
            return null;
        }
        const address = `0x${toplevel?.HyprlandToplevel?.address}`;
        return root.windowByAddress[address];
    }

    // Internals

    property bool _pendingClients: false
    property bool _pendingMonitors: false
    property bool _pendingLayers: false
    property bool _pendingWorkspaces: false

    Timer {
        id: eventDebounceTimer
        interval: 60
        repeat: false
        onTriggered: {
            if (WM.compositor !== "hyprland") return;
            // MikuOS: everything is read at once, from aosh (`readShim`)
            root._pendingClients = false;
            root._pendingMonitors = false;
            root._pendingLayers = false;
            root._pendingWorkspaces = false;
            root.readShim();
        }
    }

    function queueUpdate(clients = false, workspaces = false, monitors = false, layers = false) {
        if (WM.compositor !== "hyprland") return;
        if (clients) root._pendingClients = true;
        if (workspaces) root._pendingWorkspaces = true;
        if (monitors) root._pendingMonitors = true;
        if (layers) root._pendingLayers = true;
        eventDebounceTimer.restart();
    }

    function updateWindowList() {
        queueUpdate(true, false, false, false);
    }

    function updateLayers() {
        queueUpdate(false, false, false, true);
    }

    function updateMonitors() {
        queueUpdate(false, false, true, false);
    }

    function updateWorkspaces() {
        queueUpdate(false, true, false, false);
    }

    function updateAll() {
        queueUpdate(true, true, true, true);
    }

    function biggestWindowForWorkspace(workspaceId) {
        const windowsInThisWorkspace = HyprlandData.windowList.filter(w => w.workspace.id == workspaceId);
        return windowsInThisWorkspace.reduce((maxWin, win) => {
            const maxArea = (maxWin?.size?.[0] ?? 0) * (maxWin?.size?.[1] ?? 0);
            const winArea = (win?.size?.[0] ?? 0) * (win?.size?.[1] ?? 0);
            return winArea > maxArea ? win : maxWin;
        }, null);
    }

    Component.onCompleted: {
        if (WM.compositor === "hyprland") {
            root.readShim();
        }
    }

    // MikuOS: there is no hyprctl. What `hyprctl clients/monitors/
    // workspaces/activeworkspace -j` says on Hyprland is the `lastIpcObject`
    // of aosh's Quickshell.Hyprland objects, which say what Mayland says;
    // they say what changed as Hyprland's events, so this is read when the
    // processes would have run. Mayland has no layer list to give
    function readShim() {
        root.windowList = Hyprland.toplevels.values.map(t => t.lastIpcObject).filter(w => w);
        let tempWinByAddress = {};
        for (const win of root.windowList)
            tempWinByAddress[win.address] = win;
        root.windowByAddress = tempWinByAddress;
        root.addresses = root.windowList.map(win => win.address);
        root.monitors = Hyprland.monitors.values.map(m => m.lastIpcObject).filter(m => m);
        // hyprctl lists a workspace while it has windows or is shown
        const shown = Hyprland.focusedWorkspace?.id ?? -1;
        root.workspaces = Hyprland.workspaces.values.map(w => w.lastIpcObject)
            .filter(ws => ws && ws.id >= 1 && ws.id <= 100 && (ws.windows > 0 || ws.id === shown));
        let tempWorkspaceById = {};
        for (const ws of root.workspaces)
            tempWorkspaceById[ws.id] = ws;
        root.workspaceById = tempWorkspaceById;
        root.workspaceIds = root.workspaces.map(ws => ws.id);
        root.activeWorkspace = Hyprland.focusedWorkspace?.lastIpcObject ?? null;
        root.layers = ({});
    }

    Connections {
        target: Hyprland
        enabled: WM.compositor === "hyprland"

        function onRawEvent(event) {
            const name = event.name;
            if (["openlayer", "closelayer", "screencast", "submap", "activelayout"].includes(name)) return;

            if (name.startsWith("workspace") || name.startsWith("createworkspace") || name.startsWith("destroyworkspace") || name.startsWith("moveworkspace") || name === "renameworkspace") {
                root.queueUpdate(false, true, false, false);
            } else if (name.startsWith("activespecial")) {
                root.queueUpdate(false, true, true, false);
            } else if (name.startsWith("openwindow") || name.startsWith("closewindow") || name.startsWith("movewindow")) {
                root.queueUpdate(true, true, false, false);
            } else if (name.startsWith("window") || name.startsWith("activewindow") || name === "fullscreen" || name === "changefloatingmode" || name === "pin" || name === "urgent" || name === "minimize") {
                root.queueUpdate(true, false, false, false);
            } else if (name.startsWith("monitor") || name === "focusedmon") {
                root.queueUpdate(false, true, true, false);
            } else {
                root.queueUpdate(true, true, false, false);
            }
        }
    }
}
