//! Settings → Worktrees: where new worktrees go, what happens after creating
//! one and which removals ask first.

use std::path::Path;

use gpui::{div, px, AnyElement, Context, IntoElement, ParentElement, Styled, Window};

use super::form;
use super::{group, heading, SettingsView};
use crate::settings::{AfterCreate, WORKTREE_BASES, WORKTREE_TEMPLATES};

/// The design's example branch for the live folder-name hint.
const EXAMPLE_BRANCH: &str = "fix-git-detect";

/// The label of a base folder in the select.
fn base_label(base: &str) -> &str {
    if base == WORKTREE_BASES[0] {
        "~/develop (next to the repository)"
    } else {
        base
    }
}

fn position(options: &[&str], value: &str) -> usize {
    options.iter().position(|o| *o == value).unwrap_or(0)
}

pub(super) fn render(
    view: &mut SettingsView,
    _window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> AnyElement {
    let settings = view.settings(cx);
    let wt = settings.worktrees;
    let mut page = div().flex().flex_col();

    page = page.child(heading(
        "Worktrees",
        "Where new worktrees go and what happens after you create one. Applies to quick create and the switcher; the Add worktree dialog still lets you pick any folder.",
    ));

    let repo = view
        .root
        .as_deref()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "gitwarp".to_owned());
    let example = crate::worktree_path::worktree_path_for(
        Path::new(&wt.base),
        &wt.template,
        &repo,
        EXAMPLE_BRANCH,
        |_| false,
    );

    let base = {
        let entity = cx.entity();
        form::select(
            "settings-wt-base".into(),
            WORKTREE_BASES.iter().map(|base| base_label(base)),
            position(&WORKTREE_BASES, &wt.base),
            false,
            move |index, _, cx| {
                let base = WORKTREE_BASES[index.min(WORKTREE_BASES.len() - 1)];
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| settings.worktrees.base = base.to_owned());
                });
            },
        )
    };
    let template = {
        let entity = cx.entity();
        form::select(
            "settings-wt-template".into(),
            WORKTREE_TEMPLATES.iter().copied(),
            position(&WORKTREE_TEMPLATES, &wt.template),
            false,
            move |index, _, cx| {
                let template = WORKTREE_TEMPLATES[index.min(WORKTREE_TEMPLATES.len() - 1)];
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| {
                        settings.worktrees.template = template.to_owned();
                    });
                });
            },
        )
    };
    page = page.child(group(
        "Location",
        vec![
            form::row(
                "settings-wt-base-row",
                "Base folder",
                "New worktrees are created inside this folder, never inside the repository.",
                base,
                false,
            ),
            form::row(
                "settings-wt-template-row",
                "Folder name",
                format!("Example: {}", example.display()),
                template,
                false,
            ),
        ],
    ));

    let after = {
        let entity = cx.entity();
        form::segmented(
            "settings-wt-after".into(),
            ["Switch to it", "Stay here"],
            usize::from(wt.after_create == AfterCreate::Stay),
            false,
            move |index, _, cx| {
                let after = if index == 1 {
                    AfterCreate::Stay
                } else {
                    AfterCreate::Switch
                };
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| settings.worktrees.after_create = after);
                });
            },
        )
    };
    let copy_env = {
        let entity = cx.entity();
        form::switch(
            "settings-wt-env".into(),
            wt.copy_env,
            false,
            move |checked, _, cx| {
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| settings.worktrees.copy_env = checked);
                });
            },
        )
    };
    page = page.child(group(
        "After creating",
        vec![
            form::row(
                "settings-wt-after-row",
                "When the worktree is ready",
                "Switching keeps your current worktree untouched — its changes stay where they are.",
                after,
                false,
            ),
            form::row(
                "settings-wt-env-row",
                "Copy untracked config files",
                "Copies .env and .env.local into the new worktree so it runs right away. Ignored build output is never copied.",
                copy_env,
                false,
            ),
        ],
    ));

    let confirm_remove = {
        let entity = cx.entity();
        form::checkbox(
            "settings-wt-confirm-remove".into(),
            "Confirm",
            wt.confirm_remove_clean,
            false,
            move |checked, _, cx| {
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| {
                        settings.worktrees.confirm_remove_clean = checked;
                    });
                });
            },
        )
    };
    let confirm_branch = {
        let entity = cx.entity();
        form::checkbox(
            "settings-wt-confirm-branch".into(),
            "Confirm",
            wt.confirm_delete_branch,
            false,
            move |checked, _, cx| {
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| {
                        settings.worktrees.confirm_delete_branch = checked;
                    });
                });
            },
        )
    };
    page = page.child(group(
        "Ask before…",
        vec![
            form::row(
                "settings-wt-confirm-remove-row",
                "Removing a worktree",
                "Always shows unsaved changes first; this only controls the confirmation for clean worktrees.",
                confirm_remove,
                false,
            ),
            form::row(
                "settings-wt-confirm-branch-row",
                "Deleting its branch together with the worktree",
                "Unmerged commits stay reachable from the reflog for 30 days.",
                confirm_branch,
                false,
            ),
        ],
    ));

    let path = crate::settings::config_dir()
        .map(|dir| dir.join("settings.json"))
        .map(|path| crate::format::display_path(&path, view.home.as_deref()))
        .unwrap_or_else(|| "settings.json".to_owned());
    page.child(
        div()
            .max_w(px(760.))
            .flex()
            .items_center()
            .gap(px(4.))
            .text_size(ui::metrics::FS_SM)
            .child(ui::Icon::new(ui::IconName::Info).size(px(12.)))
            .child("Settings file: ")
            .child(div().font_family(ui::metrics::FONT_MONO).child(path)),
    )
    .into_any_element()
}
