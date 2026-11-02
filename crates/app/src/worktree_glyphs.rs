//! The status glyphs a worktree row carries: locked, folder missing, local
//! changes and ahead/behind. The sidebar's worktree panel draws them today and
//! the titlebar switcher reuses them.
//!
//! Every glyph is an icon or a number, never colour alone, and has a tooltip
//! that says it in words.

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, Div, ElementId, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled,
};

use ui::metrics::FS_SM;
use ui::theme::ThemeColors;
use ui::{Icon, IconName, Tooltip};

use crate::worktrees_model::WorktreeEntry;

/// The tooltip of the main worktree's row.
pub const MAIN_WORKTREE_TIP: &str =
    "Git calls it the main worktree: the original folder of this repository. It cannot be removed.";

/// One status glyph of a worktree row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorktreeGlyph {
    /// Locked, with the reason when there is one.
    Locked(String),
    /// The folder is gone.
    Missing,
    /// This many files have local changes.
    Changed(usize),
    /// Commits ahead of / behind the upstream.
    AheadBehind { ahead: usize, behind: usize },
}

impl WorktreeGlyph {
    /// The tooltip text.
    pub fn tooltip(&self) -> String {
        match self {
            WorktreeGlyph::Locked(reason) if reason.is_empty() => "Locked".to_owned(),
            WorktreeGlyph::Locked(reason) => format!("Locked: {reason}"),
            WorktreeGlyph::Missing => "Folder is missing".to_owned(),
            WorktreeGlyph::Changed(count) => format!("{count} changed files"),
            WorktreeGlyph::AheadBehind { ahead, behind } => {
                format!("{ahead} ahead, {behind} behind")
            }
        }
    }
}

/// The glyphs of `entry`, in the design's order.
pub fn glyphs_of(entry: &WorktreeEntry) -> Vec<WorktreeGlyph> {
    let mut glyphs = Vec::new();
    if let Some(reason) = entry.locked() {
        glyphs.push(WorktreeGlyph::Locked(reason.to_owned()));
    }
    if entry.missing {
        glyphs.push(WorktreeGlyph::Missing);
    }
    if !entry.changes.is_empty() {
        glyphs.push(WorktreeGlyph::Changed(entry.changes.len()));
    }
    if entry.ahead > 0 || entry.behind > 0 {
        glyphs.push(WorktreeGlyph::AheadBehind {
            ahead: entry.ahead,
            behind: entry.behind,
        });
    }
    glyphs
}

/// The glyph row of `entry`. `id` must be unique per row; `selected` recolours
/// the glyphs for a selection background (the missing-folder glyph keeps its
/// error colour, as in the design).
pub fn worktree_glyphs(
    id: &str,
    entry: &WorktreeEntry,
    selected: bool,
    colors: ThemeColors,
) -> Div {
    let warn = if selected {
        colors.selection_fg
    } else {
        colors.warning
    };
    let quiet = if selected {
        colors.selection_fg
    } else {
        colors.muted
    };
    let mut row = div().flex().flex_none().items_center().gap(px(6.));
    for (index, glyph) in glyphs_of(entry).into_iter().enumerate() {
        let tip = glyph.tooltip();
        let element_id = ElementId::named_usize(format!("{id}-glyph"), index);
        let body = div()
            .id(element_id)
            .flex()
            .items_center()
            .gap(px(3.))
            .text_size(FS_SM)
            .line_height(FS_SM);
        let body = match &glyph {
            WorktreeGlyph::Locked(_) => {
                body.child(Icon::new(IconName::Lock).size(px(12.)).color(warn))
            }
            WorktreeGlyph::Missing => {
                body.child(Icon::new(IconName::Warn).size(px(12.)).color(colors.error))
            }
            WorktreeGlyph::Changed(count) => body
                .text_color(warn)
                .child(Icon::new(IconName::Edit).size(px(12.)).color(warn))
                .child(count.to_string()),
            WorktreeGlyph::AheadBehind { ahead, behind } => body
                .gap(px(6.))
                .text_color(quiet)
                .when(*ahead > 0, |this| this.child(format!("↑{ahead}")))
                .when(*behind > 0, |this| this.child(format!("↓{behind}"))),
        };
        row = row.child(body.tooltip(Tooltip::text(tip)));
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;
    use git_core::{ChangedEntry, Worktree};
    use std::path::PathBuf;

    fn entry() -> WorktreeEntry {
        WorktreeEntry {
            worktree: Worktree {
                path: PathBuf::from("/tmp/wt"),
                branch: Some("main".to_owned()),
                head: None,
                bare: false,
                detached: false,
                locked: None,
                prunable: false,
                current: false,
                main: false,
            },
            changes: Vec::new(),
            missing: false,
            head_summary: None,
            ahead: 0,
            behind: 0,
            has_upstream: false,
            last_used: None,
        }
    }

    #[test]
    fn a_clean_synced_worktree_has_no_glyphs() {
        assert!(glyphs_of(&entry()).is_empty());
    }

    #[test]
    fn glyphs_come_in_the_designs_order_with_its_tooltips() {
        let mut wt = entry();
        wt.worktree.locked = Some("review in progress".to_owned());
        wt.missing = true;
        wt.changes = vec![ChangedEntry {
            status: 'M',
            path: "a.txt".to_owned(),
        }];
        wt.ahead = 1;
        wt.behind = 2;
        let tips: Vec<String> = glyphs_of(&wt).iter().map(WorktreeGlyph::tooltip).collect();
        assert_eq!(
            tips,
            [
                "Locked: review in progress",
                "Folder is missing",
                "1 changed files",
                "1 ahead, 2 behind"
            ]
        );
    }

    #[test]
    fn a_lock_without_a_reason_says_just_locked() {
        let mut wt = entry();
        wt.worktree.locked = Some(String::new());
        assert_eq!(glyphs_of(&wt), [WorktreeGlyph::Locked(String::new())]);
        assert_eq!(glyphs_of(&wt)[0].tooltip(), "Locked");
    }
}
