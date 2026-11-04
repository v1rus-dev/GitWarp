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

/// The most recent worktree folders kept for ordering by recency.
pub const WORKTREE_RECENCY_LIMIT: usize = 200;

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

/// A persisted column width, clamped to its minimum and the file sanity cap;
/// a broken value falls back to the design default.
fn clamp_width(width: f32, min: f32, default: f32) -> f32 {
    if width.is_finite() {
        width.clamp(min, COLUMN_WIDTH_MAX)
    } else {
        default
    }
}

/// The narrowest graph column.
pub const GRAPH_WIDTH_MIN: f32 = 40.0;
/// A sanity cap for a persisted column width. Dragging has no fixed maximum:
/// the pane's own room is the only bound.
pub const COLUMN_WIDTH_MAX: f32 = 10_000.0;
/// The widest graph column a settings file may carry.
pub const GRAPH_WIDTH_MAX: f32 = COLUMN_WIDTH_MAX;
/// The graph column width used when the file has no usable value.
pub const GRAPH_WIDTH_DEFAULT: f32 = 112.0;
/// The narrowest author column.
pub const AUTHOR_WIDTH_MIN: f32 = 48.0;
/// The narrowest date column.
pub const DATE_WIDTH_MIN: f32 = 48.0;
/// The narrowest hash column.
pub const HASH_WIDTH_MIN: f32 = 40.0;
/// The author column width used when the file has no usable value.
pub const AUTHOR_WIDTH_DEFAULT: f32 = 130.0;
/// The date column width used when the file has no usable value.
pub const DATE_WIDTH_DEFAULT: f32 = 104.0;
/// The hash column width used when the file has no usable value.
pub const HASH_WIDTH_DEFAULT: f32 = 64.0;

/// Prefix of the ids given to accounts migrated from the old
/// `account_github` / `account_gitlab` settings. The id is fixed per service
/// (not random) so that migrating the same file twice yields the same record.
pub const LEGACY_ACCOUNT_ID_PREFIX: &str = "legacy-";

/// How an account signs in. Persisted as `"token"` / `"browser"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountMethod {
    #[default]
    Token,
    Browser,
}

/// One hosting-service account. The secret (token) is never stored here: it
/// lives in the system keychain under [`AccountRecord::id`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountRecord {
    /// Stable, unique id; also the keychain entry name.
    pub id: String,
    /// `forge::ServiceKind::id()`.
    pub kind: String,
    /// Normalized instance base URL (`https://github.com`).
    pub base_url: String,
    pub login: String,
    /// Shown in menus; defaults to the login.
    pub label: String,
    pub method: AccountMethod,
    /// Host used for git over SSH when it differs from the web host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_host: Option<String>,
    /// The command-line tool this account was imported from (`"gh"` or
    /// `"glab"`); `None` for accounts added in GitWarp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_from: Option<String>,
}

impl AccountRecord {
    /// The label, or the login when the label is empty.
    pub fn display_label(&self) -> &str {
        if self.label.trim().is_empty() {
            &self.login
        } else {
            &self.label
        }
    }
}

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
    /// Width of the author column in the history table.
    pub author_width: f32,
    /// Width of the date column in the history table.
    pub date_width: f32,
    /// Width of the hash column in the history table.
    pub hash_width: f32,
    /// Whether idle lanes fold into gap lanes.
    pub collapse_idle_lanes: bool,
    /// Whether the History pane's Activity strip is collapsed to its header.
    pub activity_collapsed: bool,
    /// How the commit diff pane lays out a file.
    pub diff_layout: DiffLayout,
    /// Fetch the remotes in the background while a repository is open.
    pub background_fetch: bool,
    /// Show the commit activity heatmap on the Welcome screen and above History.
    pub show_activity: bool,
    /// Show CI status in history rows and the details pane, and ask the host
    /// for it. Off hides every badge and makes no checks requests.
    pub show_ci_status: bool,
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
    /// Check GitHub for new releases and show a toast when one is out.
    pub check_updates: bool,
    /// Which releases the update check reports: stable only, or pre-releases too.
    pub update_channel: UpdateChannel,
    /// The last version an update toast was shown for; a version is announced
    /// only if it is newer than this.
    pub update_notified_version: Option<String>,
    /// Minutes between background fetches.
    pub fetch_interval_minutes: u16,
    /// Ask before force-pushing.
    pub confirm_force_push: bool,
    /// Ask before discarding working-tree changes.
    pub confirm_discard: bool,
    /// Ask before deleting a branch.
    pub confirm_delete_branch: bool,
    /// Send crash reports (Sentry). On by default.
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
    /// Hosting-service accounts (Settings → Accounts); secrets are in the keychain.
    pub accounts: Vec<AccountRecord>,
    /// `gh` / `glab` logins the user dismissed on Settings → Accounts, as
    /// `tool|host|login` keys (see `cli_import::dismiss_key`).
    pub dismissed_imports: Vec<String>,
    /// The account chosen for a remote when several match its host: repository
    /// (canonical common git dir, shared by all its worktrees) → remote name →
    /// account id. Kept here, not in the repository's `.git/config`, so GitWarp
    /// never writes app data into a user's repository. Ids of removed accounts
    /// are ignored when a remote is resolved and dropped by [`Settings::forget_account`].
    pub remote_accounts: BTreeMap<String, BTreeMap<String, String>>,
    /// Which git executable to run.
    pub git_mode: GitMode,
    /// The executable used by [`GitMode::Custom`].
    pub git_path: String,
    /// Per-action shortcut overrides; an empty value unbinds the action.
    pub keybindings: BTreeMap<String, String>,
    /// Where new worktrees go and what happens around creating and removing them.
    pub worktrees: WorktreeSettings,
    /// When GitWarp last opened or switched to a worktree, by folder, in unix
    /// seconds. Orders the worktree lists by recency.
    pub worktree_recency: BTreeMap<String, u64>,
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
            author_width: AUTHOR_WIDTH_DEFAULT,
            date_width: DATE_WIDTH_DEFAULT,
            hash_width: HASH_WIDTH_DEFAULT,
            collapse_idle_lanes: false,
            activity_collapsed: false,
            diff_layout: DiffLayout::default(),
            background_fetch: true,
            show_activity: true,
            show_ci_status: true,
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
            update_notified_version: None,
            fetch_interval_minutes: 5,
            confirm_force_push: true,
            confirm_discard: true,
            confirm_delete_branch: true,
            telemetry: true,
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
            accounts: Vec::new(),
            dismissed_imports: Vec::new(),
            remote_accounts: BTreeMap::new(),
            git_mode: GitMode::default(),
            git_path: String::new(),
            keybindings: BTreeMap::new(),
            worktrees: WorktreeSettings::default(),
            worktree_recency: BTreeMap::new(),
        }
    }
}

/// The base folders offered by Settings → Worktrees, `~` stored literally.
pub const WORKTREE_BASES: [&str; 3] = ["~/develop", "~/worktrees", "~/develop/.worktrees"];

/// The folder-name templates offered by Settings → Worktrees.
pub const WORKTREE_TEMPLATES: [&str; 2] = ["{repo}-{branch}", "{branch}"];

/// Worktree creation and removal preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorktreeSettings {
    /// Folder new worktrees are created in; `~` is expanded when used.
    pub base: String,
    /// Folder name: `{repo}` and `{branch}` are substituted.
    pub template: String,
    /// Whether the app switches to a freshly created worktree.
    pub after_create: AfterCreate,
    /// Copy untracked `.env` files into the new worktree.
    pub copy_env: bool,
    /// Ask before removing a clean worktree.
    pub confirm_remove_clean: bool,
    /// Ask before deleting the branch together with its worktree.
    pub confirm_delete_branch: bool,
}

impl Default for WorktreeSettings {
    fn default() -> Self {
        Self {
            base: WORKTREE_BASES[0].to_owned(),
            template: WORKTREE_TEMPLATES[0].to_owned(),
            after_create: AfterCreate::default(),
            copy_env: true,
            confirm_remove_clean: true,
            confirm_delete_branch: true,
        }
    }
}

/// What happens when a worktree has been created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AfterCreate {
    /// Open the new worktree.
    #[default]
    Switch,
    /// Stay in the current worktree.
    Stay,
}

#[allow(dead_code)] // used by the welcome screen (stage 2.3+)
impl Settings {
    /// Remembers `account_id` as the account `remote` uses in the repository
    /// `repo_key` (see [`Settings::remote_accounts`]).
    pub fn set_remote_account(&mut self, repo_key: &str, remote: &str, account_id: &str) {
        self.remote_accounts
            .entry(repo_key.to_owned())
            .or_default()
            .insert(remote.to_owned(), account_id.to_owned());
    }

    /// Drops the remembered account of `remote` in `repo_key`.
    pub fn forget_remote(&mut self, repo_key: &str, remote: &str) {
        self.reconcile_remote_accounts(repo_key, &[remote.to_owned()], &[]);
    }

    /// Follows the remotes of `repo_key` across a reload of its remote list:
    /// one remote gone and one new is a rename, so its choice moves to the new
    /// name; any other vanished remote (removed) loses its choice. Remotes that
    /// are in both lists are untouched.
    pub fn reconcile_remote_accounts(
        &mut self,
        repo_key: &str,
        before: &[String],
        after: &[String],
    ) {
        let Some(remotes) = self.remote_accounts.get_mut(repo_key) else {
            return;
        };
        let gone: Vec<&String> = before.iter().filter(|n| !after.contains(n)).collect();
        let added: Vec<&String> = after.iter().filter(|n| !before.contains(n)).collect();
        match (gone.as_slice(), added.as_slice()) {
            ([old], [new]) => {
                if let Some(id) = remotes.remove(*old) {
                    remotes.insert((*new).clone(), id);
                }
            }
            (gone, _) => {
                for name in gone {
                    remotes.remove(*name);
                }
            }
        }
        if remotes.is_empty() {
            self.remote_accounts.remove(repo_key);
        }
    }

    /// Drops every remembered choice of the removed account `id`, and the
    /// repositories left without any choice.
    pub fn forget_account(&mut self, id: &str) {
        for remotes in self.remote_accounts.values_mut() {
            remotes.retain(|_, account| account != id);
        }
        self.remote_accounts
            .retain(|_, remotes| !remotes.is_empty());
    }

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

    /// Records that the worktree at `path` was opened at `now`. Only the most
    /// recent [`WORKTREE_RECENCY_LIMIT`] folders are kept.
    pub fn touch_worktree(&mut self, path: &Path, now: u64) {
        self.worktree_recency
            .insert(path.to_string_lossy().into_owned(), now);
        if self.worktree_recency.len() > WORKTREE_RECENCY_LIMIT {
            let mut times: Vec<u64> = self.worktree_recency.values().copied().collect();
            times.sort_unstable_by(|a, b| b.cmp(a));
            let cutoff = times[WORKTREE_RECENCY_LIMIT - 1];
            self.worktree_recency.retain(|_, time| *time >= cutoff);
        }
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
        self.author_width = clamp_width(self.author_width, AUTHOR_WIDTH_MIN, AUTHOR_WIDTH_DEFAULT);
        self.date_width = clamp_width(self.date_width, DATE_WIDTH_MIN, DATE_WIDTH_DEFAULT);
        self.hash_width = clamp_width(self.hash_width, HASH_WIDTH_MIN, HASH_WIDTH_DEFAULT);
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
        normalize_accounts(&mut self.accounts);
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

/// The interface language. The saved choice is handed to `ui::i18n::set_language`
/// once at startup (see `main.rs`), so a change takes effect after a restart, like
/// the design announces. Every language has a full catalog; a key missing from one
/// would still fall back to the English text.
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
    /// The catalog code understood by `ui::i18n::set_language`.
    pub fn code(self) -> &'static str {
        match self {
            Language::En => "en",
            Language::De => "de",
            Language::Fr => "fr",
            Language::Es => "es",
            Language::Ru => "ru",
            Language::Ja => "ja",
        }
    }

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

    pub fn label(self) -> gpui::SharedString {
        match self {
            MergeTool::Builtin => ui::t!("settings.merge_tool.builtin"),
            MergeTool::VsCode => "VS Code".into(),
            MergeTool::Vimdiff => "vimdiff".into(),
            MergeTool::Meld => "Meld".into(),
            MergeTool::Custom => ui::t!("settings.merge_tool.custom"),
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
        author_width: take(&mut object, "author_width", defaults.author_width),
        date_width: take(&mut object, "date_width", defaults.date_width),
        hash_width: take(&mut object, "hash_width", defaults.hash_width),
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
        show_activity: take(&mut object, "show_activity", defaults.show_activity),
        show_ci_status: take(&mut object, "show_ci_status", defaults.show_ci_status),
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
        update_notified_version: take(
            &mut object,
            "update_notified_version",
            defaults.update_notified_version,
        ),
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
        accounts: take_accounts(&mut object),
        dismissed_imports: take(&mut object, "dismissed_imports", defaults.dismissed_imports),
        remote_accounts: take(&mut object, "remote_accounts", defaults.remote_accounts),
        git_mode: take(&mut object, "git_mode", defaults.git_mode),
        git_path: take(&mut object, "git_path", defaults.git_path),
        keybindings: take(&mut object, "keybindings", defaults.keybindings),
        worktrees: take(&mut object, "worktrees", defaults.worktrees),
        worktree_recency: take(&mut object, "worktree_recency", defaults.worktree_recency),
    }
}

/// Parses the accounts, dropping entries that fail on their own, then
/// migrates the legacy `account_github` / `account_gitlab` names into records.
///
/// The legacy keys are removed from `object` and are not part of [`Settings`],
/// so the next save drops them from the file. A legacy account that already
/// has a record for the same service, instance and login is not added twice,
/// which makes loading a file that still holds the old keys repeatable.
fn take_accounts(object: &mut Map<String, Value>) -> Vec<AccountRecord> {
    let mut accounts = match object.remove("accounts") {
        None => Vec::new(),
        Some(Value::Array(items)) => items
            .into_iter()
            .filter_map(|item| match serde_json::from_value::<AccountRecord>(item) {
                Ok(account) => Some(account),
                Err(e) => {
                    log::warn!("dropping invalid account: {e}");
                    None
                }
            })
            .collect(),
        Some(_) => {
            log::warn!("ignoring invalid setting `accounts`: expected a list");
            Vec::new()
        }
    };
    for (key, kind, base_url) in [
        ("account_github", "github", "https://github.com"),
        ("account_gitlab", "gitlab", "https://gitlab.com"),
    ] {
        let login = take(object, key, String::new()).trim().to_owned();
        if login.is_empty() {
            continue;
        }
        let exists = accounts
            .iter()
            .any(|a| a.kind == kind && a.base_url == base_url && a.login == login);
        if !exists {
            accounts.push(AccountRecord {
                id: format!("{LEGACY_ACCOUNT_ID_PREFIX}{kind}"),
                kind: kind.to_owned(),
                base_url: base_url.to_owned(),
                label: login.clone(),
                login,
                method: AccountMethod::Token,
                ssh_host: None,
                imported_from: None,
            });
        }
    }
    accounts
}

/// Trims the text fields and drops records that cannot be used: an empty id,
/// a service this build does not know, or an id that repeats.
fn normalize_accounts(accounts: &mut Vec<AccountRecord>) {
    let mut seen = std::collections::HashSet::new();
    accounts.retain_mut(|account| {
        account.id = account.id.trim().to_owned();
        account.base_url = account.base_url.trim().trim_end_matches('/').to_owned();
        account.login = account.login.trim().to_owned();
        account.label = account.label.trim().to_owned();
        if account.label.is_empty() {
            account.label = account.login.clone();
        }
        !account.id.is_empty()
            && !account.base_url.is_empty()
            && forge::ServiceKind::from_id(&account.kind).is_some()
            && seen.insert(account.id.clone())
    });
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
            author_width: 180.0,
            date_width: 90.0,
            hash_width: 72.0,
            collapse_idle_lanes: true,
            activity_collapsed: true,
            diff_layout: DiffLayout::Split,
            background_fetch: false,
            show_activity: false,
            ..Settings::default()
        };
        store.save(&settings).unwrap();
        assert_eq!(store.load(), settings);
    }

    #[test]
    fn worktree_recency_is_kept_bounded_and_round_trips() {
        let dir = TempDir::new("worktree-recency");
        assert!(load_json(dir.path(), r#"{"theme":"dark"}"#)
            .worktree_recency
            .is_empty());

        let mut settings = Settings::default();
        settings.touch_worktree(Path::new("/w/a"), 10);
        settings.touch_worktree(Path::new("/w/a"), 20);
        assert_eq!(settings.worktree_recency["/w/a"], 20);
        for n in 0..WORKTREE_RECENCY_LIMIT as u64 {
            settings.touch_worktree(Path::new(&format!("/w/{n}")), 100 + n);
        }
        // The oldest entry ("/w/a", 20) was dropped at the limit.
        assert_eq!(settings.worktree_recency.len(), WORKTREE_RECENCY_LIMIT);
        assert!(!settings.worktree_recency.contains_key("/w/a"));

        let store = SettingsStore::new(settings_path(dir.path()));
        store.save(&settings).unwrap();
        assert_eq!(store.load().worktree_recency, settings.worktree_recency);
    }

    #[test]
    fn worktree_settings_default_without_the_key_and_round_trip() {
        let dir = TempDir::new("worktrees-settings");
        // An old file without the key keeps loading with the defaults.
        let old = load_json(dir.path(), r#"{"theme":"dark"}"#);
        assert_eq!(old.worktrees, WorktreeSettings::default());
        assert_eq!(old.worktrees.base, "~/develop");
        assert_eq!(old.worktrees.template, "{repo}-{branch}");
        assert_eq!(old.worktrees.after_create, AfterCreate::Switch);
        assert!(old.worktrees.copy_env);

        // A partial object fills the rest from the defaults.
        let partial = load_json(dir.path(), r#"{"worktrees":{"after_create":"stay"}}"#);
        assert_eq!(partial.worktrees.after_create, AfterCreate::Stay);
        assert_eq!(partial.worktrees.base, "~/develop");

        let store = SettingsStore::new(settings_path(dir.path()));
        let settings = Settings {
            worktrees: WorktreeSettings {
                base: "~/worktrees".to_owned(),
                template: "{branch}".to_owned(),
                after_create: AfterCreate::Stay,
                copy_env: false,
                confirm_remove_clean: false,
                confirm_delete_branch: false,
            },
            ..Settings::default()
        };
        store.save(&settings).unwrap();
        assert_eq!(store.load().worktrees, settings.worktrees);
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
        assert_eq!(defaults.author_width, AUTHOR_WIDTH_DEFAULT);
        assert_eq!(defaults.date_width, DATE_WIDTH_DEFAULT);
        assert_eq!(defaults.hash_width, HASH_WIDTH_DEFAULT);
        assert!(!defaults.collapse_idle_lanes);

        let dir = TempDir::new("graph-settings");
        // A file written before the graph settings existed loads the defaults.
        let old = load_json(dir.path(), r#"{"theme":"dark"}"#);
        assert_eq!(old.graph_width, GRAPH_WIDTH_DEFAULT);
        assert_eq!(old.author_width, AUTHOR_WIDTH_DEFAULT);
        assert_eq!(old.date_width, DATE_WIDTH_DEFAULT);
        assert_eq!(old.hash_width, HASH_WIDTH_DEFAULT);
        assert!(!old.collapse_idle_lanes);

        // Widths above the old 480 px graph cap are kept: dragging has no
        // fixed maximum, only the file sanity cap.
        assert_eq!(
            load_json(dir.path(), r#"{"graph_width":9999}"#).graph_width,
            9999.
        );
        assert_eq!(
            load_json(dir.path(), r#"{"graph_width":1}"#).graph_width,
            GRAPH_WIDTH_MIN
        );
        assert_eq!(
            load_json(dir.path(), r#"{"graph_width":"wide"}"#).graph_width,
            GRAPH_WIDTH_DEFAULT
        );
        assert_eq!(
            load_json(dir.path(), r#"{"author_width":2000}"#).author_width,
            2000.
        );
        assert_eq!(
            load_json(dir.path(), r#"{"author_width":1}"#).author_width,
            AUTHOR_WIDTH_MIN
        );
        assert_eq!(
            load_json(dir.path(), r#"{"date_width":1}"#).date_width,
            DATE_WIDTH_MIN
        );
        assert_eq!(
            load_json(dir.path(), r#"{"hash_width":1}"#).hash_width,
            HASH_WIDTH_MIN
        );
        assert_eq!(
            load_json(dir.path(), r#"{"hash_width":"wide"}"#).hash_width,
            HASH_WIDTH_DEFAULT
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
    fn activity_statistics_are_shown_by_default_and_remember_hiding() {
        assert!(Settings::default().show_activity);
        let dir = TempDir::new("show-activity");
        // An older settings file without the key keeps the feature on.
        assert!(load_json(dir.path(), r#"{"theme":"dark"}"#).show_activity);
        assert!(!load_json(dir.path(), r#"{"show_activity":false}"#).show_activity);
        let store = SettingsStore::new(settings_path(dir.path()));
        let hidden = Settings {
            show_activity: false,
            ..Settings::default()
        };
        store.save(&hidden).unwrap();
        assert!(!store.load().show_activity);
    }

    #[test]
    fn ci_status_is_shown_by_default_and_remember_hiding() {
        assert!(Settings::default().show_ci_status);
        let dir = TempDir::new("show-ci-status");
        // An older settings file without the key keeps CI status on.
        assert!(load_json(dir.path(), r#"{"theme":"dark"}"#).show_ci_status);
        assert!(!load_json(dir.path(), r#"{"show_ci_status":false}"#).show_ci_status);
        let store = SettingsStore::new(settings_path(dir.path()));
        let hidden = Settings {
            show_ci_status: false,
            ..Settings::default()
        };
        store.save(&hidden).unwrap();
        assert!(!store.load().show_ci_status);
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
            update_notified_version: Some("0.2.0".to_string()),
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
            accounts: vec![AccountRecord {
                id: "a1".to_owned(),
                kind: "gitlab".to_owned(),
                base_url: "https://gitlab.modsen.app".to_owned(),
                login: "mira.kovalenko".to_owned(),
                label: "Work".to_owned(),
                method: AccountMethod::Token,
                ssh_host: Some("ssh.modsen.app".to_owned()),
                imported_from: Some("gh".to_owned()),
            }],
            dismissed_imports: vec!["gh|github.com|mira-k".to_owned()],
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

    fn account(id: &str, kind: &str, url: &str, login: &str) -> AccountRecord {
        AccountRecord {
            id: id.to_owned(),
            kind: kind.to_owned(),
            base_url: url.to_owned(),
            login: login.to_owned(),
            label: login.to_owned(),
            method: AccountMethod::Token,
            ssh_host: None,
            imported_from: None,
        }
    }

    #[test]
    fn accounts_round_trip_without_secrets() {
        let dir = TempDir::new("accounts-round-trip");
        let store = SettingsStore::new(settings_path(dir.path()));
        let mut browser = account("b2", "github", "https://github.com", "mira-k");
        browser.method = AccountMethod::Browser;
        let settings = Settings {
            accounts: vec![
                account("a1", "gitea", "https://codeberg.org", "mira"),
                browser,
            ],
            ..Settings::default()
        };
        store.save(&settings).unwrap();
        assert_eq!(store.load(), settings);
        let json = fs::read_to_string(settings_path(dir.path())).unwrap();
        assert!(json.contains(r#""method": "browser""#), "{json}");
        assert!(!json.contains("token\":"), "no secret field: {json}");
    }

    #[test]
    fn import_origin_and_dismissed_imports_round_trip() {
        let dir = TempDir::new("accounts-import-origin");
        let store = SettingsStore::new(settings_path(dir.path()));
        let mut imported = account("a1", "github", "https://github.com", "mira-k");
        imported.imported_from = Some("glab".to_owned());
        let settings = Settings {
            accounts: vec![
                imported,
                account("a2", "gitea", "https://codeberg.org", "m"),
            ],
            dismissed_imports: vec!["gh|github.com|old".to_owned()],
            ..Settings::default()
        };
        store.save(&settings).unwrap();
        assert_eq!(store.load(), settings);
        let json = fs::read_to_string(settings_path(dir.path())).unwrap();
        assert_eq!(json.matches("imported_from").count(), 1, "{json}");
        // Files written before the fields existed load with the defaults.
        let old = load_json(
            dir.path(),
            r#"{"accounts":[{"id":"a1","kind":"github","base_url":"https://github.com","login":"m","label":"m","method":"token"}]}"#,
        );
        assert_eq!(old.accounts[0].imported_from, None);
        assert!(old.dismissed_imports.is_empty());
    }

    #[test]
    fn a_broken_account_is_dropped_alone() {
        let dir = TempDir::new("accounts-broken");
        let loaded = load_json(
            dir.path(),
            r#"{"accounts":[
                {"id":"a1","kind":"github","base_url":"https://github.com","login":"m","label":"m","method":"token"},
                {"id":"a2","kind":"github"},
                {"id":"a3","kind":"nope","base_url":"https://x.io","login":"m","label":"m","method":"token"},
                {"id":"a1","kind":"gitlab","base_url":"https://gitlab.com","login":"dup","label":"dup","method":"token"}]}"#,
        );
        let ids: Vec<&str> = loaded.accounts.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["a1"]);
    }

    #[test]
    fn legacy_account_names_become_records() {
        let dir = TempDir::new("accounts-legacy");
        let loaded = load_json(
            dir.path(),
            r#"{"account_github":" mira-k ","account_gitlab":"mira.kovalenko"}"#,
        );
        assert_eq!(loaded.accounts.len(), 2);
        let github = &loaded.accounts[0];
        assert_eq!(github.id, "legacy-github");
        assert_eq!(github.kind, "github");
        assert_eq!(github.base_url, "https://github.com");
        assert_eq!(github.login, "mira-k");
        assert_eq!(github.label, "mira-k");
        assert_eq!(github.method, AccountMethod::Token);
        let gitlab = &loaded.accounts[1];
        assert_eq!(gitlab.id, "legacy-gitlab");
        assert_eq!(gitlab.base_url, "https://gitlab.com");
        assert_eq!(gitlab.login, "mira.kovalenko");

        // Saving drops the legacy keys from the file; loading again changes nothing.
        let store = SettingsStore::new(settings_path(dir.path()));
        store.save(&loaded).unwrap();
        let json = fs::read_to_string(settings_path(dir.path())).unwrap();
        assert!(!json.contains("account_github"), "{json}");
        assert!(!json.contains("account_gitlab"), "{json}");
        assert_eq!(store.load(), loaded);
    }

    #[test]
    fn legacy_migration_is_idempotent_and_skips_empty_names() {
        let dir = TempDir::new("accounts-legacy-twice");
        let json = r#"{"account_github":"mira-k","account_gitlab":"  "}"#;
        let first = load_json(dir.path(), json);
        let second = load_json(dir.path(), json);
        assert_eq!(first.accounts, second.accounts);
        assert_eq!(first.accounts.len(), 1);

        // The legacy key next to a record that already covers it adds nothing.
        let both = load_json(
            dir.path(),
            r#"{"accounts":[{"id":"x1","kind":"github","base_url":"https://github.com",
                "login":"mira-k","label":"Me","method":"token"}],"account_github":"mira-k"}"#,
        );
        assert_eq!(both.accounts.len(), 1);
        assert_eq!(both.accounts[0].id, "x1");
    }

    #[test]
    fn remembered_remote_accounts_round_trip() {
        let dir = TempDir::new("remote-accounts-round-trip");
        let store = SettingsStore::new(settings_path(dir.path()));
        let mut settings = Settings::default();
        settings.set_remote_account("/home/me/app/.git", "origin", "a1");
        settings.set_remote_account("/home/me/app/.git", "upstream", "a2");
        settings.set_remote_account("/home/me/other/.git", "origin", "a1");
        store.save(&settings).unwrap();
        let loaded = store.load();
        assert_eq!(loaded, settings);
        assert_eq!(
            loaded.remote_accounts["/home/me/app/.git"]["upstream"],
            "a2"
        );
        // A file without the key (older versions) loads as no choices.
        assert!(load_json(dir.path(), "{}").remote_accounts.is_empty());
        // A malformed value falls back to the default instead of failing the load.
        assert!(load_json(dir.path(), r#"{"remote_accounts":[1]}"#)
            .remote_accounts
            .is_empty());
    }

    #[test]
    fn forgetting_an_account_drops_its_choices_and_empty_repositories() {
        let mut settings = Settings::default();
        settings.set_remote_account("/r1/.git", "origin", "a1");
        settings.set_remote_account("/r1/.git", "upstream", "a2");
        settings.set_remote_account("/r2/.git", "origin", "a1");
        settings.forget_account("a1");
        assert_eq!(settings.remote_accounts.len(), 1);
        assert_eq!(settings.remote_accounts["/r1/.git"].len(), 1);
        assert_eq!(settings.remote_accounts["/r1/.git"]["upstream"], "a2");
        // Forgetting an unknown id changes nothing.
        let before = settings.clone();
        settings.forget_account("zzz");
        assert_eq!(settings, before);
    }

    #[test]
    fn a_removed_remote_loses_its_remembered_account() {
        let names = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let mut settings = Settings::default();
        settings.set_remote_account("/r/.git", "origin", "a1");
        settings.set_remote_account("/r/.git", "upstream", "a2");
        settings.reconcile_remote_accounts(
            "/r/.git",
            &names(&["origin", "upstream"]),
            &names(&["origin"]),
        );
        assert_eq!(settings.remote_accounts["/r/.git"].len(), 1);
        assert_eq!(settings.remote_accounts["/r/.git"]["origin"], "a1");
        // The last choice going takes the repository's entry with it.
        settings.forget_remote("/r/.git", "origin");
        assert!(settings.remote_accounts.is_empty());
    }

    #[test]
    fn a_renamed_remote_keeps_its_remembered_account() {
        let names = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let mut settings = Settings::default();
        settings.set_remote_account("/r/.git", "origin", "a1");
        settings.set_remote_account("/r/.git", "upstream", "a2");
        settings.reconcile_remote_accounts(
            "/r/.git",
            &names(&["origin", "upstream"]),
            &names(&["main", "upstream"]),
        );
        let kept = &settings.remote_accounts["/r/.git"];
        assert_eq!(kept.get("main").map(String::as_str), Some("a1"));
        assert_eq!(kept.get("upstream").map(String::as_str), Some("a2"));
        assert!(!kept.contains_key("origin"));
        // A reload that changes nothing, or another repository, changes nothing.
        let before = settings.clone();
        settings.reconcile_remote_accounts(
            "/r/.git",
            &names(&["main", "upstream"]),
            &names(&["main", "upstream"]),
        );
        settings.reconcile_remote_accounts("/other/.git", &names(&["a"]), &names(&["b"]));
        assert_eq!(settings, before);
    }
}
