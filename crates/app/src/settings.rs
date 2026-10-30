//! Persistent user settings: theme, recent repositories and layout sizes.
//!
//! The file is JSON in the platform config directory. Loading is tolerant: a
//! bad field falls back to its default instead of losing the whole file.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The most recent repositories kept.
#[allow(dead_code)] // used by the welcome screen (stage 2.3+)
pub const RECENT_LIMIT: usize = 20;
/// The narrowest usable sidebar.
pub const SIDEBAR_WIDTH_MIN: f32 = 160.0;
/// The widest sidebar that still leaves room for the history.
pub const SIDEBAR_WIDTH_MAX: f32 = 420.0;
/// The sidebar width used when the file has no usable value.
pub const SIDEBAR_WIDTH_DEFAULT: f32 = 240.0;
/// The narrowest graph column.
pub const GRAPH_WIDTH_MIN: f32 = 40.0;
/// The widest graph column; the pane's own share can still narrow it.
pub const GRAPH_WIDTH_MAX: f32 = 480.0;
/// The graph column width used when the file has no usable value.
pub const GRAPH_WIDTH_DEFAULT: f32 = 112.0;

/// The user's settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Which theme to use.
    pub theme: ThemePref,
    /// Recent repository roots, most recent first.
    pub recent: Vec<RecentRepo>,
    /// `None` picks the dock placement from the window width.
    pub dock: Option<Dock>,
    /// Width of the sidebar in logical pixels.
    pub sidebar_width: f32,
    /// Height of the details pane; `None` uses the layout default.
    pub details_size: Option<f32>,
    /// Width of the graph column in the history table.
    pub graph_width: f32,
    /// Whether idle lanes fold into gap lanes.
    pub collapse_idle_lanes: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemePref::default(),
            recent: Vec::new(),
            dock: None,
            sidebar_width: SIDEBAR_WIDTH_DEFAULT,
            details_size: None,
            graph_width: GRAPH_WIDTH_DEFAULT,
            collapse_idle_lanes: false,
        }
    }
}

#[allow(dead_code)] // used by the welcome screen (stage 2.3+)
impl Settings {
    /// Moves `path` to the front of the recent list with `now` as its time.
    ///
    /// The path is stored as given (callers canonicalize once, off the UI
    /// thread, when they insert a repository root) and compared as stored:
    /// touching the list must not call into the file system.
    pub fn touch_recent(&mut self, path: PathBuf, now: u64) {
        self.recent.retain(|repo| repo.path != path);
        self.recent.insert(
            0,
            RecentRepo {
                path,
                last_opened: now,
            },
        );
        self.recent.truncate(RECENT_LIMIT);
    }

    /// Removes `path` from the recent list, if present.
    pub fn remove_recent(&mut self, path: &Path) {
        self.recent.retain(|repo| repo.path != path);
    }

    /// Clamps layout sizes and drops values that cannot be used.
    fn normalize(&mut self) {
        if self.sidebar_width.is_finite() {
            self.sidebar_width = self
                .sidebar_width
                .clamp(SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX);
        } else {
            self.sidebar_width = SIDEBAR_WIDTH_DEFAULT;
        }
        if let Some(size) = self.details_size {
            if !size.is_finite() || size <= 0.0 {
                self.details_size = None;
            }
        }
        if self.graph_width.is_finite() {
            self.graph_width = self.graph_width.clamp(GRAPH_WIDTH_MIN, GRAPH_WIDTH_MAX);
        } else {
            self.graph_width = GRAPH_WIDTH_DEFAULT;
        }
    }
}

/// The theme preference stored in the settings file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemePref {
    /// Follow the operating system.
    #[default]
    System,
    /// Always dark.
    Dark,
    /// Always light.
    Light,
}

impl From<ThemePref> for ui::ThemeMode {
    fn from(pref: ThemePref) -> Self {
        match pref {
            ThemePref::System => ui::ThemeMode::System,
            ThemePref::Dark => ui::ThemeMode::Dark,
            ThemePref::Light => ui::ThemeMode::Light,
        }
    }
}

/// Where the dock sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dock {
    /// Across the bottom.
    Bottom,
    /// Down the right side.
    Right,
}

/// A repository the user opened before.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecentRepo {
    /// Canonical repository root.
    pub path: PathBuf,
    /// When it was last opened, in unix seconds.
    pub last_opened: u64,
}

/// The platform config directory for GitWarp.
pub fn config_dir() -> Option<PathBuf> {
    config_dir_from(|name: &str| std::env::var_os(name))
}

/// [`config_dir`] with an injectable environment lookup, for tests.
pub fn config_dir_from(get: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        get("HOME")
            .map(|home| PathBuf::from(home).join("Library/Application Support/dev.v1rus.gitwarp"))
    }
    #[cfg(windows)]
    {
        get("APPDATA").map(|appdata| PathBuf::from(appdata).join("GitWarp"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(xdg) = get("XDG_CONFIG_HOME") {
            let xdg = PathBuf::from(xdg);
            if !xdg.as_os_str().is_empty() && xdg.is_absolute() {
                return Some(xdg.join("gitwarp"));
            }
        }
        get("HOME").map(|home| PathBuf::from(home).join(".config/gitwarp"))
    }
}

/// Reads and writes `settings.json` at an explicit path.
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    /// Creates a store for the settings file at `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Loads the settings, falling back to defaults for a missing or broken file.
    pub fn load(&self) -> Settings {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Settings::default(),
            Err(e) => {
                log::warn!("failed to read settings from {}: {e}", self.path.display());
                self.backup();
                return Settings::default();
            }
        };
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(value) => {
                let mut settings = settings_from_value(value);
                settings.normalize();
                settings
            }
            Err(e) => {
                log::warn!("invalid settings file {}: {e}", self.path.display());
                self.backup();
                Settings::default()
            }
        }
    }

    /// Writes the settings atomically, creating the parent directory.
    pub fn save(&self, settings: &Settings) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(settings).map_err(io::Error::other)?;
        // The quit flush can run while a background save is still writing:
        // saves in this process take turns, so the later snapshot is renamed
        // last and wins.
        static SAVING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _turn = SAVING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Unique per process, so two instances saving at once cannot rename
        // each other's half-written file over the settings.
        let tmp = self
            .path
            .with_file_name(format!("settings.json.{}.tmp", std::process::id()));
        let mut file = fs::File::create(&tmp)?;
        file.write_all(&json)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, &self.path)
    }

    /// Keeps a broken file around as `settings.json.bak`.
    fn backup(&self) {
        let backup = self.path.with_file_name("settings.json.bak");
        if let Err(e) = fs::rename(&self.path, &backup) {
            log::warn!("failed to back up settings to {}: {e}", backup.display());
        }
    }
}

/// Parses the top-level object one known key at a time.
fn settings_from_value(value: Value) -> Settings {
    let Value::Object(mut object) = value else {
        log::warn!("invalid settings file: expected a JSON object");
        return Settings::default();
    };
    let defaults = Settings::default();
    Settings {
        theme: take(&mut object, "theme", defaults.theme),
        recent: take_recent(&mut object),
        dock: take(&mut object, "dock", defaults.dock),
        sidebar_width: take(&mut object, "sidebar_width", defaults.sidebar_width),
        details_size: take(&mut object, "details_size", defaults.details_size),
        graph_width: take(&mut object, "graph_width", defaults.graph_width),
        collapse_idle_lanes: take(
            &mut object,
            "collapse_idle_lanes",
            defaults.collapse_idle_lanes,
        ),
    }
}

/// Takes `key` and deserializes it, or returns `fallback` on absence or error.
fn take<T: DeserializeOwned>(object: &mut Map<String, Value>, key: &str, fallback: T) -> T {
    let Some(value) = object.remove(key) else {
        return fallback;
    };
    match serde_json::from_value(value) {
        Ok(parsed) => parsed,
        Err(e) => {
            log::warn!("ignoring invalid setting `{key}`: {e}");
            fallback
        }
    }
}

/// Parses the recent list, dropping entries that fail on their own.
fn take_recent(object: &mut Map<String, Value>) -> Vec<RecentRepo> {
    let Some(value) = object.remove("recent") else {
        return Vec::new();
    };
    let Value::Array(items) = value else {
        log::warn!("ignoring invalid setting `recent`: expected a list");
        return Vec::new();
    };
    items
        .into_iter()
        .filter_map(|item| match serde_json::from_value::<RecentRepo>(item) {
            Ok(repo) => Some(repo),
            Err(e) => {
                log::warn!("dropping invalid recent repository: {e}");
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A unique temp directory removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "gitwarp-settings-{}-{name}-{n}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn settings_path(dir: &Path) -> PathBuf {
        dir.join("settings.json")
    }

    fn load_json(dir: &Path, json: &str) -> Settings {
        let path = settings_path(dir);
        fs::write(&path, json).unwrap();
        SettingsStore::new(path).load()
    }

    fn paths(settings: &Settings) -> Vec<&Path> {
        settings.recent.iter().map(|r| r.path.as_path()).collect()
    }

    #[test]
    fn missing_file_loads_defaults_and_round_trips() {
        let dir = TempDir::new("round-trip");
        let store = SettingsStore::new(settings_path(dir.path()));
        assert_eq!(store.load(), Settings::default());

        let settings = Settings {
            theme: ThemePref::Dark,
            recent: vec![RecentRepo {
                path: PathBuf::from("/repo"),
                last_opened: 7,
            }],
            dock: Some(Dock::Right),
            sidebar_width: 320.5,
            details_size: Some(180.0),
            graph_width: 176.0,
            collapse_idle_lanes: true,
        };
        store.save(&settings).unwrap();
        assert_eq!(store.load(), settings);
    }

    #[test]
    fn invalid_json_is_backed_up() {
        let dir = TempDir::new("invalid-json");
        let path = settings_path(dir.path());
        let original = b"{oops";
        fs::write(&path, original).unwrap();
        fs::write(dir.path().join("settings.json.bak"), b"older").unwrap();

        assert_eq!(SettingsStore::new(&path).load(), Settings::default());
        assert_eq!(
            fs::read(dir.path().join("settings.json.bak")).unwrap(),
            original
        );
    }

    #[test]
    fn partial_file_keeps_valid_fields() {
        let dir = TempDir::new("partial");
        let loaded = load_json(
            dir.path(),
            r#"{"theme":"dark","sidebar_width":"wide","future":true,
                "recent":[{"path":"/a","last_opened":1},{"oops":1}]}"#,
        );
        assert_eq!(loaded.theme, ThemePref::Dark);
        assert_eq!(loaded.sidebar_width, SIDEBAR_WIDTH_DEFAULT);
        assert_eq!(
            loaded.recent,
            vec![RecentRepo {
                path: PathBuf::from("/a"),
                last_opened: 1,
            }]
        );
    }

    #[test]
    fn unknown_theme_falls_back_to_system() {
        let dir = TempDir::new("unknown-theme");
        let loaded = load_json(dir.path(), r#"{"theme":"purple"}"#);
        assert_eq!(loaded.theme, ThemePref::System);
        assert_eq!(loaded.sidebar_width, SIDEBAR_WIDTH_DEFAULT);
        assert!(loaded.recent.is_empty());
        assert_eq!(loaded.dock, None);
        assert_eq!(loaded.details_size, None);
    }

    #[test]
    fn clamps_sizes_on_load() {
        let dir = TempDir::new("clamp");
        assert_eq!(
            load_json(dir.path(), r#"{"sidebar_width":9999}"#).sidebar_width,
            SIDEBAR_WIDTH_MAX
        );
        assert_eq!(
            load_json(dir.path(), r#"{"sidebar_width":-5}"#).sidebar_width,
            SIDEBAR_WIDTH_MIN
        );
        assert_eq!(
            load_json(dir.path(), r#"{"details_size":-1}"#).details_size,
            None
        );
    }

    #[test]
    fn graph_settings_default_and_are_clamped_on_load() {
        let defaults = Settings::default();
        assert_eq!(defaults.graph_width, GRAPH_WIDTH_DEFAULT);
        assert!(!defaults.collapse_idle_lanes);

        let dir = TempDir::new("graph-settings");
        // A file written before the graph settings existed loads the defaults.
        let old = load_json(dir.path(), r#"{"theme":"dark"}"#);
        assert_eq!(old.graph_width, GRAPH_WIDTH_DEFAULT);
        assert!(!old.collapse_idle_lanes);

        assert_eq!(
            load_json(dir.path(), r#"{"graph_width":9999}"#).graph_width,
            GRAPH_WIDTH_MAX
        );
        assert_eq!(
            load_json(dir.path(), r#"{"graph_width":1}"#).graph_width,
            GRAPH_WIDTH_MIN
        );
        assert_eq!(
            load_json(dir.path(), r#"{"graph_width":"wide"}"#).graph_width,
            GRAPH_WIDTH_DEFAULT
        );
        assert!(load_json(dir.path(), r#"{"collapse_idle_lanes":true}"#).collapse_idle_lanes);
    }

    #[test]
    fn recent_repositories_move_cap_and_remove() {
        let mut settings = Settings::default();
        settings.touch_recent(PathBuf::from("/a"), 1);
        settings.touch_recent(PathBuf::from("/b"), 2);
        assert_eq!(paths(&settings), vec![Path::new("/b"), Path::new("/a")]);

        settings.touch_recent(PathBuf::from("/a"), 3);
        assert_eq!(paths(&settings), vec![Path::new("/a"), Path::new("/b")]);
        assert_eq!(settings.recent[0].last_opened, 3);
        assert_eq!(settings.recent.len(), 2);

        let mut many = Settings::default();
        for i in 0..21 {
            many.touch_recent(PathBuf::from(format!("/repo{i}")), i);
        }
        assert_eq!(many.recent.len(), RECENT_LIMIT);
        assert_eq!(many.recent[0].path, PathBuf::from("/repo20"));
        assert_eq!(many.recent[19].path, PathBuf::from("/repo1"));

        many.remove_recent(Path::new("/repo10"));
        assert_eq!(many.recent.len(), RECENT_LIMIT - 1);
        assert!(!many.recent.iter().any(|r| r.path == Path::new("/repo10")));
    }

    #[test]
    fn save_leaves_no_temp_file() {
        let dir = TempDir::new("no-temp");
        let store = SettingsStore::new(settings_path(dir.path()));
        store.save(&Settings::default()).unwrap();
        assert!(settings_path(dir.path()).exists());
        let temp = dir
            .path()
            .join(format!("settings.json.{}.tmp", std::process::id()));
        assert!(!temp.exists());
    }

    #[test]
    fn touching_dedupes_as_stored_without_canonicalizing() {
        // Two spellings of the same directory stay two entries: the list is
        // compared as stored, and only an identical stored path is replaced.
        let dir = TempDir::new("dedupe");
        let repo = dir.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let alias = repo.join("..").join("repo");
        assert_ne!(repo, alias);

        let mut settings = Settings::default();
        settings.touch_recent(repo.clone(), 1);
        settings.touch_recent(alias.clone(), 2);
        assert_eq!(paths(&settings), vec![alias.as_path(), repo.as_path()]);
        assert_eq!(
            settings.recent[0].path, alias,
            "the path is stored as given"
        );

        settings.touch_recent(repo.clone(), 3);
        assert_eq!(paths(&settings), vec![repo.as_path(), alias.as_path()]);
        assert_eq!(settings.recent[0].last_opened, 3);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn config_dir_honours_xdg_config_home() {
        let with_xdg = |key: &str| match key {
            "XDG_CONFIG_HOME" => Some(std::ffi::OsString::from("/tmp/xdg")),
            "HOME" => Some(std::ffi::OsString::from("/home/user")),
            _ => None,
        };
        assert_eq!(
            config_dir_from(with_xdg),
            Some(PathBuf::from("/tmp/xdg/gitwarp"))
        );

        let relative = |key: &str| match key {
            "XDG_CONFIG_HOME" => Some(std::ffi::OsString::from("relative/config")),
            "HOME" => Some(std::ffi::OsString::from("/home/user")),
            _ => None,
        };
        assert_eq!(
            config_dir_from(relative),
            Some(PathBuf::from("/home/user/.config/gitwarp"))
        );

        let empty = |key: &str| match key {
            "XDG_CONFIG_HOME" => Some(std::ffi::OsString::new()),
            "HOME" => Some(std::ffi::OsString::from("/home/user")),
            _ => None,
        };
        assert_eq!(
            config_dir_from(empty),
            Some(PathBuf::from("/home/user/.config/gitwarp"))
        );

        assert_eq!(config_dir_from(|_| None), None);
    }
}
