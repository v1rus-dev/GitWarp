//! The icon set, embedded as SVG assets: Primer Octicons 16px (MIT, see
//! `assets/icons/OCTICONS-LICENSE`), plus our own git, keyboard, command and
//! window-control glyphs, which Octicons does not have.

use gpui::{px, svg, App, Hsla, IntoElement, Pixels, RenderOnce, Styled, Window};

use crate::theme::ActiveTheme;

/// Every icon of the design system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconName {
    Branch,
    Tag,
    Cloud,
    Stash,
    Box,
    Search,
    Down,
    Up,
    Fetch,
    ChevR,
    ChevD,
    X,
    Check,
    Warn,
    Err,
    Info,
    Folder,
    File,
    Plus,
    Minus,
    Gear,
    PanelR,
    PanelB,
    Sidebar,
    Split,
    Unified,
    Git,
    Command,
    Copy,
    Clone,
    Init,
    Min,
    Max,
    Undo,
    Edit,
    Trash,
    Link,
    Lanes,
    Eye,
    Play,
    User,
    Hash,
    Keyboard,
    Key,
    Worktree,
    Lock,
    Palette,
    Github,
    External,
    Pr,
    PrDraft,
    PrClosed,
    Merge,
    CheckFill,
    XFill,
    Sync,
    Hourglass,
    Skip,
    Comment,
    Repo,
    RepoForked,
    Archive,
}

impl IconName {
    /// All 62 icons, in the design's order.
    pub const ALL: [IconName; 62] = [
        Self::Branch,
        Self::Tag,
        Self::Cloud,
        Self::Stash,
        Self::Box,
        Self::Search,
        Self::Down,
        Self::Up,
        Self::Fetch,
        Self::ChevR,
        Self::ChevD,
        Self::X,
        Self::Check,
        Self::Warn,
        Self::Err,
        Self::Info,
        Self::Folder,
        Self::File,
        Self::Plus,
        Self::Minus,
        Self::Gear,
        Self::PanelR,
        Self::PanelB,
        Self::Sidebar,
        Self::Split,
        Self::Unified,
        Self::Git,
        Self::Command,
        Self::Copy,
        Self::Clone,
        Self::Init,
        Self::Min,
        Self::Max,
        Self::Undo,
        Self::Edit,
        Self::Trash,
        Self::Link,
        Self::Lanes,
        Self::Eye,
        Self::Play,
        Self::User,
        Self::Hash,
        Self::Keyboard,
        Self::Key,
        Self::Worktree,
        Self::Lock,
        Self::Palette,
        Self::Github,
        Self::External,
        Self::Pr,
        Self::PrDraft,
        Self::PrClosed,
        Self::Merge,
        Self::CheckFill,
        Self::XFill,
        Self::Sync,
        Self::Hourglass,
        Self::Skip,
        Self::Comment,
        Self::Repo,
        Self::RepoForked,
        Self::Archive,
    ];

    /// Asset path, e.g. `icons/chev-r.svg`.
    pub fn path(self) -> &'static str {
        match self {
            Self::Branch => "icons/branch.svg",
            Self::Tag => "icons/tag.svg",
            Self::Cloud => "icons/cloud.svg",
            Self::Stash => "icons/stash.svg",
            Self::Box => "icons/box.svg",
            Self::Search => "icons/search.svg",
            Self::Down => "icons/down.svg",
            Self::Up => "icons/up.svg",
            Self::Fetch => "icons/fetch.svg",
            Self::ChevR => "icons/chev-r.svg",
            Self::ChevD => "icons/chev-d.svg",
            Self::X => "icons/x.svg",
            Self::Check => "icons/check.svg",
            Self::Warn => "icons/warn.svg",
            Self::Err => "icons/err.svg",
            Self::Info => "icons/info.svg",
            Self::Folder => "icons/folder.svg",
            Self::File => "icons/file.svg",
            Self::Plus => "icons/plus.svg",
            Self::Minus => "icons/minus.svg",
            Self::Gear => "icons/gear.svg",
            Self::PanelR => "icons/panel-r.svg",
            Self::PanelB => "icons/panel-b.svg",
            Self::Sidebar => "icons/sidebar.svg",
            Self::Split => "icons/split.svg",
            Self::Unified => "icons/unified.svg",
            Self::Git => "icons/git.svg",
            Self::Command => "icons/command.svg",
            Self::Copy => "icons/copy.svg",
            Self::Clone => "icons/clone.svg",
            Self::Init => "icons/init.svg",
            Self::Min => "icons/min.svg",
            Self::Max => "icons/max.svg",
            Self::Undo => "icons/undo.svg",
            Self::Edit => "icons/edit.svg",
            Self::Trash => "icons/trash.svg",
            Self::Link => "icons/link.svg",
            Self::Lanes => "icons/lanes.svg",
            Self::Eye => "icons/eye.svg",
            Self::Play => "icons/play.svg",
            Self::User => "icons/user.svg",
            Self::Hash => "icons/hash.svg",
            Self::Keyboard => "icons/keyboard.svg",
            Self::Key => "icons/key.svg",
            Self::Worktree => "icons/worktree.svg",
            Self::Lock => "icons/lock.svg",
            Self::Palette => "icons/palette.svg",
            Self::Github => "icons/github.svg",
            Self::External => "icons/external.svg",
            Self::Pr => "icons/pr.svg",
            Self::PrDraft => "icons/pr-draft.svg",
            Self::PrClosed => "icons/pr-closed.svg",
            Self::Merge => "icons/merge.svg",
            Self::CheckFill => "icons/check-circle-fill.svg",
            Self::XFill => "icons/x-circle-fill.svg",
            Self::Sync => "icons/sync.svg",
            Self::Hourglass => "icons/hourglass.svg",
            Self::Skip => "icons/skip.svg",
            Self::Comment => "icons/comment.svg",
            Self::Repo => "icons/repo.svg",
            Self::RepoForked => "icons/repo-forked.svg",
            Self::Archive => "icons/archive.svg",
        }
    }
}

/// An icon drawn from the embedded set.
#[derive(IntoElement)]
pub struct Icon {
    name: IconName,
    size: Pixels,
    color: Option<Hsla>,
}

impl Icon {
    pub fn new(name: IconName) -> Self {
        Self {
            name,
            size: px(14.),
            color: None,
        }
    }

    /// Edge length; 14px by default.
    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }

    /// Defaults to the theme's text colour.
    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl RenderOnce for Icon {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = self.color.unwrap_or(cx.theme().colors.text);
        svg()
            .path(self.name.path())
            .w(self.size)
            .h(self.size)
            .text_color(color)
    }
}
