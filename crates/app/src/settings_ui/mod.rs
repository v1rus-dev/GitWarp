//! The Settings surface: eight pages in the design's order, with a persistent
//! left navigation and an automatically saved settings file.
//!
//! The page is a centred window drawn over the workspace, not a replacement for
//! it: closing it leaves the welcome screen or the repository underneath, and
//! the Identity page can edit the open repository's config.
//!
//! Every control writes through [`crate::update_settings`], which also saves
//! on the background executor. Appearance changes additionally rebuild the
//! theme global, so the whole window repaints at once.

mod appearance;
mod diff;
mod form;
mod general;
mod git;
mod identity;
pub(crate) mod keybindings;
mod ssh;
mod worktrees;

use std::path::PathBuf;
use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, AnyElement, App, AppContext, ClickEvent, Context, Div, Entity, FocusHandle,
    InteractiveElement, IntoElement, ParentElement, Render, RenderOnce, SharedString,
    StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window,
};

use git_core::{Error, Git};
use ui::metrics::{FONT_UI, FS_BASE, FS_SM, FS_XL, R_MD, R_SM, STATUS};
use ui::theme::{ActiveTheme, Density as UiDensity, ThemeStyle};
use ui::{Button, Icon, IconName};

use crate::settings::{Density, Settings};
use crate::workspace::{git_label, git_tooltip, titlebar, titlebar_end, titlebar_fill, Workspace};

/// One page of the Settings surface, in navigation order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    General,
    Appearance,
    Git,
    Worktrees,
    Ssh,
    Keybindings,
    Diff,
    Identity,
}

impl Page {
    pub const ALL: [Page; 8] = [
        Page::General,
        Page::Appearance,
        Page::Git,
        Page::Worktrees,
        Page::Ssh,
        Page::Keybindings,
        Page::Diff,
        Page::Identity,
    ];

    fn title(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::Appearance => "Appearance",
            Page::Git => "Git",
            Page::Worktrees => "Worktrees",
            Page::Ssh => "SSH keys",
            Page::Keybindings => "Keybindings",
            Page::Diff => "Diff & merge",
            Page::Identity => "Identity",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Page::General => IconName::Gear,
            Page::Appearance => IconName::Eye,
            Page::Git => IconName::Git,
            Page::Worktrees => IconName::Worktree,
            Page::Ssh => IconName::Key,
            Page::Keybindings => IconName::Keyboard,
            Page::Diff => IconName::Split,
            Page::Identity => IconName::User,
        }
    }
}

/// A SSH key card's connection-test state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TestState {
    Running(String),
    Ok(String, String),
    Failed(String, String),
}

/// The Settings page.
pub struct SettingsView {
    /// The repository the settings were opened from, for the Identity page's
    /// repository scope and the status bar.
    pub(crate) root: Option<PathBuf>,
    pub(crate) git: Result<Arc<Git>, Arc<Error>>,
    pub(crate) workspace: WeakEntity<Workspace>,
    pub(crate) page: Page,
    pub(crate) focus: FocusHandle,
    pub(crate) home: Option<PathBuf>,
    /// Shown when the language select changed this session.
    pub(crate) language_changed: bool,

    /// Git: detection results and the custom-path field.
    pub(crate) git_state: git::GitDetection,
    pub(crate) git_generation: u64,
    pub(crate) custom_git: Entity<ui::TextInput>,
    pub(crate) _custom_git_events: Subscription,

    /// SSH keys: keys, agent and trusted hosts, loaded off the UI thread.
    pub(crate) ssh_keys: Vec<git_core::ssh::SshKey>,
    pub(crate) ssh_agent: Option<bool>,
    pub(crate) ssh_hosts: Vec<git_core::ssh::TrustedHost>,
    pub(crate) ssh_error: Option<String>,
    pub(crate) ssh_loading: bool,
    pub(crate) ssh_generation: u64,
    pub(crate) ssh_tests: std::collections::HashMap<String, TestState>,
    pub(crate) ssh_dialog: Option<Entity<ssh::SshDialog>>,

    /// Keybindings: search, group filter, recording and a conflict.
    pub(crate) kb_query: Entity<ui::TextInput>,
    pub(crate) _kb_query_events: Subscription,
    pub(crate) kb_group: usize,
    pub(crate) kb_recording: Option<&'static str>,
    pub(crate) kb_conflict: Option<keybindings::Conflict>,
    pub(crate) kb_flash: Option<SharedString>,

    /// Identity: config values and the two author fields.
    pub(crate) identity: Option<git_core::Identity>,
    pub(crate) identity_error: Option<SharedString>,
    pub(crate) identity_busy: bool,
    pub(crate) identity_generation: u64,
    pub(crate) identity_touched: bool,
    /// Set while the fields are filled from git config, so that does not
    /// schedule a save.
    pub(crate) identity_syncing: bool,
    pub(crate) name_input: Entity<ui::TextInput>,
    pub(crate) email_input: Entity<ui::TextInput>,
    pub(crate) _name_events: Subscription,
    pub(crate) _email_events: Subscription,
    /// Bumped by every edit; a stale debounced save is dropped.
    pub(crate) identity_save_generation: u64,
    /// Accounts found in the user's git credential helper, per host.
    pub(crate) accounts: identity::Accounts,
    pub(crate) account_dialog: Option<Entity<identity::AccountDialog>>,

    /// Diff & merge: the editable merge command.
    pub(crate) merge_command: Entity<ui::TextInput>,
    pub(crate) _merge_command_events: Subscription,
    /// Merge tool test feedback: the started tool or the I/O error.
    pub(crate) merge_test: Option<Result<SharedString, SharedString>>,
}

impl SettingsView {
    pub(crate) fn new(
        root: Option<PathBuf>,
        git: Result<Arc<Git>, Arc<Error>>,
        workspace: WeakEntity<Workspace>,
        page: Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = current_settings(cx);
        let custom_git = cx.new(|cx| {
            let mut input = ui::TextInput::new(window, cx);
            input.set_text(settings.git_path.clone(), cx);
            input.set_placeholder("/path/to/git", cx);
            input.set_font_family(ui::metrics::FONT_MONO, cx);
            input
        });
        let kb_query = cx.new(|cx| {
            let mut input = ui::TextInput::new(window, cx);
            input.set_placeholder("Search actions or shortcuts", cx);
            input
        });
        let merge_command = cx.new(|cx| {
            let mut input = ui::TextInput::new(window, cx);
            input.set_text(settings.merge_command().to_owned(), cx);
            input.set_placeholder("mytool $LOCAL $BASE $REMOTE $MERGED", cx);
            input.set_font_family(ui::metrics::FONT_MONO, cx);
            input
        });
        let name_input = cx.new(|cx| {
            let mut input = ui::TextInput::new(window, cx);
            input.set_placeholder("Your name", cx);
            input
        });
        let email_input = cx.new(|cx| {
            let mut input = ui::TextInput::new(window, cx);
            input.set_placeholder("you@example.com", cx);
            input
        });

        let custom_git_events =
            cx.subscribe(&custom_git, |_, _, event: &ui::TextInputEvent, cx| {
                let ui::TextInputEvent::Changed(text) = event else {
                    return;
                };
                crate::update_settings(cx, |settings| settings.git_path = text.trim().to_owned());
                cx.notify();
            });
        let kb_query_events =
            cx.subscribe(&kb_query, |_, _, _: &ui::TextInputEvent, cx| cx.notify());
        let merge_command_events =
            cx.subscribe(&merge_command, |this, _, event: &ui::TextInputEvent, cx| {
                let ui::TextInputEvent::Changed(text) = event else {
                    return;
                };
                crate::update_settings(cx, |settings| {
                    settings.merge_command = text.trim().to_owned();
                });
                this.merge_test = None;
                cx.notify();
            });
        let name_events = cx.subscribe(&name_input, |this, _, event: &ui::TextInputEvent, cx| {
            this.on_author_changed(event, cx)
        });
        let email_events = cx.subscribe(&email_input, |this, _, event: &ui::TextInputEvent, cx| {
            this.on_author_changed(event, cx)
        });

        let mut view = Self {
            root,
            git,
            workspace,
            page,
            focus: cx.focus_handle().tab_stop(false),
            home: crate::format::home_dir(),
            language_changed: false,
            git_state: git::GitDetection::default(),
            git_generation: 0,
            custom_git,
            _custom_git_events: custom_git_events,
            ssh_keys: Vec::new(),
            ssh_agent: None,
            ssh_hosts: Vec::new(),
            ssh_error: None,
            ssh_loading: false,
            ssh_generation: 0,
            ssh_tests: std::collections::HashMap::new(),
            ssh_dialog: None,
            kb_query,
            _kb_query_events: kb_query_events,
            kb_group: 0,
            kb_recording: None,
            kb_conflict: None,
            kb_flash: None,
            identity: None,
            identity_error: None,
            identity_busy: false,
            identity_generation: 0,
            identity_touched: false,
            identity_syncing: false,
            name_input,
            email_input,
            _name_events: name_events,
            _email_events: email_events,
            identity_save_generation: 0,
            accounts: identity::Accounts::default(),
            account_dialog: None,
            merge_command,
            _merge_command_events: merge_command_events,
            merge_test: None,
        };
        view.start_git_detection(window, cx);
        match page {
            Page::Ssh => view.reload_ssh(cx),
            Page::Identity => view.reload_identity(cx),
            _ => {}
        }
        view
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// The settings as they are stored right now.
    pub(crate) fn settings(&self, cx: &App) -> Settings {
        current_settings(cx)
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(workspace) = self.workspace.upgrade() {
            workspace.update(cx, |workspace, cx| workspace.close_settings(window, cx));
        }
    }

    pub(crate) fn set_page(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        if self.page == page {
            return;
        }
        self.page = page;
        match page {
            Page::Git => self.start_git_detection(window, cx),
            Page::Ssh => self.reload_ssh(cx),
            Page::Identity => self.reload_identity(cx),
            _ => {}
        }
        cx.notify();
    }

    /// Applies the appearance settings to the theme global.
    pub(crate) fn apply_appearance(&mut self, cx: &mut Context<Self>) {
        let settings = self.settings(cx);
        ui::theme::set_mode(settings.theme.into(), cx);
        let density = match settings.density {
            Density::Compact => UiDensity::Compact,
            Density::Normal => UiDensity::Normal,
            Density::Roomy => UiDensity::Roomy,
        };
        ui::theme::set_style(
            ThemeStyle {
                accent: settings.accent.into(),
                density,
                ui_font_size: f32::from(settings.ui_font_size),
                mono_font_size: f32::from(settings.mono_font_size),
            },
            cx,
        );
        cx.notify();
    }

    fn render_nav(&self, cx: &Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let mut nav = div()
            .w(px(200.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(px(8.))
            .bg(colors.bg)
            .border_r_1()
            .border_color(colors.border);
        for page in Page::ALL {
            let selected = self.page == page;
            let view = cx.entity();
            let mut item = div()
                .id(SharedString::from(format!("settings-nav-{:?}", page)))
                .h(px(28.))
                .px(px(10.))
                .flex()
                .items_center()
                .gap(px(8.))
                .rounded(R_SM)
                .text_size(FS_BASE)
                .text_color(if selected { colors.text } else { colors.muted })
                .when(selected, |this| this.bg(colors.selection))
                .cursor_pointer()
                .hover(move |style| style.bg(colors.hover))
                .on_click(move |_: &ClickEvent, window, cx| {
                    view.update(cx, |view, cx| view.set_page(page, window, cx));
                })
                .child(Icon::new(page.icon()).size(px(14.)).color(if selected {
                    colors.accent
                } else {
                    colors.muted
                }))
                .child(page.title());
            item = item.debug_selector(move || format!("settings-nav-{page:?}"));
            nav = nav.child(item);
        }
        nav
    }

    fn render_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.page {
            Page::General => general::render(self, window, cx),
            Page::Appearance => appearance::render(self, window, cx),
            Page::Git => git::render(self, window, cx),
            Page::Worktrees => worktrees::render(self, window, cx),
            Page::Ssh => ssh::render(self, window, cx),
            Page::Keybindings => keybindings::render(self, window, cx),
            Page::Diff => diff::render(self, window, cx),
            Page::Identity => identity::render(self, window, cx),
        }
    }

    fn render_status(&self, cx: &Context<Self>) -> Div {
        let colors = cx.theme().colors;
        let workspace = self.workspace.clone();
        div()
            .h(STATUS)
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(12.))
            .bg(colors.bg)
            .border_t_1()
            .border_color(colors.border)
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child("Changes are saved automatically")
            .child(div().flex_1())
            .child(
                div()
                    .id("settings-status-git")
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .cursor_pointer()
                    .hover(|this| this.bg(colors.hover))
                    .tooltip(ui::Tooltip::text(git_tooltip(&self.git)))
                    .on_click(move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                        let _ = workspace.update(cx, |workspace, cx| {
                            workspace.open_settings_at(Page::Git, window, cx)
                        });
                    })
                    .child(Icon::new(IconName::Git).size(px(12.)).color(colors.muted))
                    .child(git_label(&self.git)),
            )
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let client = matches!(
            window.window_decorations(),
            gpui::Decorations::Client { .. }
        );
        let colors = cx.theme().colors;
        let close = {
            let view = cx.entity();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                view.update(cx, |view, cx| view.close(window, cx));
            }
        };
        let mut bar = titlebar(cx)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pl(px(4.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(Icon::new(IconName::Gear).size(px(14.)).color(colors.muted))
                    .child("Settings"),
            )
            .child(titlebar_fill())
            .child(
                Button::ghost("settings-close")
                    .icon_only(IconName::X)
                    .tooltip("Close settings (Esc)")
                    .on_click(close),
            );
        bar = titlebar_end(bar, client);

        let nav = self.render_nav(cx);
        let page = self.render_page(window, cx);
        let page = div()
            .debug_selector({
                let page = self.page;
                move || format!("settings-page-{page:?}")
            })
            .child(page);
        let pane = div()
            .id("settings-pane")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(28.))
            .pt(px(20.))
            .pb(px(28.))
            .flex()
            .flex_col()
            .items_center()
            .child(div().w_full().max_w(px(760.)).child(page));
        let body = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .child(nav)
            .child(pane);
        let status = self.render_status(cx);

        div()
            .id("settings-view")
            .debug_selector(|| "settings-view".to_owned())
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.bg)
            .text_color(colors.text)
            .font_family(FONT_UI)
            .text_size(FS_BASE)
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(|view, event: &gpui::KeyDownEvent, window, cx| {
                if view.kb_recording.is_some() {
                    view.on_record_key(event, window, cx);
                    cx.stop_propagation();
                } else if event.keystroke.key == "escape"
                    && !event.keystroke.modifiers.control
                    && !event.keystroke.modifiers.alt
                    && !event.keystroke.modifiers.shift
                    && !event.keystroke.modifiers.platform
                {
                    view.close(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(bar)
            .child(body)
            .child(status)
    }
}

/// The in-memory settings, or defaults before the app global exists (tests).
pub(crate) fn current_settings(cx: &App) -> Settings {
    cx.try_global::<crate::AppSettings>()
        .map(|state| state.settings.clone())
        .unwrap_or_default()
}

impl SettingsView {
    /// Saves one settings change, applies appearance and repaints.
    pub(crate) fn change(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Settings)) {
        crate::update_settings(cx, f);
        cx.notify();
    }

    /// Saves and applies the appearance options at once.
    pub(crate) fn change_appearance(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Settings),
    ) {
        crate::update_settings(cx, f);
        self.apply_appearance(cx);
    }
}

/// Shared heading for every page.
pub(crate) fn heading(title: &'static str, lead: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .mb(px(16.))
        .child(div().text_size(FS_XL).child(title))
        .child(div().max_w(px(760.)).text_size(FS_BASE).child(lead.into()))
}

/// One section: an uppercase label and the bordered list around it.
pub(crate) fn group(title: &'static str, rows: Vec<AnyElement>) -> Group {
    Group { title, rows }
}

/// A titled section card (design `.slist`): surface fill, 1px border, and a
/// divider under every row but the last (`.srow:last-child { border: 0 }`).
/// It is a component because the colours come from the theme at render time.
#[derive(IntoElement)]
pub(crate) struct Group {
    title: &'static str,
    rows: Vec<AnyElement>,
}

impl RenderOnce for Group {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let last = self.rows.len().saturating_sub(1);
        let mut list = div()
            .flex()
            .flex_col()
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .rounded(R_MD)
            .overflow_hidden();
        for (index, row) in self.rows.into_iter().enumerate() {
            list = list.child(
                div()
                    .when(index != last, |this| {
                        this.border_b_1().border_color(colors.border)
                    })
                    .child(row),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .mb(px(20.))
            .w_full()
            .max_w(px(760.))
            .child(
                div()
                    .text_size(ui::metrics::FS_XS)
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(self.title.to_uppercase()),
            )
            .child(list)
    }
}
