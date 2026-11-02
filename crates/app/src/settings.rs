//! Persistent user settings: theme, recent repositories and layout sizes.
//!
//! The file is JSON in the platform config directory. Loading is tolerant: a
//! bad field falls back to its default instead of losing the whole file.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The accent colours offered by Appearance → Accent colour.
pub const ACCENT_VALUES: [Accent; 4] =
    [Accent::Blue, Accent::Violet, Accent::Green, Accent::Orange];
/// The code font sizes offered by Appearance → Code font size, in px.
pub const MONO_SIZE_MIN: u8 = 9;
/// See [`MONO_SIZE_MIN`].
pub const MONO_SIZE_MAX: u8 = 20;
/// The diff context range offered by Diff & merge.
pub const DIFF_CONTEXT_MAX: u8 = 20;
/// The background-fetch intervals, in minutes.
pub const FETCH_INTERVALS: [u16; 4] = [1, 5, 15, 60];

/// The most recent repositories kept.
#[allow(dead_code)] // used by the welcome screen (stage 2.3+)
pub const RECENT_LIMIT: usize = 20;
/// The narrowest usable sidebar.
pub const SIDEBAR_WIDTH_MIN: f32 = 160.0;
/// The widest share of the window the sidebar may take. The other half stays
/// for the history and the details pane.
pub const SIDEBAR_MAX_SHARE: f32 = 0.5;
/// A sanity cap for the value persisted in the settings file. The rendered
/// width is additionally capped to [`sidebar_width_limit`].
pub const SIDEBAR_WIDTH_MAX: f32 = 10_000.0;
/// The sidebar width used when the file has no usable value.
pub const SIDEBAR_WIDTH_DEFAULT: f32 = 240.0;

/// The widest the sidebar may be at `window_width`: half the visible area,
/// but never below its own minimum on a very narrow window.
pub fn sidebar_width_limit(window_width: f32) -> f32 {
    (window_width * SIDEBAR_MAX_SHARE).max(SIDEBAR_WIDTH_MIN)
}

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
    /// Whether the History pane's Activity strip is collapsed to its header.
    pub activity_collapsed: bool,
    /// How the commit diff pane lays out a file.
    pub diff_layout: DiffLayout,
    /// Fetch the remotes in the background while a repository is open.
    pub background_fetch: bool,
    /// Selection, focus-ring and primary-button colour.
    pub accent: Accent,
    /// Height of list and tree rows.
    pub density: Density,
    /// Base interface font size, 12–14 px.
    pub ui_font_size: u8,
    /// Diff and commit-editor font size, 9–20 px.
    pub mono_font_size: u8,
    /// Draw each graph lane in its own colour.
    pub graph_colored: bool,
    /// Width of a graph edge, 1–3 px.
    pub graph_line_width: u8,
    /// Relative (`2 h ago`) or absolute (`2026-10-31 11:41`) history dates.
    pub date_format: DateFormat,
    /// Interface language; the next launch applies it.
    pub language: Language,
    /// What opens when the app starts.
    pub on_launch: OnLaunch,
    /// Download and offer updates in the background.
    pub check_updates: bool,
    /// Stable or beta update channel.
    pub update_channel: UpdateChannel,
    /// Minutes between background fetches.
    pub fetch_interval_minutes: u16,
    /// Ask before force-pushing.
    pub confirm_force_push: bool,
    /// Ask before discarding working-tree changes.
    pub confirm_discard: bool,
    /// Ask before deleting a branch.
    pub confirm_delete_branch: bool,
    /// Send anonymous usage statistics.
    pub telemetry: bool,
    /// Highlight the characters that differ inside a modified diff line.
    pub diff_word_highlight: bool,
    /// Wrap long diff lines instead of scrolling horizontally.
    pub diff_wrap: bool,
    /// Show the old/new line-number gutters.
    pub diff_line_numbers: bool,
    /// Which whitespace-only changes the diff ignores.
    pub diff_whitespace: WhitespaceMode,
    /// Unchanged lines shown around each hunk, 0–20.
    pub diff_context: u8,
    /// `builtin`, `vscode`, `vimdiff`, `meld` or `custom`.
    pub merge_tool: MergeTool,
    /// The command used by [`MergeTool::Custom`].
    pub merge_command: String,
    /// Whether the Identity page edits the global or the repository config.
    pub identity_scope: IdentityScope,
    /// Sign commits and tags with `signing_key`.
    pub sign_commits: bool,
    /// `user.signingkey`, empty when none is configured.
    pub signing_key: String,
    /// The connected GitHub account, empty when none.
    pub account_github: String,
    /// The connected GitLab account, empty when none.
    pub account_gitlab: String,
    /// Which git executable to run.
    pub git_mode: GitMode,
    /// The executable used by [`GitMode::Custom`].
    pub git_path: String,
    /// Per-action shortcut overrides; an empty value unbinds the action.
    pub keybindings: BTreeMap<String, String>,
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
            activity_collapsed: false,
            diff_layout: DiffLayout::default(),
            background_fetch: true,
            accent: Accent::default(),
            density: Density::default(),
            ui_font_size: 13,
            mono_font_size: 12,
            graph_colored: true,
            graph_line_width: 2,
            date_format: DateFormat::default(),
            language: Language::default(),
            on_launch: OnLaunch::default(),
            check_updates: true,
            update_channel: UpdateChannel::default(),
            fetch_interval_minutes: 5,
            confirm_force_push: true,
            confirm_discard: true,
            confirm_delete_branch: true,
            telemetry: false,
            diff_word_highlight: true,
            diff_wrap: false,
            diff_line_numbers: true,
            diff_whitespace: WhitespaceMode::default(),
            diff_context: 3,
            merge_tool: MergeTool::default(),
            merge_command: String::new(),
            identity_scope: IdentityScope::default(),
            sign_commits: false,
            signing_key: String::new(),
            account_github: String::new(),
            account_gitlab: String::new(),
            git_mode: GitMode::default(),
            git_path: String::new(),
            keybindings: BTreeMap::new(),
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

    /// The executable choice the Git page stored.
    pub fn git_choice(&self) -> git_core::GitChoice {
        match self.git_mode {
            GitMode::Auto => git_core::GitChoice::Auto,
            GitMode::Bundled => git_core::GitChoice::Bundled,
            GitMode::System => git_core::GitChoice::System,
            GitMode::Custom => git_core::GitChoice::Custom(PathBuf::from(self.git_path.clone())),
        }
    }

    /// The diff context and whitespace options for git and the preview.
    pub fn diff_options(&self) -> git_core::DiffOptions {
        git_core::DiffOptions {
            context: self.diff_context,
            whitespace: match self.diff_whitespace {
                WhitespaceMode::None => git_core::DiffWhitespace::None,
                WhitespaceMode::Eol => git_core::DiffWhitespace::Eol,
                WhitespaceMode::All => git_core::DiffWhitespace::All,
            },
        }
    }

    /// The command that resolves a conflict, empty for the built-in editor.
    pub fn merge_command(&self) -> &str {
        match self.merge_tool {
            MergeTool::Custom => &self.merge_command,
            tool => tool.preset(),
        }
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
        self.ui_font_size = self.ui_font_size.clamp(12, 14);
        self.mono_font_size = self.mono_font_size.clamp(MONO_SIZE_MIN, MONO_SIZE_MAX);
        self.graph_line_width = self.graph_line_width.clamp(1, 3);
        self.diff_context = self.diff_context.min(DIFF_CONTEXT_MAX);
        if !FETCH_INTERVALS.contains(&self.fetch_interval_minutes) {
            self.fetch_interval_minutes = 5;
        }
        self.git_path = self.git_path.trim().to_owned();
        self.merge_command = self.merge_command.trim().to_owned();
        self.signing_key = self.signing_key.trim().to_owned();
        self.account_github = self.account_github.trim().to_owned();
        self.account_gitlab = self.account_gitlab.trim().to_owned();
    }
}

/// Selection, focus-ring and primary-button colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Accent {
    #[default]
    Blue,
    Violet,
    Green,
    Orange,
}

impl From<Accent> for ui::theme::Accent {
    fn from(accent: Accent) -> Self {
        match accent {
            Accent::Blue => ui::theme::Accent::Blue,
            Accent::Violet => ui::theme::Accent::Violet,
            Accent::Green => ui::theme::Accent::Green,
            Accent::Orange => ui::theme::Accent::Orange,
        }
    }
}

/// Height of list and tree rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    Compact,
    #[default]
    Normal,
    Roomy,
}

impl Density {
    /// The row height in logical pixels.
    pub fn row_height(self) -> f32 {
        match self {
            Density::Compact => 22.,
            Density::Normal => 26.,
            Density::Roomy => 30.,
        }
    }
}

impl From<Density> for ui::theme::Density {
    fn from(density: Density) -> Self {
        match density {
            Density::Compact => ui::theme::Density::Compact,
            Density::Normal => ui::theme::Density::Normal,
            Density::Roomy => ui::theme::Density::Roomy,
        }
    }
}

/// Relative or absolute dates in history rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DateFormat {
    #[default]
    Relative,
    Absolute,
}

/// The interface language. Only English is translated so far; the choice is
/// stored and announced as a restart change, like the design does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    En,
    De,
    Fr,
    Es,
    Ru,
    Ja,
}

impl Language {
    pub fn label(self) -> &'static str {
        match self {
            Language::En => "English",
            Language::De => "Deutsch",
            Language::Fr => "Français",
            Language::Es => "Español",
            Language::Ru => "Русский",
            Language::Ja => "日本語",
        }
    }
}

/// What opens when the app starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnLaunch {
    /// The most recent repository, or the welcome screen when there is none.
    #[default]
    Last,
    /// Always the welcome screen.
    Welcome,
    /// The welcome screen without restoring the last repository.
    Nothing,
}

/// Stable or beta update channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    #[default]
    Stable,
    Beta,
}

/// Which whitespace-only changes the diff hides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WhitespaceMode {
    /// Show every change.
    #[default]
    None,
    /// Ignore whitespace at line end.
    Eol,
    /// Ignore all whitespace.
    All,
}

/// The tool that opens for conflicts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MergeTool {
    /// The three-way editor inside GitWarp.
    #[default]
    Builtin,
    VsCode,
    Vimdiff,
    Meld,
    /// A user command with `$LOCAL`, `$REMOTE`, `$BASE` and `$MERGED`.
    Custom,
}

impl MergeTool {
    /// The preset command, empty for the built-in editor.
    pub fn preset(self) -> &'static str {
        match self {
            MergeTool::Builtin => "",
            MergeTool::VsCode => "code --wait --merge $LOCAL $REMOTE $BASE $MERGED",
            MergeTool::Vimdiff => "vimdiff $LOCAL $BASE $REMOTE $MERGED",
            MergeTool::Meld => "meld $LOCAL $BASE $REMOTE --output $MERGED",
            MergeTool::Custom => "",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MergeTool::Builtin => "GitWarp (built-in)",
            MergeTool::VsCode => "VS Code",
            MergeTool::Vimdiff => "vimdiff",
            MergeTool::Meld => "Meld",
            MergeTool::Custom => "Custom command…",
        }
    }
}

/// Global or repository scope for the commit identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IdentityScope {
    /// `~/.gitconfig`.
    #[default]
    Global,
    /// `.git/config` of the open repository.
    Repository,
}

impl From<IdentityScope> for git_core::ConfigScope {
    fn from(scope: IdentityScope) -> Self {
        match scope {
            IdentityScope::Global => git_core::ConfigScope::Global,
            IdentityScope::Repository => git_core::ConfigScope::Local,
        }
    }
}

/// Which git executable to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GitMode {
    /// Bundled if working, otherwise the one from PATH.
    #[default]
    Auto,
    Bundled,
    System,
    Custom,
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

/// How the diff pane lays out a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffLayout {
    /// One column with `-`/`+` rows.
    #[default]
    Unified,
    /// Old and new side by side.
    Split,
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
        activity_collapsed: take(
            &mut object,
            "activity_collapsed",
            defaults.activity_collapsed,
        ),
        diff_layout: take(&mut object, "diff_layout", defaults.diff_layout),
        background_fetch: take(&mut object, "background_fetch", defaults.background_fetch),
        accent: take(&mut object, "accent", defaults.accent),
        density: take(&mut object, "density", defaults.density),
        ui_font_size: take(&mut object, "ui_font_size", defaults.ui_font_size),
        mono_font_size: take(&mut object, "mono_font_size", defaults.mono_font_size),
        graph_colored: take(&mut object, "graph_colored", defaults.graph_colored),
        graph_line_width: take(&mut object, "graph_line_width", defaults.graph_line_width),
        date_format: take(&mut object, "date_format", defaults.date_format),
        language: take(&mut object, "language", defaults.language),
        on_launch: take(&mut object, "on_launch", defaults.on_launch),
        check_updates: take(&mut object, "check_updates", defaults.check_updates),
        update_channel: take(&mut object, "update_channel", defaults.update_channel),
        fetch_interval_minutes: take(
            &mut object,
            "fetch_interval_minutes",
            defaults.fetch_interval_minutes,
        ),
        confirm_force_push: take(
            &mut object,
            "confirm_force_push",
            defaults.confirm_force_push,
        ),
        confirm_discard: take(&mut object, "confirm_discard", defaults.confirm_discard),
        confirm_delete_branch: take(
            &mut object,
            "confirm_delete_branch",
            defaults.confirm_delete_branch,
        ),
        telemetry: take(&mut object, "telemetry", defaults.telemetry),
        diff_word_highlight: take(
            &mut object,
            "diff_word_highlight",
            defaults.diff_word_highlight,
        ),
        diff_wrap: take(&mut object, "diff_wrap", defaults.diff_wrap),
        diff_line_numbers: take(&mut object, "diff_line_numbers", defaults.diff_line_numbers),
        diff_whitespace: take(&mut object, "diff_whitespace", defaults.diff_whitespace),
        diff_context: take(&mut object, "diff_context", defaults.diff_context),
        merge_tool: take(&mut object, "merge_tool", defaults.merge_tool),
        merge_command: take(&mut object, "merge_command", defaults.merge_command),
        identity_scope: take(&mut object, "identity_scope", defaults.identity_scope),
        sign_commits: take(&mut object, "sign_commits", defaults.sign_commits),
        signing_key: take(&mut object, "signing_key", defaults.signing_key),
        account_github: take(&mut object, "account_github", defaults.account_github),
        account_gitlab: take(&mut object, "account_gitlab", defaults.account_gitlab),
        git_mode: take(&mut object, "git_mode", defaults.git_mode),
        git_path: take(&mut object, "git_path", defaults.git_path),
        keybindings: take(&mut object, "keybindings", defaults.keybindings),
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
            activity_collapsed: true,
            diff_layout: DiffLayout::Split,
            background_fetch: false,
            ..Settings::default()
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
            load_json(dir.path(), r#"{"sidebar_width":99999}"#).sidebar_width,
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
    fn the_activity_strip_is_expanded_by_default_and_remembers_collapsing() {
        assert!(!Settings::default().activity_collapsed);
        let dir = TempDir::new("activity-collapsed");
        assert!(!load_json(dir.path(), r#"{"theme":"dark"}"#).activity_collapsed);
        assert!(load_json(dir.path(), r#"{"activity_collapsed":true}"#).activity_collapsed);
        // The round trip above (`missing_file_loads_defaults_and_round_trips`)
        // saves and reloads it with the other prefs.
    }

    #[test]
    fn diff_layout_defaults_to_unified_and_round_trips() {
        assert_eq!(Settings::default().diff_layout, DiffLayout::Unified);
        let dir = TempDir::new("diff-layout");
        let old = load_json(dir.path(), r#"{"theme":"dark"}"#);
        assert_eq!(old.diff_layout, DiffLayout::Unified);
        assert_eq!(
            load_json(dir.path(), r#"{"diff_layout":"split"}"#).diff_layout,
            DiffLayout::Split
        );
        assert_eq!(
            load_json(dir.path(), r#"{"diff_layout":"diagonal"}"#).diff_layout,
            DiffLayout::Unified
        );
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
    #[test]
    fn appearance_and_workflow_settings_round_trip() {
        let dir = TempDir::new("appearance-round-trip");
        let store = SettingsStore::new(settings_path(dir.path()));
        let mut settings = Settings {
            accent: Accent::Violet,
            density: Density::Compact,
            ui_font_size: 14,
            mono_font_size: 16,
            graph_colored: false,
            graph_line_width: 3,
            date_format: DateFormat::Absolute,
            language: Language::Ru,
            on_launch: OnLaunch::Welcome,
            check_updates: false,
            update_channel: UpdateChannel::Beta,
            fetch_interval_minutes: 15,
            confirm_force_push: false,
            confirm_discard: false,
            confirm_delete_branch: false,
            telemetry: true,
            diff_word_highlight: false,
            diff_wrap: true,
            diff_line_numbers: false,
            diff_whitespace: WhitespaceMode::All,
            diff_context: 8,
            merge_tool: MergeTool::Custom,
            merge_command: "mytool $LOCAL $BASE $REMOTE $MERGED".to_owned(),
            identity_scope: IdentityScope::Repository,
            sign_commits: true,
            signing_key: "SHA256:abc".to_owned(),
            account_github: "mira-k".to_owned(),
            account_gitlab: "mira.kovalenko".to_owned(),
            git_mode: GitMode::Custom,
            git_path: "/opt/git/bin/git".to_owned(),
            ..Settings::default()
        };
        settings
            .keybindings
            .insert("sidebar".to_owned(), "alt-b".to_owned());
        settings
            .keybindings
            .insert("commit".to_owned(), String::new());
        store.save(&settings).unwrap();
        assert_eq!(store.load(), settings);
        assert_eq!(
            store.load().keybindings.get("commit").map(String::as_str),
            Some(""),
            "an explicit unbind survives the round trip"
        );
    }

    #[test]
    fn clamps_the_new_numeric_settings() {
        let dir = TempDir::new("clamp-appearance");
        let loaded = load_json(
            dir.path(),
            r#"{"ui_font_size":99,"mono_font_size":1,"graph_line_width":9,
                "diff_context":99,"fetch_interval_minutes":7}"#,
        );
        assert_eq!(loaded.ui_font_size, 14);
        assert_eq!(loaded.mono_font_size, MONO_SIZE_MIN);
        assert_eq!(loaded.graph_line_width, 3);
        assert_eq!(loaded.diff_context, DIFF_CONTEXT_MAX);
        assert_eq!(loaded.fetch_interval_minutes, 5);
    }

    #[test]
    fn unknown_appearance_values_fall_back_to_defaults() {
        let dir = TempDir::new("unknown-appearance");
        let loaded = load_json(
            dir.path(),
            r#"{"accent":"pink","density":"huge","date_format":"stardate",
                "merge_tool":"emacs","git_mode":"flatpak","keybindings":"j"}"#,
        );
        let defaults = Settings::default();
        assert_eq!(loaded.accent, defaults.accent);
        assert_eq!(loaded.density, defaults.density);
        assert_eq!(loaded.date_format, defaults.date_format);
        assert_eq!(loaded.merge_tool, defaults.merge_tool);
        assert_eq!(loaded.git_mode, defaults.git_mode);
        assert!(loaded.keybindings.is_empty());
    }

    #[test]
    fn the_git_page_choice_becomes_a_git_choice() {
        let mut settings = Settings::default();
        assert_eq!(settings.git_choice(), git_core::GitChoice::Auto);
        settings.git_mode = GitMode::Bundled;
        assert_eq!(settings.git_choice(), git_core::GitChoice::Bundled);
        settings.git_mode = GitMode::System;
        assert_eq!(settings.git_choice(), git_core::GitChoice::System);
        settings.git_mode = GitMode::Custom;
        settings.git_path = "/opt/git/bin/git".to_owned();
        assert_eq!(
            settings.git_choice(),
            git_core::GitChoice::Custom(PathBuf::from("/opt/git/bin/git"))
        );
    }

    #[test]
    fn diff_options_map_to_git_core() {
        let mut settings = Settings::default();
        assert_eq!(settings.diff_options().context, 3);
        assert_eq!(
            settings.diff_options().whitespace,
            git_core::DiffWhitespace::None
        );
        settings.diff_context = 7;
        settings.diff_whitespace = WhitespaceMode::All;
        assert_eq!(settings.diff_options().context, 7);
        assert_eq!(
            settings.diff_options().whitespace,
            git_core::DiffWhitespace::All
        );
        settings.diff_whitespace = WhitespaceMode::Eol;
        assert_eq!(
            settings.diff_options().whitespace,
            git_core::DiffWhitespace::Eol
        );
    }

    #[test]
    fn the_merge_command_uses_the_preset_or_the_custom_text() {
        let mut settings = Settings::default();
        assert_eq!(settings.merge_tool, MergeTool::Builtin);
        assert_eq!(settings.merge_command(), "");
        settings.merge_tool = MergeTool::Meld;
        assert!(settings.merge_command().contains("meld"));
        settings.merge_tool = MergeTool::Custom;
        settings.merge_command = "mytool $LOCAL".to_owned();
        assert_eq!(settings.merge_command(), "mytool $LOCAL");
    }
}
