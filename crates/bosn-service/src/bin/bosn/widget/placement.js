// KDE Wayland owns window placement. Use the bubble's own monitor and usable
// area, so panels/docks and negative monitor origins are respected.
function isStatusWindow(window) {
    return window.resourceClass === "dev.bosn.widget" &&
        (window.caption === "bosn bubble" || window.caption === "bosn panel");
}
function placeBubble(window) {
    if (!isStatusWindow(window)) return true;
    const area = workspace.clientArea(KWin.MaximizeArea, window);
    const geometry = window.frameGeometry;
    const margin = 24;
    window.keepAbove = true;
    window.skipTaskbar = true;
    window.skipPager = true;
    window.skipSwitcher = true;
    const expected = {
        x: Math.max(area.x, area.x + area.width - geometry.width - margin),
        y: Math.max(area.y, area.y + area.height - geometry.height - margin),
        width: geometry.width,
        height: geometry.height
    };
    window.frameGeometry = expected;
    // KWin may apply geometry after the setter returns: report the readback
    // and let the health timer decide whether a mismatch persists.
    const actual = window.frameGeometry;
    return actual.x === expected.x && actual.y === expected.y &&
        actual.width === expected.width && actual.height === expected.height;
    // Never activate the bubble: the panel/full view alone accept user focus.
}

function placeAll() {
    let applied = true;
    workspace.windowList().forEach(function (window) {
        if (!placeBubble(window)) applied = false;
    });
    return applied;
}
function connectIfAvailable(signal, callback) {
    if (signal) signal.connect(callback);
}
function watch(window) {
    placeBubble(window);
    if (window.dock) {
        // Panel resizing, moving, appearing or hiding changes usable geometry.
        window.frameGeometryChanged.connect(placeAll);
        connectIfAvailable(window.windowShown, placeAll);
        connectIfAvailable(window.windowHidden, placeAll);
    } else if (isStatusWindow(window)) {
        window.outputChanged.connect(function () { placeBubble(window); });
        connectIfAvailable(window.windowShown, function () { placeBubble(window); });
    }
    placeAll();
}
workspace.windowList().forEach(watch);
workspace.windowAdded.connect(watch);
workspace.windowRemoved.connect(placeAll);
workspace.screensChanged.connect(placeAll);
workspace.virtualScreenGeometryChanged.connect(placeAll);

// A fresh marker transition proves this loaded script actually executes its
// placement handler and reads the resulting geometry. Never run the marker:
// it is an empty, compositor-owned script object used only for readiness.
const marker = "bosn-widget-corner-ready-v1";
let pulsePending = false;
let pendingTicks = 0;
// A failure withdraws readiness and backs off, doubling up to 60 s, instead of
// stopping the timer: a transient compositor rejection heals on a later tick.
// One diagnostic per transition into failure, one on recovery.
const maxHoldTicks = 120;
let failures = 0;
let holdTicks = 0;
let failing = false;
let mismatchTicks = 0;
const health = new QTimer();
health.interval = 500;
function reportReadiness(reason) {
    console.warn("bosn-widget-corner readiness: " + reason + "; run bosn widget install to reload the owned placement script");
}
function failReadiness(reason) {
    if (!failing) reportReadiness(reason);
    failing = true;
    pulsePending = false;
    holdTicks = Math.min(maxHoldTicks, Math.pow(2, failures));
    failures++;
}
function recovered() {
    if (failing) console.warn("bosn-widget-corner readiness: recovered");
    failing = false;
    failures = 0;
}
health.timeout.connect(function () {
    if (pulsePending) {
        // KWin omits the callback on D-Bus errors. Report once per pending
        // request, without overlapping it or changing click-time deadlines.
        pendingTicks++;
        if (pendingTicks === 4) reportReadiness("D-Bus callback pending");
        return;
    }
    if (holdTicks > 0) { holdTicks--; return; }
    let applied;
    try { applied = placeAll(); } catch (error) { applied = false; mismatchTicks = 1; }
    if (!applied) {
        // One tick of grace for asynchronous placement; a mismatch on two
        // consecutive ticks is a rejection. Withdraw the marker so the widget
        // refuses the compact panel while placement is not proven.
        if (++mismatchTicks < 2) return;
        mismatchTicks = 0;
        failReadiness("geometry readback rejected");
        callDBus("org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting", "unloadScript", marker);
        return;
    }
    mismatchTicks = 0;
    pulsePending = true;
    pendingTicks = 0;
    // Script restarts can leave the empty marker loaded. Query compositor state
    // each time; a local toggle also races KWin's deferred script destruction.
    callDBus("org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting", "isScriptLoaded", marker, function (present) {
        if (typeof present !== "boolean") { failReadiness("invalid marker query reply"); return; }
        function complete(result) {
            const accepted = present ? result === true : typeof result === "number" && result >= 0;
            if (!accepted) { failReadiness(present ? "marker removal rejected" : "marker creation rejected"); return; }
            pulsePending = false;
            recovered();
        }
        if (present) {
            callDBus("org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting", "unloadScript", marker, complete);
        } else {
            callDBus("org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting", "loadScript", "/dev/null", marker, complete);
        }
    });
});
health.start();
