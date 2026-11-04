// Reads KDE settings from `kdeglobals` to match the system theme (fonts, icons,
// and colors)
// when running under `QT_QPA_PLATFORMTHEME=kde`. Follows standard XDG
// precedence:
// user config overrides system-wide defaults. Locale-specific keys
// (`Key[lang]`) are ignored

use std::collections::HashMap;
use std::path::PathBuf;

use super::xdg::{kind, Env};

#[derive(Default, Debug)]
pub struct KdeGlobals {
    entries: HashMap<(String, String), String>,
}

impl KdeGlobals {
    /// Every `kdeglobals` the environment's configuration directories have
    pub fn read(env: &Env<'_>) -> KdeGlobals {
        let mut k = KdeGlobals::default();
        // System files first so user values override them
        for p in files(env).iter().rev() {
            if let Ok(text) = std::fs::read_to_string(p) {
                k.add(&text);
            }
        }
        k
    }

    /// One file's entries over what is there
    pub fn add(&mut self, text: &str) {
        let mut group = String::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') {
                group = line.to_string();
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let k = k.trim();
            if k.contains('[') {
                continue;
            }
            self.entries
                .insert((group.clone(), k.to_string()), v.trim().to_string());
        }
    }

    /// `get("General", "font")`
    pub fn get(&self, group: &str, key: &str) -> Option<&str> {
        self.entries
            .get(&(format!("[{group}]"), key.to_string()))
            .map(String::as_str)
    }
}

/// KConfig search path for `kdeglobals`, user first
fn files(env: &Env<'_>) -> Vec<PathBuf> {
    env.locations(kind::GENERIC_CONFIG)
        .into_iter()
        .map(|d| d.join("kdeglobals"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_users_entries_win_and_groups_are_whole_headers() {
        let mut k = KdeGlobals::default();
        k.add("[General]\nfont=System,10\nColorScheme=X\n[Icons]\nTheme=breeze\n");
        k.add("# the user's\n[General]\nfont=Google Sans Flex,11\n[Colors:Header][Inactive]\nForegroundNormal=#111\n[Colors:Header]\nForegroundNormal=#222\nName[de]=nicht\n");
        assert_eq!(k.get("General", "font"), Some("Google Sans Flex,11"));
        assert_eq!(k.get("General", "ColorScheme"), Some("X"));
        assert_eq!(k.get("Icons", "Theme"), Some("breeze"));
        assert_eq!(k.get("Colors:Header", "ForegroundNormal"), Some("#222"));
        assert_eq!(
            k.get("Colors:Header][Inactive", "ForegroundNormal"),
            Some("#111")
        );
        assert_eq!(k.get("Colors:Header", "Name"), None);
        assert_eq!(k.get("WM", "font"), None);
    }
}
