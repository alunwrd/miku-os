// Property values: native where aosh reads them (numbers, colors), a kept
// JavaScript value otherwise (arrays, functions, plain objects)

use std::rc::Rc;

use rquickjs::Persistent;

/// Index into the engine's object table
pub type Oid = u32;

#[derive(Clone)]
pub enum Value {
    Undefined,
    Null,
    Bool(bool),
    Num(f64),
    Str(Rc<str>),
    Obj(Oid),
    /// A list of objects, as `property list<Item>` or `[ A {}, B {} ]` hold
    List(Rc<Vec<Value>>),
    /// Other JavaScript values: arrays, plain objects, functions
    Js(Rc<Persistent<rquickjs::Value<'static>>>),
}

impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Undefined => write!(f, "undefined"),
            Value::Null => write!(f, "null"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Num(n) => write!(f, "{n}"),
            Value::Str(s) => write!(f, "{s:?}"),
            Value::Obj(o) => write!(f, "object#{o}"),
            Value::List(l) => write!(f, "{l:?}"),
            Value::Js(_) => write!(f, "<js>"),
        }
    }
}

impl Value {
    pub fn str(s: &str) -> Value {
        Value::Str(Rc::from(s))
    }

    pub fn as_num(&self) -> f64 {
        match self {
            Value::Num(n) => *n,
            Value::Bool(b) => *b as u8 as f64,
            Value::Str(s) => s.trim().parse().unwrap_or(f64::NAN),
            Value::Null => 0.0,
            _ => f64::NAN,
        }
    }

    pub fn truthy(&self) -> bool {
        match self {
            Value::Undefined | Value::Null => false,
            Value::Bool(b) => *b,
            Value::Num(n) => *n != 0.0 && !n.is_nan(),
            Value::Str(s) => !s.is_empty(),
            _ => true,
        }
    }

    pub fn as_obj(&self) -> Option<Oid> {
        match self {
            Value::Obj(o) => Some(*o),
            _ => None,
        }
    }

    /// List contents; a single object is a list of one, as in QML list
    /// assignment
    pub fn as_list(&self) -> Option<Rc<Vec<Value>>> {
        match self {
            Value::List(l) => Some(l.clone()),
            Value::Obj(_) => Some(Rc::new(vec![self.clone()])),
            _ => None,
        }
    }

    /// Text form as JavaScript's `String()`: `3`, not `3.0`
    pub fn to_text(&self) -> String {
        match self {
            Value::Undefined => "undefined".into(),
            Value::Null => "null".into(),
            Value::Bool(b) => b.to_string(),
            Value::Num(n) => num_text(*n),
            Value::Str(s) => s.to_string(),
            Value::Obj(o) => format!("object#{o}"),
            Value::List(_) => "[list]".into(),
            Value::Js(_) => "[object]".into(),
        }
    }
}

pub fn num_text(n: f64) -> String {
    if n.is_nan() {
        "NaN".into()
    } else if n.is_infinite() {
        if n > 0.0 { "Infinity".into() } else { "-Infinity".into() }
    } else if n == n.trunc() && n.abs() < 1e21 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// What a property may hold, and what a value is turned into on the way in
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropType {
    Var,
    Bool,
    Int,
    Real,
    Str,
    Color,
    Obj,
    List,
}

impl PropType {
    /// The QML spelling of a declared property's type
    pub fn parse(ty: &str) -> PropType {
        match ty {
            "int" => PropType::Int,
            "real" | "double" | "number" => PropType::Real,
            "bool" => PropType::Bool,
            "string" | "url" => PropType::Str,
            "color" => PropType::Color,
            "var" | "variant" | "" | "date" | "point" | "size" | "rect" | "font" => PropType::Var,
            t if t.starts_with("list<") => PropType::List,
            t if t.chars().next().is_some_and(|c| c.is_ascii_uppercase()) => PropType::Obj,
            _ => PropType::Var,
        }
    }

    /// What a declared property starts as when nothing is assigned
    pub fn default_value(self) -> Value {
        match self {
            PropType::Var => Value::Undefined,
            PropType::Bool => Value::Bool(false),
            PropType::Int | PropType::Real => Value::Num(0.0),
            PropType::Str => Value::str(""),
            PropType::Color => Value::str("#00000000"),
            PropType::Obj => Value::Null,
            PropType::List => Value::List(Rc::new(Vec::new())),
        }
    }

    /// Narrows `v` to this type. JavaScript-side conversions happen earlier
    pub fn coerce(self, v: Value) -> Value {
        match self {
            PropType::Var => v,
            PropType::Bool => Value::Bool(v.truthy()),
            PropType::Int => {
                let n = v.as_num();
                // ToInt32 approximation: toward zero, NaN is 0
                Value::Num(if n.is_finite() { n.trunc() } else { 0.0 })
            }
            PropType::Real => Value::Num(v.as_num()),
            PropType::Str => match v {
                Value::Str(_) => v,
                Value::Undefined | Value::Null => Value::str(""),
                other => Value::str(&other.to_text()),
            },
            PropType::Color => match &v {
                Value::Str(s) => Value::str(&normalise_colour(s)),
                _ => v,
            },
            PropType::Obj => match v {
                Value::Obj(_) | Value::Null => v,
                Value::Undefined => Value::Null,
                _ => Value::Null,
            },
            PropType::List => match v {
                Value::List(_) | Value::Js(_) => v,
                Value::Obj(_) => Value::List(Rc::new(vec![v])),
                _ => Value::List(Rc::new(Vec::new())),
            },
        }
    }
}

/// Normalizes a QML color (`"#rgb"`, `"#rrggbb"`, `"#aarrggbb"`, or a name) to
/// lowercase `#aarrggbb`. Unknown names stay as written and are reported when
/// drawn
pub fn normalise_colour(s: &str) -> String {
    let t = s.trim();
    if let Some(hex) = t.strip_prefix('#') {
        let ok = hex.chars().all(|c| c.is_ascii_hexdigit());
        let h = hex.to_ascii_lowercase();
        return match (ok, h.len()) {
            (true, 3) => {
                let c: Vec<char> = h.chars().collect();
                format!("#ff{0}{0}{1}{1}{2}{2}", c[0], c[1], c[2])
            }
            (true, 4) => {
                let c: Vec<char> = h.chars().collect();
                format!("#{0}{0}{1}{1}{2}{2}{3}{3}", c[0], c[1], c[2], c[3])
            }
            (true, 6) => format!("#ff{h}"),
            (true, 8) => format!("#{h}"),
            _ => t.to_string(),
        };
    }
    let name = t.to_ascii_lowercase();
    if name == "transparent" {
        return "#00000000".into();
    }
    // SVG names, all accepted by `QColor::fromString`
    match SVG_COLOURS.binary_search_by(|(n, _)| (*n).cmp(name.as_str())) {
        Ok(i) => format!("#ff{:06x}", SVG_COLOURS[i].1),
        Err(_) => t.to_string(),
    }
}

/// The SVG colour keywords, sorted for the search above
pub const SVG_COLOURS: &[(&str, u32)] = &[
    ("aliceblue", 0xf0f8ff), ("antiquewhite", 0xfaebd7), ("aqua", 0x00ffff), ("aquamarine", 0x7fffd4),
    ("azure", 0xf0ffff), ("beige", 0xf5f5dc), ("bisque", 0xffe4c4), ("black", 0x000000),
    ("blanchedalmond", 0xffebcd), ("blue", 0x0000ff), ("blueviolet", 0x8a2be2), ("brown", 0xa52a2a),
    ("burlywood", 0xdeb887), ("cadetblue", 0x5f9ea0), ("chartreuse", 0x7fff00), ("chocolate", 0xd2691e),
    ("coral", 0xff7f50), ("cornflowerblue", 0x6495ed), ("cornsilk", 0xfff8dc), ("crimson", 0xdc143c),
    ("cyan", 0x00ffff), ("darkblue", 0x00008b), ("darkcyan", 0x008b8b), ("darkgoldenrod", 0xb8860b),
    ("darkgray", 0xa9a9a9), ("darkgreen", 0x006400), ("darkgrey", 0xa9a9a9), ("darkkhaki", 0xbdb76b),
    ("darkmagenta", 0x8b008b), ("darkolivegreen", 0x556b2f), ("darkorange", 0xff8c00), ("darkorchid", 0x9932cc),
    ("darkred", 0x8b0000), ("darksalmon", 0xe9967a), ("darkseagreen", 0x8fbc8f), ("darkslateblue", 0x483d8b),
    ("darkslategray", 0x2f4f4f), ("darkslategrey", 0x2f4f4f), ("darkturquoise", 0x00ced1), ("darkviolet", 0x9400d3),
    ("deeppink", 0xff1493), ("deepskyblue", 0x00bfff), ("dimgray", 0x696969), ("dimgrey", 0x696969),
    ("dodgerblue", 0x1e90ff), ("firebrick", 0xb22222), ("floralwhite", 0xfffaf0), ("forestgreen", 0x228b22),
    ("fuchsia", 0xff00ff), ("gainsboro", 0xdcdcdc), ("ghostwhite", 0xf8f8ff), ("gold", 0xffd700),
    ("goldenrod", 0xdaa520), ("gray", 0x808080), ("green", 0x008000), ("greenyellow", 0xadff2f),
    ("grey", 0x808080), ("honeydew", 0xf0fff0), ("hotpink", 0xff69b4), ("indianred", 0xcd5c5c),
    ("indigo", 0x4b0082), ("ivory", 0xfffff0), ("khaki", 0xf0e68c), ("lavender", 0xe6e6fa),
    ("lavenderblush", 0xfff0f5), ("lawngreen", 0x7cfc00), ("lemonchiffon", 0xfffacd), ("lightblue", 0xadd8e6),
    ("lightcoral", 0xf08080), ("lightcyan", 0xe0ffff), ("lightgoldenrodyellow", 0xfafad2), ("lightgray", 0xd3d3d3),
    ("lightgreen", 0x90ee90), ("lightgrey", 0xd3d3d3), ("lightpink", 0xffb6c1), ("lightsalmon", 0xffa07a),
    ("lightseagreen", 0x20b2aa), ("lightskyblue", 0x87cefa), ("lightslategray", 0x778899), ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xb0c4de), ("lightyellow", 0xffffe0), ("lime", 0x00ff00), ("limegreen", 0x32cd32),
    ("linen", 0xfaf0e6), ("magenta", 0xff00ff), ("maroon", 0x800000), ("mediumaquamarine", 0x66cdaa),
    ("mediumblue", 0x0000cd), ("mediumorchid", 0xba55d3), ("mediumpurple", 0x9370db), ("mediumseagreen", 0x3cb371),
    ("mediumslateblue", 0x7b68ee), ("mediumspringgreen", 0x00fa9a), ("mediumturquoise", 0x48d1cc), ("mediumvioletred", 0xc71585),
    ("midnightblue", 0x191970), ("mintcream", 0xf5fffa), ("mistyrose", 0xffe4e1), ("moccasin", 0xffe4b5),
    ("navajowhite", 0xffdead), ("navy", 0x000080), ("oldlace", 0xfdf5e6), ("olive", 0x808000),
    ("olivedrab", 0x6b8e23), ("orange", 0xffa500), ("orangered", 0xff4500), ("orchid", 0xda70d6),
    ("palegoldenrod", 0xeee8aa), ("palegreen", 0x98fb98), ("paleturquoise", 0xafeeee), ("palevioletred", 0xdb7093),
    ("papayawhip", 0xffefd5), ("peachpuff", 0xffdab9), ("peru", 0xcd853f), ("pink", 0xffc0cb),
    ("plum", 0xdda0dd), ("powderblue", 0xb0e0e6), ("purple", 0x800080), ("red", 0xff0000),
    ("rosybrown", 0xbc8f8f), ("royalblue", 0x4169e1), ("saddlebrown", 0x8b4513), ("salmon", 0xfa8072),
    ("sandybrown", 0xf4a460), ("seagreen", 0x2e8b57), ("seashell", 0xfff5ee), ("sienna", 0xa0522d),
    ("silver", 0xc0c0c0), ("skyblue", 0x87ceeb), ("slateblue", 0x6a5acd), ("slategray", 0x708090),
    ("slategrey", 0x708090), ("snow", 0xfffafa), ("springgreen", 0x00ff7f), ("steelblue", 0x4682b4),
    ("tan", 0xd2b48c), ("teal", 0x008080), ("thistle", 0xd8bfd8), ("tomato", 0xff6347),
    ("turquoise", 0x40e0d0), ("violet", 0xee82ee), ("wheat", 0xf5deb3), ("white", 0xffffff),
    ("whitesmoke", 0xf5f5f5), ("yellow", 0xffff00), ("yellowgreen", 0x9acd32),
];
