// A colour, as Qt's `color` value type: components from 0 to 1, the HSV and HSL
// views of them, and the string Qt makes of one (`#rrggbb`, or `#aarrggbb` when
// it is not opaque). A `color` property holds the string `#aarrggbb`, which is
// what the painter reads, and reading the property gives one of these
// (`__aosh_get`), interned, so a binding reading the same colour a thousand
// times makes one object
class __AoshColor {
    constructor(r, g, b, a, valid) {
        this.r = r; this.g = g; this.b = b; this.a = a;
        Object.defineProperty(this, "valid", { value: valid !== false });
    }
    get hsvHue() { return __aosh_hsv(this)[0]; }
    get hsvSaturation() { return __aosh_hsv(this)[1]; }
    get hsvValue() { return __aosh_hsv(this)[2]; }
    get hslHue() { return __aosh_hsl(this)[0]; }
    get hslSaturation() { return __aosh_hsl(this)[1]; }
    get hslLightness() { return __aosh_hsl(this)[2]; }
    lighter(f) { return Qt.lighter(this, f); }
    darker(f) { return Qt.darker(this, f); }
    alpha(a) { return Qt.alpha(this, a); }
    tint(t) { return Qt.tint(this, t); }
    toString() {
        const h = x => Math.round(Math.min(1, Math.max(0, x)) * 255).toString(16).padStart(2, "0");
        const rgb = h(this.r) + h(this.g) + h(this.b);
        return this.a >= 1 ? "#" + rgb : "#" + h(this.a) + rgb;
    }
}
// Qt's hue is from 0 to 1 here, and -1 for a grey, which has none
function __aosh_hsv(c) {
    const max = Math.max(c.r, c.g, c.b), min = Math.min(c.r, c.g, c.b), d = max - min;
    return [__aosh_hue(c, max, d), max === 0 ? 0 : d / max, max];
}
function __aosh_hsl(c) {
    const max = Math.max(c.r, c.g, c.b), min = Math.min(c.r, c.g, c.b), d = max - min;
    const l = (max + min) / 2;
    const s = d === 0 ? 0 : d / (1 - Math.abs(2 * l - 1));
    return [__aosh_hue(c, max, d), Math.min(1, s), l];
}
function __aosh_hue(c, max, d) {
    if (d === 0) return -1;
    let h = max === c.r ? ((c.g - c.b) / d) % 6 : max === c.g ? (c.b - c.r) / d + 2 : (c.r - c.g) / d + 4;
    h /= 6;
    return h < 0 ? h + 1 : h;
}
function __aosh_from_hsv(h, s, v, a) {
    if (h < 0) return new __AoshColor(v, v, v, a);
    const i = Math.floor(h * 6) % 6, f = h * 6 - Math.floor(h * 6);
    const p = v * (1 - s), q = v * (1 - f * s), t = v * (1 - (1 - f) * s);
    const [r, g, b] = [[v, t, p], [q, v, p], [p, v, t], [p, q, v], [t, p, v], [v, p, q]][i];
    return new __AoshColor(r, g, b, a);
}
// What a colour property's string is, as a colour, made once per string
var __aosh_colours = new Map();
function __aosh_colour_of(str) {
    let c = __aosh_colours.get(str);
    if (c === undefined) {
        if (__aosh_colours.size > 4096) __aosh_colours.clear();
        c = Qt.color(str);
        __aosh_colours.set(str, c);
    }
    return c;
}
