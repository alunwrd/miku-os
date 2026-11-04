// Parsed QML tree. JavaScript values stay as source text with their location,
// so QuickJS errors map back to it

pub use super::lexer::Loc;

#[derive(Debug, Clone, Default)]
pub struct Document {
    /// `pragma Singleton`, `pragma ComponentBehavior: Bound`, as written
    pub pragmas: Vec<String>,
    pub imports: Vec<Import>,
    /// `component Name: Type { }`
    pub inline: Vec<(String, ObjDef)>,
    /// `enum Shape { Circle, Square = 3 }`, numbered from zero or from the last
    /// value given
    pub enums: Vec<(String, Vec<(String, i64)>)>,
    pub root: ObjDef,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Import {
    /// A module (`QtQuick`, `Quickshell.Io`) or a quoted path (`"widgets"`)
    pub what: String,
    pub is_path: bool,
    pub version: Option<String>,
    pub alias: Option<String>,
    pub loc: Loc,
}

#[derive(Debug, Clone, Default)]
pub struct ObjDef {
    /// The type as written: `Rectangle`, `Bar`, `QtQuick.Item`
    pub ty: String,
    pub id: Option<String>,
    pub members: Vec<Member>,
    pub loc: Loc,
}

// Kept for diagnostics and default properties
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum Member {
    /// `name: value`, including `anchors.left: ...`, `Layout.fillWidth: ...`,
    /// group members prefixed with the group's name, and handlers
    Assign { name: String, value: Value, loc: Loc },
    /// `property <type> name[: value]`
    Property { name: String, ty: String, value: Option<Value>, default: bool, readonly: bool, required: bool, loc: Loc },
    /// `property alias name: target.path`. `default` receives an instance's
    /// children (`default property alias data: box.data`)
    Alias { name: String, target: String, default: bool, loc: Loc },
    /// `signal name(params)`
    Signal { name: String, params: Vec<String>, loc: Loc },
    /// `function name(params) { body }`, the body with braces. `types` and
    /// `ret` are declared types (`""` if absent), used to convert `IpcHandler`
    /// arguments (`ipc/`)
    Function { name: String, params: Vec<String>, types: Vec<String>, ret: String, body: String, loc: Loc },
    /// A child object
    Child(ObjDef),
    /// `Behavior on width { ... }`: an object applied to one property
    On { prop: String, obj: ObjDef, loc: Loc },
}

#[derive(Debug, Clone)]
pub enum Value {
    /// JavaScript: an expression, or a `{ ... }` block returning one
    Js { src: String, loc: Loc },
    Object(ObjDef),
    Objects(Vec<ObjDef>),
}

impl Value {
    /// The value as a plain literal, set once at creation with no binding
    pub fn literal(&self) -> Option<Literal> {
        let Value::Js { src, .. } = self else { return None };
        let s = src.trim();
        if s == "true" {
            return Some(Literal::Bool(true));
        }
        if s == "false" {
            return Some(Literal::Bool(false));
        }
        if let Ok(n) = s.parse::<f64>() {
            return Some(Literal::Number(n));
        }
        if let Some(rest) = s.strip_prefix('-') {
            if let Ok(n) = rest.trim().parse::<f64>() {
                return Some(Literal::Number(-n));
            }
        }
        // A quoted string with no escapes or inner quotes; others go through
        // QuickJS
        for q in ['"', '\''] {
            if s.len() >= 2 && s.starts_with(q) && s.ends_with(q) {
                let inner = &s[1..s.len() - 1];
                if !inner.contains(q) && !inner.contains('\\') {
                    return Some(Literal::Str(inner.to_string()));
                }
            }
        }
        None
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Bool(bool),
    Number(f64),
    Str(String),
}
