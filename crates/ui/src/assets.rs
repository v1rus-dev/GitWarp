//! Embedded assets: the icon set, served through GPUI's `AssetSource`.
//!
//! Everything is compiled in with `include_bytes!`; there is no filesystem
//! access at runtime.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

const FILES: &[(&str, &[u8])] = &[
    (
        "icons/branch.svg",
        include_bytes!("../assets/icons/branch.svg"),
    ),
    ("icons/tag.svg", include_bytes!("../assets/icons/tag.svg")),
    (
        "icons/cloud.svg",
        include_bytes!("../assets/icons/cloud.svg"),
    ),
    (
        "icons/stash.svg",
        include_bytes!("../assets/icons/stash.svg"),
    ),
    ("icons/box.svg", include_bytes!("../assets/icons/box.svg")),
    (
        "icons/search.svg",
        include_bytes!("../assets/icons/search.svg"),
    ),
    ("icons/down.svg", include_bytes!("../assets/icons/down.svg")),
    ("icons/up.svg", include_bytes!("../assets/icons/up.svg")),
    (
        "icons/fetch.svg",
        include_bytes!("../assets/icons/fetch.svg"),
    ),
    (
        "icons/chev-r.svg",
        include_bytes!("../assets/icons/chev-r.svg"),
    ),
    (
        "icons/chev-d.svg",
        include_bytes!("../assets/icons/chev-d.svg"),
    ),
    ("icons/x.svg", include_bytes!("../assets/icons/x.svg")),
    (
        "icons/check.svg",
        include_bytes!("../assets/icons/check.svg"),
    ),
    ("icons/warn.svg", include_bytes!("../assets/icons/warn.svg")),
    ("icons/err.svg", include_bytes!("../assets/icons/err.svg")),
    ("icons/info.svg", include_bytes!("../assets/icons/info.svg")),
    (
        "icons/folder.svg",
        include_bytes!("../assets/icons/folder.svg"),
    ),
    ("icons/file.svg", include_bytes!("../assets/icons/file.svg")),
    ("icons/plus.svg", include_bytes!("../assets/icons/plus.svg")),
    (
        "icons/minus.svg",
        include_bytes!("../assets/icons/minus.svg"),
    ),
    ("icons/gear.svg", include_bytes!("../assets/icons/gear.svg")),
    (
        "icons/panel-r.svg",
        include_bytes!("../assets/icons/panel-r.svg"),
    ),
    (
        "icons/panel-b.svg",
        include_bytes!("../assets/icons/panel-b.svg"),
    ),
    (
        "icons/sidebar.svg",
        include_bytes!("../assets/icons/sidebar.svg"),
    ),
    (
        "icons/split.svg",
        include_bytes!("../assets/icons/split.svg"),
    ),
    (
        "icons/unified.svg",
        include_bytes!("../assets/icons/unified.svg"),
    ),
    ("icons/git.svg", include_bytes!("../assets/icons/git.svg")),
    (
        "icons/command.svg",
        include_bytes!("../assets/icons/command.svg"),
    ),
    ("icons/copy.svg", include_bytes!("../assets/icons/copy.svg")),
    (
        "icons/clone.svg",
        include_bytes!("../assets/icons/clone.svg"),
    ),
    ("icons/init.svg", include_bytes!("../assets/icons/init.svg")),
    ("icons/min.svg", include_bytes!("../assets/icons/min.svg")),
    ("icons/max.svg", include_bytes!("../assets/icons/max.svg")),
    ("icons/undo.svg", include_bytes!("../assets/icons/undo.svg")),
    ("icons/edit.svg", include_bytes!("../assets/icons/edit.svg")),
    (
        "icons/trash.svg",
        include_bytes!("../assets/icons/trash.svg"),
    ),
    ("icons/link.svg", include_bytes!("../assets/icons/link.svg")),
    (
        "icons/lanes.svg",
        include_bytes!("../assets/icons/lanes.svg"),
    ),
    ("icons/eye.svg", include_bytes!("../assets/icons/eye.svg")),
    ("icons/play.svg", include_bytes!("../assets/icons/play.svg")),
    ("icons/user.svg", include_bytes!("../assets/icons/user.svg")),
    ("icons/hash.svg", include_bytes!("../assets/icons/hash.svg")),
    (
        "icons/keyboard.svg",
        include_bytes!("../assets/icons/keyboard.svg"),
    ),
    ("icons/key.svg", include_bytes!("../assets/icons/key.svg")),
    (
        "icons/worktree.svg",
        include_bytes!("../assets/icons/worktree.svg"),
    ),
    ("icons/lock.svg", include_bytes!("../assets/icons/lock.svg")),
    (
        "icons/palette.svg",
        include_bytes!("../assets/icons/palette.svg"),
    ),
    (
        "icons/github.svg",
        include_bytes!("../assets/icons/github.svg"),
    ),
    (
        "icons/external.svg",
        include_bytes!("../assets/icons/external.svg"),
    ),
    ("icons/pr.svg", include_bytes!("../assets/icons/pr.svg")),
    (
        "icons/pr-draft.svg",
        include_bytes!("../assets/icons/pr-draft.svg"),
    ),
    (
        "icons/merge.svg",
        include_bytes!("../assets/icons/merge.svg"),
    ),
    (
        "icons/pr-closed.svg",
        include_bytes!("../assets/icons/pr-closed.svg"),
    ),
    (
        "icons/check-circle-fill.svg",
        include_bytes!("../assets/icons/check-circle-fill.svg"),
    ),
    (
        "icons/x-circle-fill.svg",
        include_bytes!("../assets/icons/x-circle-fill.svg"),
    ),
    ("icons/sync.svg", include_bytes!("../assets/icons/sync.svg")),
    (
        "icons/hourglass.svg",
        include_bytes!("../assets/icons/hourglass.svg"),
    ),
    ("icons/skip.svg", include_bytes!("../assets/icons/skip.svg")),
    (
        "icons/comment.svg",
        include_bytes!("../assets/icons/comment.svg"),
    ),
];

/// The embedded asset source.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(FILES
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(FILES
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::from(*name))
            .collect())
    }
}
