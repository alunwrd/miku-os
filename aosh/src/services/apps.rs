// DesktopEntries: launchable programs (quickshell's `DesktopEntries`)
//
// Reads `/etc/mayland/apps.d`, one program per file in name order, in
// `libmay::conf` syntax:
//
//     app {
//         name = Терминал
//         icon = /etc/mayland/icons/terminal.qoi
//         exec = /bin/mayterm
//     }
//
// `applications.values` and `applications` as a model list the entries.
// `execute()` starts a program without `CAP_MAY_PRIV`, as
// `Quickshell.execDetached` does.

use std::path::Path;

use libmay::conf::{self, Note, Sink, Where};
use rquickjs::{Array, Ctx, Function, Object};

use crate::object::value::{Oid, Value};
use crate::object::Core;
use crate::types::Kind;

/// Where the programs are described
pub const APPS_D: &str = "/etc/mayland/apps.d";

#[derive(Clone, Default, Debug, PartialEq)]
pub struct Entry {
    /// The file's name without `.conf`, as a `.desktop` file's is its id
    pub id: String,
    pub name: String,
    pub generic_name: String,
    pub comment: String,
    pub icon: String,
    pub exec: String,
    pub categories: Vec<String>,
    pub keywords: Vec<String>,
    pub terminal: bool,
    pub hidden: bool,
}

/// Programs in `dir` and errors for the rest
pub fn read(dir: &Path) -> (Vec<Entry>, Vec<String>) {
    let mut names: Vec<_> = std::fs::read_dir(dir).into_iter().flatten().flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "conf"))
        .collect();
    names.sort();
    let mut out = Vec::new();
    let mut notes = Vec::new();
    for path in names {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let mut one = One {
            entry: Entry { id: path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), ..Entry::default() },
            notes: Vec::new(),
            file: path.display().to_string(),
        };
        conf::parse_into(&text, &mut one, &conf::Vars::new(), 0);
        notes.append(&mut one.notes);
        // Skips a block with no command
        if !one.entry.name.is_empty() && !one.entry.exec.is_empty() {
            out.push(one.entry);
        }
    }
    (out, notes)
}

/// The file being read
struct One {
    entry: Entry,
    notes: Vec<String>,
    file: String,
}

impl Sink for One {
    fn key(&mut self, at: Where, section: &str, key: &str, value: &str) {
        // Ignores other block types
        if section != "app" {
            return;
        }
        let value = value.trim().to_string();
        let list = |v: &str| v.split(';').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect();
        match key {
            "name" | "title" => self.entry.name = value,
            "generic_name" => self.entry.generic_name = value,
            "comment" => self.entry.comment = value,
            "icon" => self.entry.icon = value,
            "exec" | "command" => self.entry.exec = value,
            "categories" => self.entry.categories = list(&value),
            "keywords" => self.entry.keywords = list(&value),
            "terminal" => self.entry.terminal = matches!(value.as_str(), "yes" | "true" | "on"),
            "hidden" => self.entry.hidden = matches!(value.as_str(), "yes" | "true" | "on"),
            _ => self.notes.push(format!("{}:{}: a program has no setting '{key}'", self.file, at.line)),
        }
    }

    fn note(&mut self, at: Where, note: Note, text: &str) {
        self.notes.push(format!("{}:{}: {} {text}", self.file, at.line, note.as_str()));
    }
}

/// `DesktopEntries.applications`, made from `dir`
pub fn publish(ctx: &Ctx<'_>, core: &Core, entries: Oid, dir: &Path) {
    let (list, notes) = read(dir);
    for n in notes {
        core.log(n);
    }
    let made = (|| -> rquickjs::Result<rquickjs::Value> {
        let arr = Array::new(ctx.clone())?;
        for (i, e) in list.iter().enumerate() {
            let o = Object::new(ctx.clone())?;
            o.set("id", e.id.as_str())?;
            o.set("name", e.name.as_str())?;
            o.set("genericName", e.generic_name.as_str())?;
            o.set("comment", e.comment.as_str())?;
            o.set("icon", e.icon.as_str())?;
            o.set("execString", e.exec.as_str())?;
            o.set("command", e.exec.split_whitespace().collect::<Vec<_>>())?;
            o.set("workingDirectory", "")?;
            o.set("runInTerminal", e.terminal)?;
            o.set("noDisplay", e.hidden)?;
            o.set("categories", e.categories.clone())?;
            o.set("keywords", e.keywords.clone())?;
            o.set("startupClass", "")?;
            o.set("actions", Array::new(ctx.clone())?)?;
            arr.set(i, o)?;
        }
        let wrap: Function = ctx.globals().get("__aosh_entries")?;
        wrap.call((arr,))
    })();
    match made {
        Ok(v) => core.set_layout_value(ctx, entries, "applications", crate::js::from_js(ctx, v)),
        Err(e) => core.log(format!("DesktopEntries: {}", crate::js::describe(ctx, e))),
    }
}

/// `byId(id)` and `heuristicLookup(name)`: an entry or null
pub fn call<'js>(ctx: &Ctx<'js>, core: &Core, oid: Oid, kind: Kind, name: &str, args: &[rquickjs::Value<'js>]) -> Option<Value> {
    if kind != Kind::Apps {
        return None;
    }
    let want = args.first().and_then(|v| v.as_string()).and_then(|s| s.to_string().ok()).unwrap_or_default();
    let values = (|| -> Option<Array<'js>> {
        let v = crate::js::to_js(ctx, core, &core.peek_named(oid, "applications")?).ok()?;
        v.as_object()?.get::<_, Array>("values").ok()
    })();
    let Some(values) = values else { return Some(Value::Null) };
    let find = |pred: &dyn Fn(&Object<'js>) -> bool| -> Value {
        values.iter::<Object>().flatten().find(|o| pred(o))
            .map(|o| crate::js::from_js(ctx, o.into_value()))
            .unwrap_or(Value::Null)
    };
    let field = |o: &Object<'js>, f: &str| o.get::<_, String>(f).unwrap_or_default();
    Some(match name {
        "byId" => find(&|o| field(o, "id") == want),
        // Matches a window's name: id, name, or program, case-insensitive
        "heuristicLookup" => {
            let w = want.to_lowercase();
            let prog = |o: &Object<'js>| {
                let e = field(o, "execString");
                let first = e.split_whitespace().next().unwrap_or("").to_string();
                first.rsplit('/').next().unwrap_or("").to_lowercase()
            };
            find(&|o| field(o, "id").to_lowercase() == w || field(o, "name").to_lowercase() == w || prog(o) == w)
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apps_d_is_read_in_order_and_unfinished_blocks_are_left_out() {
        let dir = std::env::temp_dir().join(format!("aosh-apps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("20-zed.conf"), "app {\n    name = Zed\n    exec = /bin/zed --new\n    categories = Development; Editor\n}\n").unwrap();
        std::fs::write(dir.join("10-terminal.conf"), "app {\n    name = Терминал\n    icon = /etc/t.qoi\n    exec = /bin/mayterm\n}\n").unwrap();
        std::fs::write(dir.join("30-half.conf"), "app {\n    name = Nothing yet\n}\n").unwrap();
        std::fs::write(dir.join("40-odd.conf"), "app {\n    name = Odd\n    exec = /bin/odd\n    colour = red\n}\n").unwrap();
        let (list, notes) = read(&dir);
        let names: Vec<&str> = list.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Терминал", "Zed", "Odd"]);
        assert_eq!(list[1].id, "20-zed");
        assert_eq!(list[1].categories, ["Development", "Editor"]);
        assert_eq!(list[0].icon, "/etc/t.qoi");
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("colour"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
