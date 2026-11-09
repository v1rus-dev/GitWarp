//! The renderer-independent application menu model.
//!
//! macOS builds the native menu bar from it; other platforms render the same
//! model as an in-window bar.

use gpui::{Action, SharedString};

use crate::repo_view::{
    FetchAll, FocusSearch, OpenBranchSwitcher, OpenWorktreeSwitcher, PullBranch, PushBranch,
    Refresh, StashChanges, ToggleDetails, ToggleSidebar,
};
use crate::workspace::{
    CloseRepository, CloseWindow, MinimizeWindow, OpenCloneDialog, OpenRepositoryDialog,
    OpenSettings, ShowAbout, ZoomWindow,
};

/// One row of a menu.
pub(crate) enum Entry {
    Separator,
    Item {
        label: SharedString,
        action: Box<dyn Action>,
    },
    /// Reserved for later steps; renderers must handle it.
    #[allow(dead_code)]
    Submenu(MenuSpec),
}

/// A top-level menu or a submenu.
pub(crate) struct MenuSpec {
    pub name: SharedString,
    pub entries: Vec<Entry>,
}

fn item(label: SharedString, action: impl Action) -> Entry {
    Entry::Item {
        label,
        action: Box::new(action),
    }
}

/// The menus in display order. `mac` drops what the macOS app menu already
/// holds (Settings, Quit).
pub(crate) fn menus(mac: bool) -> Vec<MenuSpec> {
    use Entry::Separator;
    let mut file = vec![
        item(ui::t!("app.menu.open_repository"), OpenRepositoryDialog),
        item(ui::t!("app.menu.clone"), OpenCloneDialog),
        item(
            ui::t!("app.menu.new_repository"),
            crate::welcome::NewRepository,
        ),
        Separator,
        item(ui::t!("app.menu.close_repository"), CloseRepository),
        item(ui::t!("app.menu.close_window"), CloseWindow),
    ];
    if !mac {
        file.extend([
            Separator,
            item(ui::t!("app.menu.settings"), OpenSettings),
            Separator,
            item(ui::t!("app.menu.exit"), crate::Quit),
        ]);
    }
    let mut menus = vec![
        MenuSpec {
            name: ui::t!("app.menu.file"),
            entries: file,
        },
        MenuSpec {
            name: ui::t!("app.menu.view"),
            entries: vec![
                item(ui::t!("app.menu.toggle_sidebar"), ToggleSidebar),
                item(ui::t!("app.menu.toggle_details"), ToggleDetails),
                Separator,
                item(ui::t!("app.menu.palette"), crate::palette::OpenPalette),
                item(ui::t!("app.menu.search"), FocusSearch),
            ],
        },
        MenuSpec {
            name: ui::t!("app.menu.repository"),
            entries: vec![
                item(ui::t!("app.menu.refresh"), Refresh),
                Separator,
                item(ui::t!("app.menu.fetch_all"), FetchAll),
                item(ui::t!("app.menu.pull"), PullBranch),
                item(ui::t!("app.menu.push"), PushBranch),
                Separator,
                item(ui::t!("app.menu.stash"), StashChanges),
                Separator,
                item(ui::t!("app.menu.switch_branch"), OpenBranchSwitcher),
                item(ui::t!("app.menu.switch_worktree"), OpenWorktreeSwitcher),
            ],
        },
        MenuSpec {
            name: ui::t!("app.menu.window"),
            entries: vec![
                item(ui::t!("app.menu.minimize"), MinimizeWindow),
                item(ui::t!("app.menu.zoom"), ZoomWindow),
            ],
        },
    ];
    // macOS keeps About in the app menu.
    if !mac {
        menus.push(MenuSpec {
            name: ui::t!("app.menu.help"),
            entries: vec![item(ui::t!("app.menu.about"), ShowAbout)],
        });
    }
    menus
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(mac: bool) -> MenuSpec {
        menus(mac).into_iter().next().unwrap()
    }

    fn has_action(spec: &MenuSpec, name: &str) -> bool {
        spec.entries.iter().any(|e| match e {
            Entry::Item { action, .. } => action.name().ends_with(name),
            _ => false,
        })
    }

    #[test]
    fn settings_and_exit_only_off_mac() {
        assert!(!has_action(&file(true), "OpenSettings"));
        assert!(!has_action(&file(true), "Quit"));
        assert!(has_action(&file(false), "OpenSettings"));
        assert!(has_action(&file(false), "Quit"));
    }

    fn check(entries: &[Entry]) {
        assert!(!matches!(entries.first(), Some(Entry::Separator)));
        assert!(!matches!(entries.last(), Some(Entry::Separator)));
        for pair in entries.windows(2) {
            assert!(!(matches!(pair[0], Entry::Separator) && matches!(pair[1], Entry::Separator)));
        }
        for e in entries {
            match e {
                Entry::Item { label, .. } => {
                    assert!(!label.is_empty());
                    assert!(!label.starts_with("app.menu."), "missing key: {label}");
                }
                Entry::Submenu(s) => check(&s.entries),
                Entry::Separator => {}
            }
        }
    }

    #[test]
    fn labels_resolve_and_separators_are_tidy() {
        for mac in [true, false] {
            for m in menus(mac) {
                assert!(!m.name.starts_with("app.menu."), "missing key: {}", m.name);
                check(&m.entries);
            }
        }
    }

    #[test]
    fn help_with_about_only_off_mac() {
        assert!(!menus(true).iter().any(|m| has_action(m, "ShowAbout")));
        let all = menus(false);
        let help = all.last().unwrap();
        assert_eq!(&*help.name, &*ui::t!("app.menu.help"));
        assert_eq!(help.entries.len(), 1);
        assert!(has_action(help, "ShowAbout"));
        assert_eq!(all.iter().filter(|m| has_action(m, "ShowAbout")).count(), 1);
    }

    #[test]
    fn actions_are_wired() {
        let f = file(false);
        assert!(has_action(&f, "OpenCloneDialog"));
        assert!(has_action(&f, "Quit"));
    }
}
