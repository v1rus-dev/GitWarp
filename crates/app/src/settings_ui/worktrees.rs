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
fn base_label(base: &str) -> gpui::SharedString {
    if base == WORKTREE_BASES[0] {
        ui::t!("settings.worktrees.base_next_to_repo")
    } else {
        base.to_owned().into()
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
        ui::t!("settings.page.worktrees"),
        ui::t!("settings.worktrees.subtitle"),
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
        ui::t!("settings.worktrees.group.location"),
        vec![
            form::row(
                "settings-wt-base-row",
                ui::t!("settings.worktrees.base.title"),
                ui::t!("settings.worktrees.base.desc"),
                base,
                false,
            ),
            form::row(
                "settings-wt-template-row",
                ui::t!("settings.worktrees.template.title"),
                ui::t!(
                    "settings.worktrees.template.example",
                    path = example.display()
                ),
                template,
                false,
            ),
        ],
    ));

    let after = {
        let entity = cx.entity();
        form::segmented(
            "settings-wt-after".into(),
            [
                ui::t!("settings.worktrees.after.switch"),
                ui::t!("settings.worktrees.after.stay"),
            ],
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
        ui::t!("settings.worktrees.group.after"),
        vec![
            form::row(
                "settings-wt-after-row",
                ui::t!("settings.worktrees.ready.title"),
                ui::t!("settings.worktrees.ready.desc"),
                after,
                false,
            ),
            form::row(
                "settings-wt-env-row",
                ui::t!("settings.worktrees.copy.title"),
                ui::t!("settings.worktrees.copy.desc"),
                copy_env,
                false,
            ),
        ],
    ));

    let confirm_remove = {
        let entity = cx.entity();
        form::checkbox(
            "settings-wt-confirm-remove".into(),
            ui::t!("settings.general.confirm.label"),
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
            ui::t!("settings.general.confirm.label"),
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
        ui::t!("settings.general.group.confirm"),
        vec![
            form::row(
                "settings-wt-confirm-remove-row",
                ui::t!("settings.worktrees.remove.title"),
                ui::t!("settings.worktrees.remove.desc"),
                confirm_remove,
                false,
            ),
            form::row(
                "settings-wt-confirm-branch-row",
                ui::t!("settings.worktrees.delete_branch.title"),
                ui::t!("settings.general.confirm.branch.desc"),
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
            .child(ui::t!("settings.general.file_label"))
            .child(div().font_family(ui::metrics::FONT_MONO).child(path)),
    )
    .into_any_element()
}
