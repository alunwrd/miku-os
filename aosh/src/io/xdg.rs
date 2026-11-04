// User file locations, as Qt's `QStandardPaths` reports them on Unix
// (`qstandardpaths_unix.cpp`)
//
// Uses the XDG variable if absolute, else the XDG default under `$HOME`.
// `App*`, `Cache`, and `State` kinds append the application directory.
//
// The application name is `quickshell`, so shells find their state at the same
// paths as under quickshell (`~/.local/state/quickshell/user/todo.json`).
//
// User directories come from `~/.config/user-dirs.dirs`, with English defaults.

use std::path::{Path, PathBuf};

/// Qt's application name, which the per-application kinds end in
const APP: &str = "quickshell";

/// `QStandardPaths::StandardLocation`, in Qt's order
pub mod kind {
    pub const DESKTOP: u32 = 0;
    pub const DOCUMENTS: u32 = 1;
    pub const FONTS: u32 = 2;
    pub const APPLICATIONS: u32 = 3;
    pub const MUSIC: u32 = 4;
    pub const MOVIES: u32 = 5;
    pub const PICTURES: u32 = 6;
    pub const TEMP: u32 = 7;
    pub const HOME: u32 = 8;
    pub const APP_LOCAL_DATA: u32 = 9;
    pub const CACHE: u32 = 10;
    pub const GENERIC_DATA: u32 = 11;
    pub const RUNTIME: u32 = 12;
    pub const CONFIG: u32 = 13;
    pub const DOWNLOAD: u32 = 14;
    pub const GENERIC_CACHE: u32 = 15;
    pub const GENERIC_CONFIG: u32 = 16;
    pub const APP_DATA: u32 = 17;
    pub const APP_CONFIG: u32 = 18;
    pub const PUBLIC_SHARE: u32 = 19;
    pub const TEMPLATES: u32 = 20;
    pub const STATE: u32 = 21;
    pub const GENERIC_STATE: u32 = 22;
}

/// `$HOME` and XDG variables, separate from `std::env` for tests
pub struct Env<'a> {
    pub var: &'a dyn Fn(&str) -> Option<String>,
}

impl Env<'_> {
    pub fn process() -> Env<'static> {
        fn var(k: &str) -> Option<String> {
            std::env::var(k).ok()
        }
        Env { var: &var }
    }

    fn home(&self) -> PathBuf {
        PathBuf::from((self.var)("HOME").filter(|h| !h.is_empty()).unwrap_or_else(|| "/".into()))
    }

    /// XDG variable if absolute; relative values are ignored, per the spec
    fn xdg(&self, name: &str, default: &str) -> PathBuf {
        match (self.var)(name).filter(|v| v.starts_with('/')) {
            Some(v) => PathBuf::from(v),
            None => self.home().join(default),
        }
    }

    /// `XDG_DATA_DIRS` or `XDG_CONFIG_DIRS`: system directories after the
    /// user's
    fn system(&self, name: &str, default: &str) -> Vec<PathBuf> {
        let v = (self.var)(name).filter(|v| !v.is_empty()).unwrap_or_else(|| default.into());
        v.split(':').filter(|d| d.starts_with('/')).map(PathBuf::from).collect()
    }

    /// A user directory from `user-dirs.dirs`:
    /// `XDG_PICTURES_DIR="$HOME/Pictures"`
    fn user_dir(&self, key: &str, default: &str) -> PathBuf {
        let file = self.xdg("XDG_CONFIG_HOME", ".config").join("user-dirs.dirs");
        let home = self.home();
        if let Ok(text) = std::fs::read_to_string(&file) {
            for line in text.lines() {
                let Some(v) = line.trim().strip_prefix(key).and_then(|r| r.strip_prefix('=')) else { continue };
                let v = v.trim().trim_matches('"');
                let p = match v.strip_prefix("$HOME") {
                    Some(rest) => home.join(rest.trim_start_matches('/')),
                    None if v.starts_with('/') => PathBuf::from(v),
                    None => continue,
                };
                return p;
            }
        }
        home.join(default)
    }

    /// `QStandardPaths::writableLocation`, or `None` if the kind has no
    /// writable location
    pub fn writable(&self, k: u32) -> Option<PathBuf> {
        let app = |p: PathBuf| p.join(APP);
        Some(match k {
            kind::HOME => self.home(),
            kind::TEMP => PathBuf::from((self.var)("TMPDIR").filter(|t| t.starts_with('/')).unwrap_or_else(|| "/tmp".into())),
            kind::CACHE => app(self.xdg("XDG_CACHE_HOME", ".cache")),
            kind::GENERIC_CACHE => self.xdg("XDG_CACHE_HOME", ".cache"),
            kind::STATE => app(self.xdg("XDG_STATE_HOME", ".local/state")),
            kind::GENERIC_STATE => self.xdg("XDG_STATE_HOME", ".local/state"),
            kind::APP_DATA | kind::APP_LOCAL_DATA => app(self.xdg("XDG_DATA_HOME", ".local/share")),
            kind::GENERIC_DATA => self.xdg("XDG_DATA_HOME", ".local/share"),
            kind::FONTS => self.xdg("XDG_DATA_HOME", ".local/share").join("fonts"),
            kind::APPLICATIONS => self.xdg("XDG_DATA_HOME", ".local/share").join("applications"),
            // Qt's ConfigLocation is the generic `~/.config` on Unix, not the
            // application's
            kind::CONFIG | kind::GENERIC_CONFIG => self.xdg("XDG_CONFIG_HOME", ".config"),
            kind::APP_CONFIG => app(self.xdg("XDG_CONFIG_HOME", ".config")),
            kind::RUNTIME => PathBuf::from((self.var)("XDG_RUNTIME_DIR").filter(|r| r.starts_with('/'))?),
            kind::DESKTOP => self.user_dir("XDG_DESKTOP_DIR", "Desktop"),
            kind::DOCUMENTS => self.user_dir("XDG_DOCUMENTS_DIR", "Documents"),
            kind::DOWNLOAD => self.user_dir("XDG_DOWNLOAD_DIR", "Downloads"),
            kind::MUSIC => self.user_dir("XDG_MUSIC_DIR", "Music"),
            kind::PICTURES => self.user_dir("XDG_PICTURES_DIR", "Pictures"),
            kind::MOVIES => self.user_dir("XDG_VIDEOS_DIR", "Videos"),
            kind::PUBLIC_SHARE => self.user_dir("XDG_PUBLICSHARE_DIR", "Public"),
            kind::TEMPLATES => self.user_dir("XDG_TEMPLATES_DIR", "Templates"),
            _ => return None,
        })
    }

    /// `QStandardPaths::standardLocations`: writable location first, then
    /// system directories
    pub fn locations(&self, k: u32) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self.writable(k).into_iter().collect();
        let data = || self.system("XDG_DATA_DIRS", "/usr/local/share:/usr/share");
        let more: Vec<PathBuf> = match k {
            kind::GENERIC_DATA => data(),
            kind::APP_DATA | kind::APP_LOCAL_DATA => data().into_iter().map(|d| d.join(APP)).collect(),
            kind::FONTS => data().into_iter().map(|d| d.join("fonts")).collect(),
            kind::APPLICATIONS => data().into_iter().map(|d| d.join("applications")).collect(),
            kind::CONFIG | kind::GENERIC_CONFIG => self.system("XDG_CONFIG_DIRS", "/etc/xdg"),
            kind::APP_CONFIG => self.system("XDG_CONFIG_DIRS", "/etc/xdg").into_iter().map(|d| d.join(APP)).collect(),
            _ => Vec::new(),
        };
        for d in more {
            if !out.contains(&d) {
                out.push(d);
            }
        }
        out
    }
}

/// A path as the `file://` URL QML's `url` values are
pub fn url(p: &Path) -> String {
    format!("file://{}", p.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qts_answers_under_a_home() {
        let dir = std::env::temp_dir().join(format!("aosh-xdg-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".config")).unwrap();
        std::fs::write(dir.join(".config/user-dirs.dirs"),
            "# written by xdg-user-dirs-update\nXDG_PICTURES_DIR=\"$HOME/Картинки\"\nXDG_MUSIC_DIR=\"/srv/music\"\n").unwrap();
        let home = dir.display().to_string();
        let var = move |k: &str| match k {
            "HOME" => Some(home.clone()),
            "XDG_CACHE_HOME" => Some("relative/is/ignored".into()),
            "XDG_STATE_HOME" => Some("/state".into()),
            _ => None,
        };
        let env = Env { var: &var };
        assert_eq!(env.writable(kind::HOME), Some(dir.clone()));
        assert_eq!(env.writable(kind::CONFIG), Some(dir.join(".config")));
        assert_eq!(env.writable(kind::STATE), Some(PathBuf::from("/state/quickshell")));
        assert_eq!(env.writable(kind::CACHE), Some(dir.join(".cache/quickshell")));
        assert_eq!(env.writable(kind::GENERIC_CACHE), Some(dir.join(".cache")));
        assert_eq!(env.writable(kind::PICTURES), Some(dir.join("Картинки")));
        assert_eq!(env.writable(kind::MUSIC), Some(PathBuf::from("/srv/music")));
        assert_eq!(env.writable(kind::MOVIES), Some(dir.join("Videos")));
        assert_eq!(env.writable(kind::RUNTIME), None);
        assert_eq!(env.locations(kind::CONFIG), vec![dir.join(".config"), PathBuf::from("/etc/xdg")]);
        assert_eq!(url(&dir.join("x")), format!("file://{}/x", dir.display()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
