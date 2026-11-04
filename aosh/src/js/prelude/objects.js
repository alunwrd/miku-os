var __aosh_h = {
    has(t, k) {
        if (typeof k !== "string") return k in t;
        return (k in t) || __aosh_kind(t.__oid, t.__p + k) !== 0;
    },
    get(t, k) {
        if (typeof k !== "string" || k in t) return t[k];
        // quickshell's attached `QsWindow`: the window the item is in
        if (k === "QsWindow" && t.__p === "") {
            const oid = t.__oid;
            return { get window() { return __aosh_window(oid); } };
        }
        const n = t.__p + k;
        switch (__aosh_kind(t.__oid, n)) {
            case 1: {
                const v = __aosh_get(t.__oid, n);
                return Array.isArray(v) && __aosh_is_list(t.__oid, n) ? __aosh_seq(t.__oid, n, v) : v;
            }
            case 2: { const g = new Proxy(__aosh_target(t.__oid, n + "."), __aosh_h); t[k] = g; return g; }
            case 3: { const f = (...a) => __aosh_call(t.__oid, n, ...a); t[k] = f; return f; }
            // A signal: called, it is emitted; `connect` and `disconnect`
            // add and take off a handler, as a QML signal's do
            case 4: {
                const oid = t.__oid;
                const f = (...a) => __aosh_emit(oid, n, ...a);
                f.connect = (h, m) => __aosh_connect(oid, n, typeof m === "function" ? m.bind(h) : h);
                f.disconnect = (h) => __aosh_disconnect(oid, n, h);
                t[k] = f;
                return f;
            }
        }
        return undefined;
    },
    set(t, k, v) {
        if (typeof k === "string") {
            const n = t.__p + k;
            switch (__aosh_kind(t.__oid, n)) {
                case 1: __aosh_set(t.__oid, n, v); return true;
                // `image.sourceSize = Qt.size(w, h)`: a value type given
                // whole is its parts given one by one
                case 2: __aosh_set_group(t.__oid, n, v); return true;
            }
        }
        t[k] = v;
        return true;
    },
};
// What a binding returns to leave its property as it is: a part of a value
// type the value given whole does not have (`object/build.rs`)
var __aosh_keep = Object.freeze({ __aosh_keep: true });
function __aosh_target(oid, p) {
    const t = Object.create(null);
    Object.defineProperty(t, "__oid", { value: oid });
    Object.defineProperty(t, "__p", { value: p });
    Object.defineProperty(t, "toString", {
        value: () => __aosh_type(oid) + (p ? "." + p.slice(0, -1) : ""), writable: true,
    });
    // An object's properties are its own, as a QObject wrapper says in Qt:
    // end4 asks `item.hasOwnProperty("mirrored")` of whatever a Loader made
    Object.defineProperty(t, "hasOwnProperty", {
        value: (k) => (typeof k === "string" && __aosh_kind(oid, p + k) !== 0)
            || Object.prototype.hasOwnProperty.call(t, k),
        writable: true,
    });
    return t;
}
function __aosh_wrap(oid) { return new Proxy(__aosh_target(oid, ""), __aosh_h); }
// A typed list property's array, as Qt 6 hands a sequence to JavaScript:
// changed in place (`list.push(x)`, `list.splice(i, 1)`) it is written back to
// the property, and what follows the property hears of it. end4's
// GlobalFocusGrab pushes the windows it holds onto one. The array is a plain
// one with its own changing methods, so it is an array to everything that reads
// it; `list[i] = x` is not caught
var __aosh_mutators = ["push", "pop", "shift", "unshift", "splice", "sort", "reverse", "fill", "copyWithin"];
function __aosh_seq(oid, name, arr) {
    for (const m of __aosh_mutators) {
        const f = Array.prototype[m];
        Object.defineProperty(arr, m, {
            value: (...a) => { const out = f.apply(arr, a); __aosh_set(oid, name, arr.slice()); return out; },
            enumerable: false, configurable: true, writable: true,
        });
    }
    return arr;
}
var __aosh_none = Object.create(null);

// The shell's singletons as globals, each made the first time it is named and
// then a plain value (`qml/import.rs`). A name a global already has is not
// taken over
function __aosh_singletons(names) {
    for (const n of names) {
        if (n in globalThis) continue;
        Object.defineProperty(globalThis, n, {
            configurable: true,
            get() {
                const v = __aosh_singleton(n);
                Object.defineProperty(globalThis, n, { value: v, writable: true, configurable: true });
                // A singleton's file may declare enums too: they are on it
                const later = __aosh_enums_later[n];
                if (later !== undefined && v !== null) {
                    delete __aosh_enums_later[n];
                    for (const k of Object.keys(later)) v[k] = later[k];
                }
                return v;
            },
        });
    }
}
// `import qs.modules.common.functions as CF`: that module's singletons, under
// its alias
function __aosh_space(names) {
    const o = Object.create(null);
    for (const n of names) Object.defineProperty(o, n, { get: () => globalThis[n], enumerable: true });
    return o;
}
// A type's enums, `MaterialShape.Shape.Circle`, and each value on the type as
// well, `MaterialShape.Circle`, as QML has both
var __aosh_enums_later = Object.create(null);
function __aosh_enums(type, enums) {
    // A singleton not made yet is not made for this: its enums wait for it
    const d = Object.getOwnPropertyDescriptor(globalThis, type);
    let t;
    if (d !== undefined && d.get !== undefined) t = __aosh_enums_later[type] || (__aosh_enums_later[type] = {});
    else {
        t = globalThis[type];
        if (t === undefined || t === null || typeof t !== "object") t = globalThis[type] = {};
    }
    for (const [name, values] of enums) {
        const e = t[name] || (t[name] = {});
        for (const [k, v] of values) {
            e[k] = v;
            if (!(k in t)) t[k] = v;
        }
    }
}

// An id scope. A delegate's has its row in it too, as getters that read the
// row object, so a binding that says `index` or `modelData` or a role's name
// follows it; and a ListView's delegate has the attached `ListView`
function __aosh_scope(row, roles, view) {
    const ids = Object.create(null);
    if (row !== null) {
        const def = (k, get, set) => Object.defineProperty(ids, k, { get, set, configurable: true });
        def("index", () => row.index);
        def("modelData", () => row.modelData);
        def("model", () => row);
        for (const r of roles) def(r, () => row[r], (v) => { row[r] = v; });
    }
    if (view !== null) {
        const attached = Object.create(ListView);
        Object.defineProperty(attached, "view", { get: () => view });
        Object.defineProperty(attached, "isCurrentItem", {
            get: () => row !== null && view.currentIndex === row.index,
        });
        Object.defineProperty(ids, "ListView", { value: attached, configurable: true });
    }
    return ids;
}

// The scopes a scope was made from, as one object `with` can look names up
// in: their ids, then their root's properties, then theirs
function __aosh_chain(ids, root, next) {
    const where = (k) => (k in ids) ? ids : (root !== null && k in root) ? root
        : (next !== null && k in next) ? next : null;
    return new Proxy(Object.create(null), {
        has(t, k) { return typeof k === "string" && where(k) !== null; },
        get(t, k) {
            if (typeof k !== "string") return undefined;
            const w = where(k);
            return w === null ? undefined : w[k];
        },
        set(t, k, v) {
            const w = typeof k === "string" ? where(k) : null;
            if (w === null) return false;
            w[k] = v;
            return true;
        },
    });
}

var console = {
    log: (...a) => __aosh_log("info", a.map(String).join(" ")),
    info: (...a) => __aosh_log("info", a.map(String).join(" ")),
    debug: (...a) => __aosh_log("debug", a.map(String).join(" ")),
    warn: (...a) => __aosh_log("warn", a.map(String).join(" ")),
    error: (...a) => __aosh_log("error", a.map(String).join(" ")),
};

var __aosh_later = [];
