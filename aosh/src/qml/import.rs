// File imports, following quickshell's layout
//
// The directory of `shell.qml` is the module `qs`, and each directory under it
// a module (`qs.modules.common` is `<root>/modules/common`). A module has one
// type per `Name.qml`; files with `pragma Singleton` are single objects created
// on first use.
//
// A file sees, in order:
// - its own directory;
// - unaliased imports (`qs.*` or a quoted path);
// - aliased imports under their alias
//   (`import qs.modules.common.functions as CF`);
// - JavaScript files under their alias (`import "shapes/offset.js" as Offset`).
//
// Other modules (`QtQuick`, `Quickshell.Io`) are built-in types available
// everywhere.
//
// Deviation: singletons are global, while Qt limits them to importing files.
// Everything that loads in Qt still resolves

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::ast::Document;

/// Shell directory contents: singletons and types with enums
#[derive(Default)]
pub struct Modules {
    /// The directory `qs` is
    pub root: PathBuf,
    /// Singleton name and file. The first in sorted order wins; others go to
    /// `clashes`
    pub singletons: HashMap<String, PathBuf>,
    /// Type name and its enums, for `Type.Enum.Value` and `Type.Value`
    pub enums: Vec<(String, Vec<(String, Vec<(String, i64)>)>)>,
    /// Two singletons of one name, which a global cannot be both of
    pub clashes: Vec<(String, PathBuf)>,
}

impl Modules {
    /// Scans every `.qml` under `root` for `pragma Singleton` and `enum`,
    /// parsing only files that may have either
    pub fn scan(root: &Path) -> Modules {
        let mut m = Modules { root: root.to_path_buf(), ..Modules::default() };
        let mut files = Vec::new();
        qml_files(root, &mut files);
        files.sort();
        for f in files {
            let Ok(src) = std::fs::read_to_string(&f) else { continue };
            let singleton = header(&src).contains("pragma Singleton");
            if !singleton && !src.contains("enum ") {
                continue;
            }
            let Some(stem) = f.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
            if singleton {
                if m.singletons.contains_key(&stem) {
                    m.clashes.push((stem.clone(), f.clone()));
                } else {
                    m.singletons.insert(stem.clone(), f.clone());
                }
            }
            if src.contains("enum ") {
                if let Ok(doc) = super::parse(&src) {
                    if !doc.enums.is_empty() {
                        m.enums.push((stem, doc.enums));
                    }
                }
            }
        }
        m
    }

    /// The directory a `qs.a.b` import names, if it is one
    pub fn module_dir(&self, module: &str) -> Option<PathBuf> {
        let rest = match module {
            "qs" => "",
            m => m.strip_prefix("qs.")?,
        };
        let mut d = self.root.clone();
        for part in rest.split('.').filter(|p| !p.is_empty()) {
            d.push(part);
        }
        Some(d)
    }

    /// The singletons whose file is in `dir`, for an aliased import of it
    pub fn singletons_in(&self, dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = self.singletons.iter()
            .filter(|(_, p)| p.parent() == Some(dir))
            .map(|(n, _)| n.clone())
            .collect();
        v.sort();
        v
    }
}

/// What one file's imports give it
#[derive(Default, Clone, Debug)]
pub struct Imports {
    /// Type search path, the file's directory first
    pub dirs: Vec<PathBuf>,
    /// `import "x.js" as X`
    pub js: Vec<(String, PathBuf)>,
    /// `import qs.a as A`, `import "dir" as A`: alias and directory
    pub spaces: Vec<(String, PathBuf)>,
}

/// The imports of `doc`, a file in `dir`, against the shell's modules
pub fn imports(doc: &Document, dir: &Path, modules: Option<&Modules>) -> Imports {
    let mut out = Imports { dirs: vec![dir.to_path_buf()], ..Imports::default() };
    for i in &doc.imports {
        let target = if i.is_path {
            Some(dir.join(&i.what))
        } else {
            modules.and_then(|m| m.module_dir(&i.what))
        };
        let Some(target) = target else { continue };
        let is_js = i.is_path && i.what.ends_with(".js");
        match (&i.alias, is_js) {
            (Some(a), true) => out.js.push((a.clone(), target)),
            (Some(a), false) => out.spaces.push((a.clone(), target)),
            // A script imported with no alias is unreachable
            (None, true) => {}
            (None, false) => {
                if !out.dirs.contains(&target) {
                    out.dirs.push(target);
                }
            }
        }
    }
    out
}

/// quickshell pragmas before the root object, `//@ pragma Name args`:
/// `NativeTextRendering`, `Env QT_…=…`
pub fn shell_pragmas(src: &str) -> Vec<String> {
    header(src).lines().filter_map(|l| l.trim().strip_prefix("//@ pragma ")).map(|p| p.trim().to_string()).collect()
}

/// The file up to its root object, where pragmas are
fn header(src: &str) -> &str {
    let end = src.find('{').unwrap_or(src.len());
    &src[..end]
}

/// Every `.qml` at or under `dir`, skipping nested shells (directories with
/// their own `shell.qml`)
fn qml_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        if p.is_dir() {
            if p.join("shell.qml").is_file() {
                continue;
            }
            qml_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "qml") {
            out.push(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(files: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("aosh-import-{}-{}", std::process::id(),
            files.len() * 7 + files.first().map_or(0, |f| f.0.len())));
        let _ = std::fs::remove_dir_all(&root);
        for (name, src) in files {
            let p = root.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, src).unwrap();
        }
        root
    }

    #[test]
    fn modules_singletons_and_enums() {
        let root = tree(&[
            ("shell.qml", "import qs.modules.common\nItem {}"),
            ("modules/common/Appearance.qml", "pragma Singleton\nimport QtQuick\nQtObject { property int r: 3 }"),
            ("modules/common/functions/ColorUtils.qml", "pragma Singleton\nQtObject {}"),
            ("modules/common/widgets/MaterialShape.qml", "Item { enum Shape { Circle, Square, Gem = 7, Pill } }"),
            ("modules/common/widgets/Plain.qml", "Item {}"),
        ]);
        let m = Modules::scan(&root);
        assert_eq!(m.singletons.len(), 2);
        assert!(m.singletons["Appearance"].ends_with("modules/common/Appearance.qml"));
        assert_eq!(m.module_dir("qs.modules.common"), Some(root.join("modules/common")));
        assert_eq!(m.module_dir("qs"), Some(root.clone()));
        assert_eq!(m.module_dir("QtQuick"), None);
        assert_eq!(m.singletons_in(&root.join("modules/common/functions")), vec!["ColorUtils"]);
        let (ty, enums) = &m.enums[0];
        assert_eq!(ty, "MaterialShape");
        assert_eq!(enums[0].0, "Shape");
        assert_eq!(enums[0].1, vec![("Circle".into(), 0), ("Square".into(), 1), ("Gem".into(), 7), ("Pill".into(), 8)]);

        let doc = crate::qml::parse(r#"
            import QtQuick
            import qs.modules.common
            import qs.modules.common.functions as CF
            import "widgets"
            import "shapes/offset.js" as Offset
            Item {}
        "#).unwrap();
        let dir = root.join("modules/common");
        let i = imports(&doc, &dir, Some(&m));
        assert_eq!(i.dirs, vec![dir.clone(), root.join("modules/common"), dir.join("widgets")]
            .into_iter().fold(Vec::new(), |mut v, d| { if !v.contains(&d) { v.push(d) } v }));
        assert_eq!(i.spaces, vec![("CF".to_string(), root.join("modules/common/functions"))]);
        assert_eq!(i.js, vec![("Offset".to_string(), dir.join("shapes/offset.js"))]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
