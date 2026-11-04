// Qt's `Locale`, and the dates and numbers a QML file formats with it:
// `Qt.locale().toString(date, "dddd, dd/MM")`, `Qt.formatDateTime`,
// `date.toLocaleDateString(Qt.locale(), "MMM")`
//
// One locale, en_US's (names, formats, separators), under whatever name `$LANG`
// gives it. That is what Qt says on Arch, which end4 targets, and MikuOS has no
// locale data of its own (`dev/aosh-negi.md`). With no `$LANG` the name is
// `en_US`, where Qt says `C` for the same English.

var Locale = {
    LongFormat: 0, ShortFormat: 1, NarrowFormat: 2,
    // A day of the week as JavaScript's Date numbers it, as QML's Locale does;
    // C++'s Qt::DayOfWeek has Sunday as 7
    Sunday: 0, Monday: 1, Tuesday: 2, Wednesday: 3, Thursday: 4, Friday: 5, Saturday: 6,
    MetricSystem: 0, ImperialUSSystem: 1, ImperialUKSystem: 2,
};

var __aosh_days = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
var __aosh_months = ["January", "February", "March", "April", "May", "June", "July",
                     "August", "September", "October", "November", "December"];

// en_US's formats, as Qt 6 has them from CLDR, by Locale.LongFormat,
// ShortFormat, NarrowFormat
var __aosh_formats = {
    date: ["dddd, MMMM d, yyyy", "M/d/yy", "M/d/yy"],
    time: ["h:mm:ss AP t", "h:mm AP", "h:mm AP"],
};

// A date in one of Qt's format strings. The letters are Qt's: d to dddd, M
// to MMMM, yy and yyyy, h and hh (twelve-hour when the format has AP), H and
// HH, m, mm, s, ss, z, zzz, AP, ap, A, a, t. Text in single quotes is as it
// is, and '' is a quote
function __aosh_format_date(d, fmt) {
    if (!(d instanceof Date)) d = new Date(d);
    // Qt's answer for a date that is not one: nothing, not "NaN:NaN"
    if (isNaN(d.getTime())) return "";
    fmt = String(fmt);
    const p = (n, w) => String(n).padStart(w, "0");
    // Twelve-hour if an AM/PM marker is anywhere outside the quotes
    const twelve = /a/i.test(fmt.replace(/'[^']*'/g, ""));
    const hours = d.getHours();
    const h12 = ((hours + 11) % 12) + 1;
    const run = (i, c, max) => { let n = 1; while (n < max && fmt[i + n] === c) n++; return n; };
    let out = "";
    for (let i = 0; i < fmt.length;) {
        const c = fmt[i];
        if (c === "'") {
            if (fmt[i + 1] === "'") { out += "'"; i += 2; continue; }
            const end = fmt.indexOf("'", i + 1);
            const stop = end < 0 ? fmt.length : end;
            out += fmt.slice(i + 1, stop);
            i = stop + 1;
            continue;
        }
        let n;
        switch (c) {
            case "d":
                n = run(i, c, 4);
                out += n === 1 ? d.getDate() : n === 2 ? p(d.getDate(), 2)
                    : n === 3 ? __aosh_days[d.getDay()].slice(0, 3) : __aosh_days[d.getDay()];
                break;
            case "M":
                n = run(i, c, 4);
                out += n === 1 ? d.getMonth() + 1 : n === 2 ? p(d.getMonth() + 1, 2)
                    : n === 3 ? __aosh_months[d.getMonth()].slice(0, 3) : __aosh_months[d.getMonth()];
                break;
            case "y":
                n = run(i, c, 4);
                if (n === 4) out += p(d.getFullYear(), 4);
                else if (n >= 2) { n = 2; out += p(d.getFullYear() % 100, 2); }
                else out += "y";
                break;
            case "h":
                n = run(i, c, 2);
                out += p(twelve ? h12 : hours, n);
                break;
            case "H":
                n = run(i, c, 2);
                out += p(hours, n);
                break;
            case "m":
                n = run(i, c, 2);
                out += p(d.getMinutes(), n);
                break;
            case "s":
                n = run(i, c, 2);
                out += p(d.getSeconds(), n);
                break;
            case "z":
                n = run(i, c, 3);
                out += n === 3 ? p(d.getMilliseconds(), 3) : String(d.getMilliseconds());
                break;
            case "A": case "a": {
                const pm = hours >= 12;
                n = (fmt[i + 1] === "P" || fmt[i + 1] === "p") ? 2 : 1;
                const s = pm ? "PM" : "AM";
                out += c === "A" ? s : s.toLowerCase();
                break;
            }
            case "t": {
                n = 1;
                // Qt's is the zone's abbreviation, which there is no table
                // for here: the offset says the same
                const off = -d.getTimezoneOffset();
                if (off === 0) out += "UTC";
                else {
                    const a = Math.abs(off);
                    out += "UTC" + (off < 0 ? "-" : "+") + Math.floor(a / 60) + (a % 60 ? ":" + p(a % 60, 2) : "");
                }
                break;
            }
            default:
                n = 1;
                out += c;
        }
        i += n;
    }
    return out;
}

// `toLocaleTimeString("en-US", { hour: "2-digit", minute: "2-digit" })`:
// ECMAScript's form, which QuickJS has no Intl for. en-US's answer, from
// the options asked for
function __aosh_intl_time(d, o) {
    const parts = [];
    const two = (k) => o[k] === "2-digit";
    if (o.hour !== undefined) parts.push(o.hour12 === false ? (two("hour") ? "HH" : "H") : (two("hour") ? "hh" : "h"));
    if (o.minute !== undefined) parts.push("mm");
    if (o.second !== undefined) parts.push("ss");
    if (parts.length === 0) parts.push("h", "mm", "ss");
    let fmt = parts.join(":");
    if (o.hour12 !== false && o.hour !== undefined) fmt += " AP";
    return __aosh_format_date(d, fmt);
}

// A number as en_US writes one: `toString(1234.5, 'f', 2)` is `1,234.50`
function __aosh_format_number(v, fmt, prec) {
    v = Number(v);
    if (!isFinite(v)) return String(v);
    fmt = fmt === undefined ? "g" : String(fmt);
    let s;
    if (fmt === "f" || fmt === "F") s = v.toFixed(prec === undefined ? 6 : prec);
    else if (fmt === "e" || fmt === "E") s = v.toExponential(prec === undefined ? 6 : prec);
    else s = prec === undefined ? String(v) : String(Number(v.toPrecision(Math.max(1, prec))));
    if (fmt === "E") s = s.toUpperCase();
    const m = /^(-?)(\d+)(.*)$/.exec(s);
    if (m === null) return s;
    return m[1] + m[2].replace(/\B(?=(\d{3})+(?!\d))/g, ",") + m[3];
}

function __aosh_locale(name) {
    const fmt = (kind, t) => __aosh_formats[kind][t === undefined ? 0 : t] || __aosh_formats[kind][0];
    return {
        name,
        uiLanguages: [name.replace("_", "-")],
        nativeLanguageName: "American English",
        nativeTerritoryName: "United States",
        amText: "AM", pmText: "PM",
        decimalPoint: ".", groupSeparator: ",", zeroDigit: "0", negativeSign: "-", positiveSign: "+",
        percent: "%", exponential: "e",
        firstDayOfWeek: Locale.Sunday,
        weekDays: [1, 2, 3, 4, 5],
        measurementSystem: Locale.ImperialUSSystem,
        textDirection: 0,
        dateFormat(t) { return fmt("date", t); },
        timeFormat(t) { return fmt("time", t); },
        dateTimeFormat(t) { return fmt("date", t) + " " + fmt("time", t); },
        dayName(i, t) { const n = __aosh_days[i % 7]; return t === Locale.LongFormat || t === undefined ? n : n.slice(0, 3); },
        standaloneDayName(i, t) { return this.dayName(i, t); },
        monthName(i, t) { const n = __aosh_months[i % 12]; return t === Locale.LongFormat || t === undefined ? n : n.slice(0, 3); },
        standaloneMonthName(i, t) { return this.monthName(i, t); },
        toString(v, f, prec) {
            if (v instanceof Date) {
                return __aosh_format_date(v, typeof f === "string" ? f : this.dateTimeFormat(f));
            }
            return __aosh_format_number(v, f, prec);
        },
        formattedDataSize(bytes, prec) {
            prec = prec === undefined ? 2 : prec;
            const units = ["bytes", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
            let i = 0, b = Number(bytes);
            while (Math.abs(b) >= 1024 && i < units.length - 1) { b /= 1024; i++; }
            return i === 0 ? `${b} bytes` : `${b.toFixed(prec)} ${units[i]}`;
        },
    };
}

// What `$LANG` names, `en_US.UTF-8` being `en_US`
var __aosh_default_locale = null;
function __aosh_system_locale() {
    if (__aosh_default_locale === null) {
        const lang = __aosh_env("LC_ALL") || __aosh_env("LC_TIME") || __aosh_env("LANG") || "";
        const name = lang.split(/[.@]/)[0];
        __aosh_default_locale = __aosh_locale(name === "" || name === "C" || name === "POSIX" ? "en_US" : name);
    }
    return __aosh_default_locale;
}

// Qt's additions to Date and Number: a Locale first, then a format (a string,
// or Locale.LongFormat and the rest)
(function () {
    const date = (kind) => function (locale, f) {
        // ECMAScript's own form, a language tag and Intl options
        if (typeof locale === "string" || (f !== null && typeof f === "object")) {
            const o = (f !== null && typeof f === "object") ? f : {};
            if (kind === "time") return __aosh_intl_time(this, o);
        }
        const l = (locale && typeof locale === "object" && typeof locale.dateFormat === "function")
            ? locale : __aosh_system_locale();
        if (typeof f === "string") return __aosh_format_date(this, f);
        if (kind === "date") return __aosh_format_date(this, l.dateFormat(f));
        if (kind === "time") return __aosh_format_date(this, l.timeFormat(f));
        return __aosh_format_date(this, l.dateTimeFormat(f));
    };
    Date.prototype.toLocaleDateString = date("date");
    Date.prototype.toLocaleTimeString = date("time");
    Date.prototype.toLocaleString = date("both");
    Number.prototype.toLocaleString = function (locale, f, prec) {
        return __aosh_format_number(this, f === undefined ? "f" : f, prec === undefined ? 2 : prec);
    };
})();
