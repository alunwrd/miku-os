// A Canvas's "2d" context, as far as a shell paints with one
//
// It draws nothing itself. Every call and every style set goes on a list, which
// aosh replays into the Canvas's picture once `onPaint` has run
// (`scene/canvas/`). What a script reads back (a style, the line width, the
// dash) is kept here too, saved and restored with `save()`/`restore()`.
var __aosh_ctx2d_styles = {
    fillStyle: "#000000", strokeStyle: "#000000", lineWidth: 1, lineCap: "butt", lineJoin: "miter",
    miterLimit: 10, globalAlpha: 1, globalCompositeOperation: "source-over", lineDashOffset: 0,
    font: "10px sans-serif", textAlign: "start", textBaseline: "alphabetic",
    shadowBlur: 0, shadowColor: "#00000000", shadowOffsetX: 0, shadowOffsetY: 0, fillRule: 1,
};

class __AoshGradient {
    constructor(kind, args) {
        this.kind = kind;
        this.args = args;
        this.stops = [];
    }
    addColorStop(at, colour) {
        this.stops.push([Number(at), String(colour)]);
        return this;
    }
}

class __AoshContext2D {
    constructor(canvas) {
        Object.defineProperty(this, "canvas", { value: canvas });
        Object.defineProperty(this, "__ops", { value: [], writable: true });
        Object.defineProperty(this, "__s", { value: Object.assign({}, __aosh_ctx2d_styles), writable: true });
        Object.defineProperty(this, "__dash", { value: [], writable: true });
        Object.defineProperty(this, "__saved", { value: [], writable: true });
    }
    save() {
        this.__saved.push([Object.assign({}, this.__s), this.__dash.slice()]);
        this.__ops.push(["save"]);
    }
    restore() {
        const s = this.__saved.pop();
        if (s) [this.__s, this.__dash] = s;
        this.__ops.push(["restore"]);
    }
    reset() {
        this.__s = Object.assign({}, __aosh_ctx2d_styles);
        this.__dash = [];
        this.__saved = [];
        this.__ops.push(["reset"]);
    }
    setLineDash(d) {
        this.__dash = Array.from(d || [], Number);
        this.__ops.push(["setLineDash", this.__dash.slice()]);
    }
    getLineDash() { return this.__dash.slice(); }
    createLinearGradient(x0, y0, x1, y1) { return new __AoshGradient("linear", [x0, y0, x1, y1]); }
    createRadialGradient(x0, y0, r0, x1, y1, r1) { return new __AoshGradient("radial", [x0, y0, r0, x1, y1, r1]); }
    createConicalGradient(x, y, a) { return new __AoshGradient("conical", [x, y, a]); }
    // Text is measured by its length until the context draws text
    measureText(t) { return { width: String(t).length * 6 }; }
}

// A style is put on the list as a string the replay parses, or a
// gradient's description as it stands
for (const name of Object.keys(__aosh_ctx2d_styles)) {
    Object.defineProperty(__AoshContext2D.prototype, name, {
        get() { return this.__s[name]; },
        set(v) {
            this.__s[name] = v;
            const out = v instanceof __AoshGradient
                ? { kind: v.kind, args: v.args.map(Number), stops: v.stops.map(s => s.slice()) }
                : typeof v === "number" ? v : String(v);
            this.__ops.push([name, out]);
        },
    });
}

// The calls that draw or make the path: their arguments, as given
for (const name of ["beginPath", "closePath", "moveTo", "lineTo", "bezierCurveTo", "quadraticCurveTo", "arc",
                    "arcTo", "rect", "roundedRect", "ellipse", "fill", "stroke", "clip", "fillRect", "strokeRect",
                    "clearRect", "scale", "rotate", "translate", "transform", "setTransform", "resetTransform",
                    "fillText", "strokeText", "drawImage"]) {
    __AoshContext2D.prototype[name] = function (...args) {
        this.__ops.push([name, ...args.map(a => typeof a === "number" || typeof a === "boolean" ? a : String(a))]);
    };
}

function __aosh_context2d(canvas) { return new __AoshContext2D(canvas); }
