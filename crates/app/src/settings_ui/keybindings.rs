//! Settings → Keybindings: the searchable shortcut table, recording, conflict
//! handling and export. Overrides live in `settings.json` and are applied when
//! the app starts.

use std::io::Write;

use gpui::prelude::FluentBuilder;
use gpui::InteractiveElement;
use gpui::{div, px, AnyElement, Context, IntoElement, ParentElement, Styled, Window};

use ui::metrics::{FS_BASE, FS_SM, R_SM};
use ui::theme::ActiveTheme;
use ui::{Button, ButtonSize, Icon, IconName, Kbd};

use super::form;
use super::{heading, SettingsView};

/// One bindable action. The id is stable and is the key in `settings.json`.
#[derive(Clone, Copy, Debug)]
pub struct Action {
    pub id: &'static str,
    pub group: &'static str,
    pub name: &'static str,
    pub default: &'static str,
}

pub const ACTIONS: [Action; 15] = [
    Action {
        id: "palette",
        group: "General",
        name: "Command palette",
        default: "ctrl-p",
    },
    Action {
        id: "search",
        group: "General",
        name: "Search commits",
        default: "ctrl-k",
    },
    Action {
        id: "settings",
        group: "General",
        name: "Open settings",
        default: "ctrl-,",
    },
    Action {
        id: "sidebar",
        group: "View",
        name: "Toggle sidebar",
        default: "ctrl-b",
    },
    Action {
        id: "dock",
        group: "View",
        name: "Toggle details dock",
        default: "ctrl-j",
    },
    Action {
        id: "branch",
        group: "Repository",
        name: "Switch branch",
        default: "ctrl-shift-b",
    },
    Action {
        id: "worktree_switcher",
        group: "Repository",
        name: "Switch worktree",
        default: "ctrl-alt-w",
    },
    Action {
        id: "fetch",
        group: "Repository",
        name: "Fetch",
        default: "ctrl-shift-f",
    },
    Action {
        id: "pull",
        group: "Repository",
        name: "Pull",
        default: "ctrl-shift-l",
    },
    Action {
        id: "push",
        group: "Repository",
        name: "Push",
        default: "ctrl-shift-p",
    },
    Action {
        id: "next",
        group: "History",
        name: "Next commit",
        default: "j",
    },
    Action {
        id: "prev",
        group: "History",
        name: "Previous commit",
        default: "k",
    },
    Action {
        id: "stage",
        group: "Changes",
        name: "Stage / unstage file",
        default: "space",
    },
    Action {
        id: "commit",
        group: "Changes",
        name: "Commit",
        default: "ctrl-enter",
    },
    Action {
        id: "discard",
        group: "Changes",
        name: "Discard changes",
        default: "ctrl-shift-z",
    },
];

const GROUPS: [&str; 6] = ["All", "General", "View", "Repository", "History", "Changes"];

/// A captured shortcut that another action already uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub id: &'static str,
    pub keys: String,
    pub other: &'static str,
    pub other_id: &'static str,
}

/// The shortcut currently effective for `action`.
pub fn current(settings: &crate::settings::Settings, action: &Action) -> String {
    settings
        .keybindings
        .get(action.id)
        .cloned()
        .unwrap_or_else(|| action.default.to_owned())
}

fn action(id: &str) -> Option<&'static Action> {
    ACTIONS.iter().find(|action| action.id == id)
}

/// The key cap text (`Ctrl Shift L`) of the shortcut effective for `id`:
/// the user's override from the settings, else the default. `None` for an
/// unknown id (an empty override counts as the default, as at startup).
pub fn key_cap(cx: &gpui::App, id: &str) -> Option<String> {
    let action = action(id)?;
    let keys = cx
        .try_global::<crate::AppSettings>()
        .and_then(|state| state.settings.keybindings.get(id))
        .filter(|keys| !keys.is_empty())
        .map_or(action.default, String::as_str);
    Some(display(keys).replace('+', " "))
}

/// Formats the stored GPUI syntax as the design's `Ctrl + Shift + B` caps.
pub fn display(keys: &str) -> String {
    if keys.is_empty() {
        return "Unbound".to_owned();
    }
    keys.split('-')
        .map(|part| match part {
            "ctrl" => "Ctrl".to_owned(),
            "shift" => "Shift".to_owned(),
            "alt" => "Alt".to_owned(),
            "cmd" | "super" | "platform" => "Cmd".to_owned(),
            "space" => "Space".to_owned(),
            "enter" => "Enter".to_owned(),
            "escape" => "Esc".to_owned(),
            "tab" => "Tab".to_owned(),
            "backspace" => "Backspace".to_owned(),
            "delete" => "Del".to_owned(),
            "up" => "↑".to_owned(),
            "down" => "↓".to_owned(),
            "left" => "←".to_owned(),
            "right" => "→".to_owned(),
            "pagedown" => "PgDn".to_owned(),
            "pageup" => "PgUp".to_owned(),
            other if other.chars().count() == 1 => other.to_uppercase(),
            other => {
                let mut chars = other.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => other.to_owned(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

fn kbd_caps(keys: &str) -> AnyElement {
    if keys.is_empty() {
        return div().text_size(FS_SM).child("Unbound").into_any_element();
    }
    let mut row = div().flex().items_center().gap(px(4.));
    for (index, part) in keys.split('-').enumerate() {
        if index > 0 {
            row = row.child(div().text_size(FS_SM).child("+"));
        }
        row = row.child(Kbd::new(display(part)));
    }
    row.into_any_element()
}

/// Records a keystroke while the table is waiting for one.
pub(super) fn capture(event: &gpui::KeyDownEvent) -> Option<String> {
    let key = event.keystroke.key.to_lowercase();
    if matches!(
        key.as_str(),
        "control" | "shift" | "alt" | "meta" | "super" | "cmd" | "fn"
    ) {
        return None;
    }
    if key == "escape" {
        return Some("escape".to_owned());
    }
    let mut parts = Vec::new();
    if event.keystroke.modifiers.control {
        parts.push("ctrl");
    }
    if event.keystroke.modifiers.alt {
        parts.push("alt");
    }
    if event.keystroke.modifiers.shift {
        parts.push("shift");
    }
    if event.keystroke.modifiers.platform {
        parts.push("cmd");
    }
    let key = match key.as_str() {
        " " => "space".to_owned(),
        other => other.to_owned(),
    };
    parts.push(&key);
    Some(parts.join("-"))
}

impl SettingsView {
    pub(super) fn start_recording(&mut self, id: &'static str, cx: &mut Context<Self>) {
        self.kb_recording = Some(id);
        self.kb_conflict = None;
        self.kb_flash = None;
        cx.notify();
    }

    pub(super) fn on_record_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.kb_recording else {
            return;
        };
        let Some(action) = action(id) else {
            self.kb_recording = None;
            return;
        };
        let key = event.keystroke.key.as_str();
        if key == "escape" {
            self.kb_recording = None;
            self.kb_conflict = None;
            cx.notify();
            return;
        }
        if key == "backspace" {
            self.bind(id, "", cx);
            self.kb_recording = None;
            return;
        }
        let Some(keys) = capture(event) else {
            return;
        };
        let settings = self.settings(cx);
        if let Some(other) = ACTIONS
            .iter()
            .find(|candidate| candidate.id != id && current(&settings, candidate) == keys)
        {
            self.kb_conflict = Some(Conflict {
                id,
                keys,
                other: other.name,
                other_id: other.id,
            });
            cx.notify();
            return;
        }
        if keys == action.default {
            self.unbind_override(id, cx);
        } else {
            self.bind(id, &keys, cx);
        }
        self.kb_recording = None;
        self.kb_conflict = None;
    }

    fn bind(&mut self, id: &'static str, keys: &str, cx: &mut Context<Self>) {
        self.change(cx, |settings| {
            settings.keybindings.insert(id.to_owned(), keys.to_owned());
        });
    }

    fn unbind_override(&mut self, id: &'static str, cx: &mut Context<Self>) {
        self.change(cx, |settings| {
            settings.keybindings.remove(id);
        });
    }

    fn reassign(&mut self, conflict: Conflict, cx: &mut Context<Self>) {
        self.change(cx, |settings| {
            settings
                .keybindings
                .insert(conflict.other_id.to_owned(), String::new());
            settings
                .keybindings
                .insert(conflict.id.to_owned(), conflict.keys.clone());
        });
        self.kb_recording = None;
        self.kb_conflict = None;
    }

    fn reset_all(&mut self, cx: &mut Context<Self>) {
        self.change(cx, |settings| settings.keybindings.clear());
        self.kb_flash = Some("All shortcuts reset to defaults".into());
        self.kb_recording = None;
        self.kb_conflict = None;
    }

    fn export(&mut self, cx: &mut Context<Self>) {
        let settings = self.settings(cx);
        let Some(dir) = crate::settings::config_dir() else {
            self.kb_flash = Some("No configuration directory to export to".into());
            cx.notify();
            return;
        };
        if let Err(error) = std::fs::create_dir_all(&dir) {
            self.kb_flash = Some(format!("Could not create {}: {error}", dir.display()).into());
            cx.notify();
            return;
        }
        let path = dir.join("keybindings.toml");
        let mut out = String::from("# GitWarp keybindings\n");
        for action in ACTIONS {
            let keys = current(&settings, &action);
            if keys.is_empty() {
                out.push_str(&format!("# {} is unbound\n", action.id));
            } else if keys != action.default {
                out.push_str(&format!("\"{keys}\" = \"{}\"\n", action.id));
            }
        }
        let result =
            std::fs::File::create(&path).and_then(|mut file| file.write_all(out.as_bytes()));
        self.kb_flash = Some(
            match result {
                Ok(()) => format!("Saved to {}", path.display()),
                Err(error) => format!("Could not write {}: {error}", path.display()),
            }
            .into(),
        );
        cx.notify();
    }
}

pub(super) fn render(
    view: &mut SettingsView,
    _window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> AnyElement {
    let settings = view.settings(cx);
    let colors = cx.theme().colors;
    let query = view.kb_query.read(cx).text().trim().to_lowercase();
    let group = GROUPS[view.kb_group.min(GROUPS.len() - 1)];
    let rows: Vec<Action> = ACTIONS
        .iter()
        .copied()
        .filter(|action| group == "All" || action.group == group)
        .filter(|action| {
            query.is_empty()
                || action.name.to_lowercase().contains(&query)
                || current(&settings, action).to_lowercase().contains(&query)
        })
        .collect();
    let changed = settings.keybindings.len();

    let mut page = div().flex().flex_col();
    page = page.child(heading(
        "Keybindings",
        "Click the pencil, then press the shortcut you want. Esc cancels, Backspace unbinds.",
    ));

    let search = div()
        .flex_1()
        .min_w_0()
        .h(px(26.))
        .child(view.kb_query.clone());
    let group_picker = {
        let entity = cx.entity();
        form::select(
            "settings-kb-group".into(),
            GROUPS,
            view.kb_group,
            false,
            move |index, _, cx| {
                entity.update(cx, |view, cx| {
                    view.kb_group = index.min(GROUPS.len() - 1);
                    cx.notify();
                });
            },
        )
    };
    let reset_all = {
        let entity = cx.entity();
        Button::new("settings-kb-reset-all")
            .label("Reset all")
            .disabled(changed == 0)
            .on_click(move |_, _, cx| {
                entity.update(cx, |view, cx| view.reset_all(cx));
            })
    };
    let export = {
        let entity = cx.entity();
        Button::new("settings-kb-export")
            .icon(IconName::Copy)
            .label("Export")
            .on_click(move |_, _, cx| {
                entity.update(cx, |view, cx| view.export(cx));
            })
    };
    page = page.child(
        div()
            .w_full()
            .max_w(px(760.))
            .mb(px(12.))
            .flex()
            .items_center()
            .gap(px(8.))
            .child(search)
            .child(div().w(px(150.)).child(group_picker))
            .child(reset_all)
            .child(export),
    );
    if let Some(flash) = &view.kb_flash {
        page = page.child(
            div()
                .max_w(px(760.))
                .mb(px(12.))
                .child(ui::Banner::new(ui::BannerKind::Info, flash.clone())),
        );
    }
    page = page.child(
        div()
            .max_w(px(760.))
            .mb(px(6.))
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child(format!(
                "{} of {} actions{}",
                rows.len(),
                ACTIONS.len(),
                if changed > 0 {
                    format!(" · {changed} modified")
                } else {
                    String::new()
                }
            )),
    );

    let mut list = div()
        .w_full()
        .max_w(px(760.))
        .flex()
        .flex_col()
        .border_1()
        .rounded(R_SM)
        .overflow_hidden();
    if rows.is_empty() {
        list = list.child(
            div()
                .py(px(28.))
                .px(px(12.))
                .text_center()
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child(format!("No actions match “{query}”.")),
        );
    }
    for action in &rows {
        let keys = current(&settings, action);
        let modified = settings.keybindings.contains_key(action.id);
        let recording = view.kb_recording == Some(action.id);
        let conflict = view
            .kb_conflict
            .as_ref()
            .filter(|conflict| conflict.id == action.id)
            .cloned();
        let row = div()
            .id(format!("settings-kb-{}", action.id))
            .w_full()
            .min_h(px(36.))
            .px(px(12.))
            .flex()
            .items_center()
            .gap(px(8.))
            .border_b_1()
            .when(recording, |this| this.bg(colors.accent_soft))
            .when(conflict.is_some(), |this| this.bg(colors.diff_del_bg))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().text_size(FS_BASE).child(action.name))
                    .when(modified, |this| {
                        this.child(
                            div()
                                .text_size(px(11.))
                                .text_color(colors.muted)
                                .child("· modified"),
                        )
                    }),
            )
            .child(
                div()
                    .w(px(190.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .when(recording, |this| {
                        this.child(
                            div()
                                .text_size(FS_SM)
                                .text_color(colors.muted)
                                .child("Press the new shortcut…"),
                        )
                    })
                    .when(!recording, |this| this.child(kbd_caps(&keys))),
            )
            .child(
                div()
                    .w(px(72.))
                    .flex()
                    .justify_end()
                    .gap(px(2.))
                    .when(!recording, |this| {
                        let entity = cx.entity();
                        let id = action.id;
                        this.child(
                            Button::ghost(format!("settings-kb-edit-{}", action.id))
                                .icon_only(IconName::Edit)
                                .size(ButtonSize::Small)
                                .tooltip("Change shortcut")
                                .on_click(move |_, _, cx| {
                                    entity.update(cx, |view, cx| view.start_recording(id, cx));
                                }),
                        )
                        .when(modified, |this| {
                            let entity = cx.entity();
                            let id = action.id;
                            this.child(
                                Button::ghost(format!("settings-kb-reset-{}", action.id))
                                    .icon_only(IconName::Undo)
                                    .size(ButtonSize::Small)
                                    .tooltip("Reset to default")
                                    .on_click(move |_, _, cx| {
                                        entity.update(cx, |view, cx| {
                                            view.change(cx, |settings| {
                                                settings.keybindings.remove(id);
                                            });
                                        });
                                    }),
                            )
                        })
                    })
                    .when(recording, |this| {
                        let entity = cx.entity();
                        this.child(
                            Button::new(format!("settings-kb-cancel-{}", action.id))
                                .size(ButtonSize::Small)
                                .label("Cancel")
                                .on_click(move |_, _, cx| {
                                    entity.update(cx, |view, cx| {
                                        view.kb_recording = None;
                                        view.kb_conflict = None;
                                        cx.notify();
                                    });
                                }),
                        )
                    }),
            );
        let element = if let Some(conflict) = conflict {
            let entity = cx.entity();
            let message = div()
                .w_full()
                .px(px(12.))
                .pb(px(8.))
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(FS_SM)
                .text_color(colors.error)
                .child(Icon::new(IconName::Err).size(px(12.)).color(colors.error))
                .child(format!(
                    "{} is already used by “{}”.",
                    display(&conflict.keys),
                    conflict.other
                ))
                .child(
                    Button::new("settings-kb-conflict-reassign")
                        .size(ButtonSize::Small)
                        .label("Reassign here")
                        .on_click({
                            let conflict = conflict.clone();
                            move |_, _, cx| {
                                entity.update(cx, |view, cx| {
                                    view.reassign(conflict.clone(), cx);
                                });
                            }
                        }),
                )
                .child(
                    Button::new("settings-kb-conflict-cancel")
                        .size(ButtonSize::Small)
                        .label("Cancel")
                        .on_click({
                            let entity = cx.entity();
                            move |_, _, cx| {
                                entity.update(cx, |view, cx| {
                                    view.kb_recording = None;
                                    view.kb_conflict = None;
                                    cx.notify();
                                });
                            }
                        }),
                );
            div()
                .w_full()
                .flex()
                .flex_col()
                .child(row)
                .child(message)
                .into_any_element()
        } else {
            row.into_any_element()
        };
        list = list.child(element);
    }
    page = page.child(list);
    page.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_gpui_syntax_as_key_caps() {
        assert_eq!(display("ctrl-shift-b"), "Ctrl+Shift+B");
        assert_eq!(display("secondary-enter"), "Secondary+Enter");
        assert_eq!(display("space"), "Space");
        assert_eq!(display("ctrl-,"), "Ctrl+,");
        assert_eq!(display(""), "Unbound");
    }

    fn key(control: bool, shift: bool, key: &str) -> gpui::KeyDownEvent {
        gpui::KeyDownEvent {
            keystroke: gpui::Keystroke {
                modifiers: gpui::Modifiers {
                    control,
                    shift,
                    ..Default::default()
                },
                key: key.to_owned(),
                key_char: None,
            },
            is_held: false,
            prefer_character_input: false,
        }
    }

    #[test]
    fn captures_keystrokes_in_the_stored_syntax() {
        assert_eq!(
            capture(&key(true, true, "b")).as_deref(),
            Some("ctrl-shift-b")
        );
        assert_eq!(
            capture(&key(true, false, " ")).as_deref(),
            Some("ctrl-space")
        );
        assert_eq!(capture(&key(false, false, "j")).as_deref(), Some("j"));
        assert_eq!(
            capture(&key(false, false, "Escape")).as_deref(),
            Some("escape")
        );
        assert_eq!(capture(&key(false, false, "Shift")), None);
        assert_eq!(capture(&key(false, false, "Control")), None);
    }

    #[test]
    fn current_prefers_the_override_and_unbinding_stays_empty() {
        let mut settings = crate::settings::Settings::default();
        let action = ACTIONS[3];
        assert_eq!(current(&settings, &action), action.default);
        settings
            .keybindings
            .insert(action.id.to_owned(), "alt-b".to_owned());
        assert_eq!(current(&settings, &action), "alt-b");
        settings
            .keybindings
            .insert(action.id.to_owned(), String::new());
        assert_eq!(current(&settings, &action), "");
    }
}
