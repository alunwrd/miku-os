pragma Singleton
pragma ComponentBehavior: Bound

import qs.modules.common
import QtQuick
import Quickshell
import Quickshell.Io

/**
 * Simple polled resource usage service with RAM, Swap, CPU and Disk usage.
 */
Singleton {
    id: root
    property real memoryTotal: 1
    property real memoryFree: 0
    property real memoryUsed: memoryTotal - memoryFree
    property real memoryUsedPercentage: memoryUsed / memoryTotal
    property real swapTotal: 1
    property real swapFree: 0
    property real swapUsed: swapTotal - swapFree
    property real swapUsedPercentage: swapTotal > 0 ? (swapUsed / swapTotal) : 0
    property real cpuUsage: 0
    property var previousCpuStats

    property string maxAvailableMemoryString: kbToGbString(ResourceUsage.memoryTotal)
    property string maxAvailableSwapString: kbToGbString(ResourceUsage.swapTotal)
    property string maxAvailableCpuString: "--"

    readonly property int historyLength: Config?.options.resources.historyLength ?? 60
    property list<real> cpuUsageHistory: []
    property list<real> memoryUsageHistory: []
    property list<real> swapUsageHistory: []

    property real cpuTemp: 0

    property real diskTotal: 1
    property real diskUsed: 0
    property real diskFree: 0
    property real diskUsedPercentage: diskTotal > 0 ? diskUsed / diskTotal : 0
    property list<real> diskUsageHistory: []
    property string maxAvailableDiskString: kbToGbString(diskTotal)

    property string thermalPath: ""

    // MikuOS: no sensors and no hwmon or thermal zones to look for one in,
    // and no sh to look with: the temperature stays unknown, as upstream
    // shows it when nothing is found
    FileView {
        id: fileTemp
        path: root.thermalPath
        printErrors: false
        onLoaded: {
            const raw = parseFloat(fileTemp.text().trim());
            if (!isNaN(raw) && raw > 0) {
                root.cpuTemp = raw > 200 ? Math.round(raw / 100) / 10 : raw;
            }
        }
    }

    Process {
        id: diskProc
        command: ["df", "-k", "/"]
        stdout: StdioCollector {
            onStreamFinished: {
                const lines = text.trim().split("\n");
                if (lines.length >= 2) {
                    const parts = lines[1].trim().split(/\s+/).map(Number);
                    if (parts.length >= 4) {
                        root.diskTotal = parts[1];
                        root.diskUsed  = parts[2];
                        root.diskFree  = parts[3];
                    }
                }
            }
        }
    }

    function kbToGbString(kb) {
        return (kb / (1024 * 1024)).toFixed(1) + " GB"
    }

    function updateMemoryUsageHistory() {
        memoryUsageHistory = [...memoryUsageHistory, memoryUsedPercentage]
        if (memoryUsageHistory.length > historyLength) memoryUsageHistory.shift()
    }
    function updateSwapUsageHistory() {
        swapUsageHistory = [...swapUsageHistory, swapUsedPercentage]
        if (swapUsageHistory.length > historyLength) swapUsageHistory.shift()
    }
    function updateCpuUsageHistory() {
        cpuUsageHistory = [...cpuUsageHistory, cpuUsage]
        if (cpuUsageHistory.length > historyLength) cpuUsageHistory.shift()
    }
    function updateDiskUsageHistory() {
        diskUsageHistory = [...diskUsageHistory, diskUsedPercentage]
        if (diskUsageHistory.length > historyLength) diskUsageHistory.shift()
    }
    function updateHistories() {
        updateMemoryUsageHistory()
        updateSwapUsageHistory()
        updateCpuUsageHistory()
        updateDiskUsageHistory()
    }

    Timer {
        interval: 1
        running: true
        repeat: true
        onTriggered: {
            fileMeminfo.reload()
            fileStat.reload()

            if (root.thermalPath.length > 0) {
                fileTemp.reload()
                const raw = parseFloat(fileTemp.text().trim())
                if (!isNaN(raw) && raw > 0) {
                    root.cpuTemp = raw > 200 ? Math.round(raw / 100) / 10 : raw
                }
            }

            diskProc.running = false
            diskProc.running = true

            const textMeminfo = fileMeminfo.text()
            memoryTotal = Number(textMeminfo.match(/MemTotal: *(\d+)/)?.[1] ?? 1)
            memoryFree  = Number(textMeminfo.match(/MemAvailable: *(\d+)/)?.[1] ?? 0)
            swapTotal   = Number(textMeminfo.match(/SwapTotal: *(\d+)/)?.[1] ?? 1)
            swapFree    = Number(textMeminfo.match(/SwapFree: *(\d+)/)?.[1] ?? 0)

            const textStat = fileStat.text()
            const cpuLine  = textStat.match(/^cpu\s+(\d+)\s+(\d+)\s+(\d+)\s+(\d+)\s+(\d+)\s+(\d+)\s+(\d+)/)
            if (cpuLine) {
                const stats = cpuLine.slice(1).map(Number)
                const total = stats.reduce((a, b) => a + b, 0)
                const idle  = stats[3]
                if (previousCpuStats) {
                    const totalDiff = total - previousCpuStats.total
                    const idleDiff  = idle  - previousCpuStats.idle
                    cpuUsage = totalDiff > 0 ? (1 - idleDiff / totalDiff) : 0
                }
                previousCpuStats = { total, idle }
            }

            root.updateHistories()
            interval = Config.options?.resources?.updateInterval ?? 3000
        }
    }

    FileView { id: fileMeminfo; path: "/proc/meminfo" }
    FileView { id: fileStat;    path: "/proc/stat" }

    // MikuOS: no lscpu; the CPU's top speed stays "--"
}
