var Qt = {
    // Any spelling QML accepts (`#rgb`, `#rrggbb`, `#aarrggbb`, an SVG name,
    // `transparent`) or a colour already
    color(v) {
        if (v instanceof __AoshColor) return v;
        const s = __aosh_colour(String(v));
        if (s.length !== 9 || s[0] !== "#") return new __AoshColor(0, 0, 0, 0, false);
        const n = i => parseInt(s.slice(i, i + 2), 16) / 255;
        const [a, r, g, b] = [n(1), n(3), n(5), n(7)];
        if ([a, r, g, b].some(isNaN)) return new __AoshColor(0, 0, 0, 0, false);
        return new __AoshColor(r, g, b, a);
    },
    rgba(r, g, b, a) {
        const k = x => Math.min(1, Math.max(0, Number(x)));
        return new __AoshColor(k(r), k(g), k(b), a === undefined ? 1 : k(a));
    },
    rgb(r, g, b) { return Qt.rgba(r, g, b, 1); },
    hsva(h, s, v, a) { return __aosh_from_hsv(h, s, v, a === undefined ? 1 : a); },
    hsla(h, s, l, a) {
        // HSL to HSV, then the one conversion
        const v = l + s * Math.min(l, 1 - l);
        return __aosh_from_hsv(h, v === 0 ? 0 : 2 * (1 - l / v), v, a === undefined ? 1 : a);
    },
    colorEqual(x, y) { return String(Qt.color(x)) === String(Qt.color(y)); },
    alpha(c, a) { c = Qt.color(c); return new __AoshColor(c.r, c.g, c.b, a); },
    // QColor::lighter: the HSV value times the factor, and past full value
    // the saturation gives way instead
    lighter(c, f) {
        f = f === undefined ? 1.5 : f;
        c = Qt.color(c);
        if (f <= 0) return c;
        if (f < 1) return Qt.darker(c, 1 / f);
        let [h, s, v] = __aosh_hsv(c);
        v *= f;
        if (v > 1) { s = Math.max(0, s - (v - 1)); v = 1; }
        return __aosh_from_hsv(h, s, v, c.a);
    },
    darker(c, f) {
        f = f === undefined ? 2.0 : f;
        c = Qt.color(c);
        if (f <= 0) return c;
        if (f < 1) return Qt.lighter(c, 1 / f);
        const [h, s, v] = __aosh_hsv(c);
        return __aosh_from_hsv(h, s, v / f, c.a);
    },
    // The tint's own alpha is how much of it lies over the base
    tint(base, tint) {
        const b = Qt.color(base), t = Qt.color(tint), k = t.a;
        return new __AoshColor(b.r + (t.r - b.r) * k, b.g + (t.g - b.g) * k, b.b + (t.b - b.b) * k, b.a);
    },
    point(x, y) { return { x, y }; },
    size(width, height) { return { width, height }; },
    rect(x, y, width, height) { return { x, y, width, height }; },
    callLater(f, ...args) { __aosh_later.push(() => f(...args)); },
    // A string is Qt's format letters (`locale.js`). Nothing is
    // Locale.ShortFormat; Qt.ISODate (1) and Qt.TextDate (0) are those
    formatDateTime(d, fmt) {
        if (typeof fmt === "string") return __aosh_format_date(d, fmt);
        if (fmt === 1 || fmt === 9) return __aosh_format_date(d, fmt === 9 ? "yyyy-MM-ddTHH:mm:ss.zzz" : "yyyy-MM-ddTHH:mm:ss");
        if (fmt === 0) return __aosh_format_date(d, "ddd MMM d HH:mm:ss yyyy");
        return __aosh_format_date(d, __aosh_system_locale().dateTimeFormat(Locale.ShortFormat));
    },
    formatTime(d, fmt) {
        return typeof fmt === "string" ? __aosh_format_date(d, fmt)
            : __aosh_format_date(d, __aosh_system_locale().timeFormat(fmt === undefined ? Locale.ShortFormat : fmt));
    },
    formatDate(d, fmt) {
        return typeof fmt === "string" ? __aosh_format_date(d, fmt)
            : __aosh_format_date(d, __aosh_system_locale().dateFormat(fmt === undefined ? Locale.ShortFormat : fmt));
    },
    locale(name) { return name === undefined ? __aosh_system_locale() : __aosh_locale(String(name)); },
    // `x = Qt.binding(() => y * 2)`: a binding, made by the property it is
    // assigned to (`__aosh_set`)
    binding(f) { return { __aosh_binding: f }; },
    // A file's own `Qt` resolves a relative URL against the file's directory
    // (`object/build.rs`, `file_scope`). This one, for scripts outside any
    // file, has only absolute ones to resolve
    resolvedUrl(u) { return __aosh_resolved(u, ""); },
    md5(text) { return __aosh_md5(String(text)); },
    vector2d(x, y) { return { x, y }; },
    vector3d(x, y, z) { return { x, y, z }; },
    vector4d(x, y, z, w) { return { x, y, z, w }; },
    quit() { console.warn("Qt.quit(): aosh is the desktop, and does not quit"); },
    openUrlExternally(u) {
        console.warn(`Qt.openUrlExternally(${u}): nothing opens URLs on MikuOS yet`);
        return false;
    },
    AlignLeft: 1, AlignRight: 2, AlignHCenter: 4, AlignJustify: 8,
    AlignTop: 32, AlignBottom: 64, AlignVCenter: 128, AlignCenter: 132,
    Horizontal: 1, Vertical: 2,
    LeftToRight: 0, RightToLeft: 1,
    TextDate: 0, ISODate: 1, RFC2822Date: 8, ISODateWithMs: 9,
    LeftButton: 1, RightButton: 2, MiddleButton: 4, AllButtons: 0x7ffffff,
    NoModifier: 0, ShiftModifier: 0x2000000, ControlModifier: 0x4000000, AltModifier: 0x8000000,
    MetaModifier: 0x10000000,
    PointingHandCursor: 13, ArrowCursor: 0, IBeamCursor: 4,
    transparent: "#00000000",
    // Qt's key numbers (`input/keys.rs` makes them from Linux keycodes)
    Key_Escape: 0x01000000, Key_Tab: 0x01000001, Key_Backtab: 0x01000002,
    Key_Backspace: 0x01000003, Key_Return: 0x01000004, Key_Enter: 0x01000005,
    Key_Insert: 0x01000006, Key_Delete: 0x01000007, Key_Pause: 0x01000008,
    Key_Home: 0x01000010, Key_End: 0x01000011, Key_Left: 0x01000012, Key_Up: 0x01000013,
    Key_Right: 0x01000014, Key_Down: 0x01000015, Key_PageUp: 0x01000016, Key_PageDown: 0x01000017,
    Key_Shift: 0x01000020, Key_Control: 0x01000021, Key_Meta: 0x01000022, Key_Alt: 0x01000023,
    Key_CapsLock: 0x01000024, Key_Space: 0x20, Key_Asterisk: 0x2a, Key_Plus: 0x2b,
    Key_Minus: 0x2d, Key_Period: 0x2e, Key_Slash: 0x2f, Key_unknown: 0x01ffffff,
};
for (let i = 0; i < 26; i++) Qt["Key_" + String.fromCharCode(65 + i)] = 65 + i;
for (let i = 0; i < 10; i++) Qt["Key_" + i] = 48 + i;
for (let i = 1; i <= 12; i++) Qt["Key_F" + i] = 0x01000030 + i - 1;

// `Qt.resolvedUrl`'s rule. A URL with a scheme (`file:`, `image:`, `https:`),
// or nothing, stays as it is. A path is a `file://` URL: an absolute one as it
// is, a relative one against `dir` with `.` and `..` taken out, as
// QUrl::resolved does, or, with no `dir`, as it is
function __aosh_resolved(u, dir) {
    if (u === undefined || u === null) return "";
    u = String(u);
    if (u === "" || /^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(u)) return u;
    if (!u.startsWith("/") && dir === "") return u;
    const segs = (u.startsWith("/") ? u : dir + "/" + u).split("/").slice(1);
    const out = [];
    segs.forEach((p, i) => {
        const last = i === segs.length - 1;
        if (p === ".") { if (last) out.push(""); }
        else if (p === "..") { out.pop(); if (last) out.push(""); }
        else out.push(p);
    });
    return "file:///" + out.join("/");
}

// A file's `Qt`: the global one, resolving against the file's directory
function __aosh_file_qt(dir) {
    const q = Object.create(Qt);
    q.resolvedUrl = (u) => __aosh_resolved(u, dir);
    return q;
}
