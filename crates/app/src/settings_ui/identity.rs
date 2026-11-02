//! Settings → Identity: the commit author (global or repository) and commit
//! signing. Hosting accounts moved to Settings → Accounts.

use std::time::Duration;

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, AnyElement, App, ClickEvent, Context, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, Window,
};

use git_core::{ConfigScope, Identity, Repo};
use ui::metrics::FS_SM;
use ui::theme::ActiveTheme;
use ui::{Banner, BannerKind, TextInputEvent};

use super::form;
use super::{group, heading, SettingsView};
use crate::settings::{IdentityScope, Settings};

fn scope_of(settings: &Settings) -> ConfigScope {
    settings.identity_scope.into()
}

impl SettingsView {
    pub(super) fn reload_identity(&mut self, cx: &mut Context<Self>) {
        if self.ssh_keys.is_empty() {
            self.reload_ssh(cx);
        }
        let settings = self.settings(cx);
        let scope = scope_of(&settings);
        let root = self.root.clone();
        if root.is_none() && scope == ConfigScope::Local {
            self.identity = None;
            self.identity_error = Some("Open a repository to edit its identity.".into());
            self.identity_busy = false;
            cx.notify();
            return;
        }
        let Ok(git) = self.git.clone() else {
            self.identity = None;
            self.identity_error = Some("Git is not available.".into());
            self.identity_busy = false;
            cx.notify();
            return;
        };
        self.identity_generation = self.identity_generation.wrapping_add(1);
        let generation = self.identity_generation;
        self.identity_busy = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match root {
                        Some(root) => Repo::open(git, &root).and_then(|repo| repo.identity(scope)),
                        None => git_core::global_identity(&git),
                    }
                })
                .await;
            this.update(cx, |view, cx| {
                if generation != view.identity_generation {
                    return;
                }
                view.identity_busy = false;
                match result {
                    Ok(identity) => {
                        view.identity_error = None;
                        view.apply_identity(&identity, cx);
                    }
                    Err(error) => {
                        view.identity_error = Some(error.to_string().into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn apply_identity(&mut self, identity: &Identity, cx: &mut Context<Self>) {
        self.identity = Some(identity.clone());
        self.identity_touched = false;
        // The Signing controls mirror git's configuration, not a separate
        // app-level preference.
        let sign = identity.sign_commits;
        let key = identity.signing_key.clone();
        crate::update_settings(cx, |settings| {
            settings.sign_commits = sign;
            settings.signing_key = key;
        });
        self.identity_syncing = true;
        self.name_input
            .update(cx, |input, cx| input.set_text(identity.name.clone(), cx));
        self.email_input
            .update(cx, |input, cx| input.set_text(identity.email.clone(), cx));
        self.identity_syncing = false;
    }

    pub(super) fn on_author_changed(&mut self, event: &TextInputEvent, cx: &mut Context<Self>) {
        if self.identity_syncing {
            return;
        }
        if let TextInputEvent::Changed(_) = event {
            self.identity_touched = true;
            self.schedule_identity_save(cx);
        }
        if let TextInputEvent::Submit = event {
            self.save_identity_now(cx);
        }
        cx.notify();
    }

    fn schedule_identity_save(&mut self, cx: &mut Context<Self>) {
        self.identity_save_generation = self.identity_save_generation.wrapping_add(1);
        let generation = self.identity_save_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(600))
                .await;
            this.update(cx, |view, cx| {
                if generation == view.identity_save_generation {
                    view.save_identity_now(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn save_identity_now(&mut self, cx: &mut Context<Self>) {
        let settings = self.settings(cx);
        let name = self.name_input.read(cx).text().trim().to_owned();
        let email = self.email_input.read(cx).text().trim().to_owned();
        if name.is_empty() {
            self.identity_touched = true;
            self.identity_error =
                Some("Name is required — git refuses to commit without it.".into());
            cx.notify();
            return;
        }
        if !valid_email(&email) {
            self.identity_touched = true;
            self.identity_error =
                Some("Enter a valid email address, like name@example.com.".into());
            cx.notify();
            return;
        }
        let root = self.root.clone();
        let Ok(git) = self.git.clone() else {
            self.identity_error = Some("Git is not available.".into());
            cx.notify();
            return;
        };
        let scope = scope_of(&settings);
        let sign = settings.sign_commits;
        let key = settings.signing_key.clone();
        self.identity_busy = true;
        self.identity_error = None;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match root {
                        Some(root) => {
                            let repo = Repo::open(git, &root)?;
                            repo.set_identity(scope, &name, &email)?;
                            repo.set_signing(scope, sign, &key)?;
                            repo.identity(scope)
                        }
                        None => {
                            git_core::set_global_identity(&git, &name, &email)?;
                            git_core::set_global_signing(&git, sign, &key)?;
                            git_core::global_identity(&git)
                        }
                    }
                })
                .await;
            this.update(cx, |view, cx| {
                view.identity_busy = false;
                match result {
                    Ok(identity) => {
                        view.identity_error = None;
                        view.apply_identity(&identity, cx);
                    }
                    Err(error) => {
                        view.identity_error = Some(error.to_string().into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn save_signing(&mut self, cx: &mut Context<Self>) {
        let settings = self.settings(cx);
        let root = self.root.clone();
        let Ok(git) = self.git.clone() else {
            return;
        };
        let scope = scope_of(&settings);
        let sign = settings.sign_commits;
        let key = settings.signing_key.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match root {
                        Some(root) => {
                            let repo = Repo::open(git, &root)?;
                            repo.set_signing(scope, sign, &key)
                        }
                        None => git_core::set_global_signing(&git, sign, &key),
                    }
                })
                .await;
            this.update(cx, |view, cx| {
                if let Err(error) = result {
                    view.identity_error = Some(error.to_string().into());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
}

fn valid_email(email: &str) -> bool {
    let Some((local, domain)) = email.rsplit_once('@') else {
        return false;
    };
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
}

pub(super) fn render(
    view: &mut SettingsView,
    _window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> AnyElement {
    let settings = view.settings(cx);
    let colors = cx.theme().colors;
    let mut page = div().flex().flex_col();

    page = page.child(heading(
        "Identity",
        "Who your commits are attributed to and how they are signed.",
    ));
    if let Some(error) = &view.identity_error {
        page = page.child(
            div().max_w(px(760.)).mb(px(12.)).child(
                Banner::new(BannerKind::Error, "Could not read or save the identity")
                    .body(error.clone()),
            ),
        );
    }

    let scope = {
        let entity = cx.entity();
        let control = form::segmented(
            "settings-id-scope".into(),
            ["All repositories", "This repository only"],
            usize::from(settings.identity_scope == IdentityScope::Repository),
            false,
            move |index, _, cx| {
                let scope = if index == 1 {
                    IdentityScope::Repository
                } else {
                    IdentityScope::Global
                };
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| settings.identity_scope = scope);
                    view.reload_identity(cx);
                });
            },
        );
        div().w(px(330.)).child(control).into_any_element()
    };
    let scope_help = if settings.identity_scope == IdentityScope::Global {
        "Written to ~/.gitconfig. A repository can still override it."
    } else {
        "Written to .git/config of the open repository."
    };

    let name = div().w_full().h(px(26.)).child(view.name_input.clone());
    let email = div().w_full().h(px(26.)).child(view.email_input.clone());
    let name_empty = view.name_input.read(cx).text().trim().is_empty();
    let name_error = (view.identity_touched && name_empty)
        .then_some("Name is required — git refuses to commit without it.");
    let email_text = view.email_input.read(cx).text().trim().to_owned();
    let email_error = (view.identity_touched && !valid_email(&email_text))
        .then_some("Enter a valid email address, like name@example.com.");

    let mut author_rows = vec![
        form::field_row("Apply to", None, scope).into_any_element(),
        div()
            .px(px(12.))
            .pb(px(6.))
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child(scope_help)
            .into_any_element(),
        div()
            .px(px(12.))
            .py(px(10.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .border_t_1()
            .child(div().text_size(FS_SM).child("Name"))
            .child(name)
            .when_some(name_error, |this, error| this.child(form::error(error, cx)))
            .into_any_element(),
        div()
            .px(px(12.))
            .py(px(10.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .border_t_1()
            .child(div().text_size(FS_SM).child("Email"))
            .child(email)
            .when_some(email_error, |this, error| {
                this.child(form::error(error, cx))
            })
            .when(view.identity_touched && email_error.is_none(), |this| {
                this.child(form::help("Visible in every commit you create.", cx))
            })
            .into_any_element(),
    ];
    if view.identity_busy {
        author_rows.push(
            div()
                .px(px(12.))
                .pb(px(8.))
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child("Saving…")
                .into_any_element(),
        );
    }
    page = page.child(group("Commit author", author_rows));

    let sign = {
        let entity = cx.entity();
        form::switch(
            "settings-id-sign".into(),
            settings.sign_commits,
            false,
            move |checked, _, cx| {
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| settings.sign_commits = checked);
                    view.save_signing(cx);
                });
            },
        )
    };
    let key_names: Vec<String> = {
        let mut names: Vec<String> = view.ssh_keys.iter().map(|key| key.name.clone()).collect();
        if !settings.signing_key.is_empty() && !names.contains(&settings.signing_key) {
            names.insert(0, settings.signing_key.clone());
        }
        names
    };
    let key_index = key_names
        .iter()
        .position(|name| *name == settings.signing_key)
        .map(|index| index + 1)
        .unwrap_or(0);
    let mut key_options = vec!["No signing key".to_owned()];
    key_options.extend(key_names.clone());
    let signing_key = {
        let entity = cx.entity();
        let names = key_names.clone();
        form::select(
            "settings-id-key".into(),
            key_options,
            key_index,
            !settings.sign_commits,
            move |index, _, cx| {
                let key = if index == 0 {
                    String::new()
                } else {
                    names.get(index - 1).cloned().unwrap_or_default()
                };
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| settings.signing_key = key);
                    view.save_signing(cx);
                });
            },
        )
    };
    page = page.child(group(
        "Signing",
        vec![
            form::row(
                "settings-id-sign-row",
                "Sign commits",
                "Adds a verified badge on hosts that know your key.",
                sign,
                false,
            ),
            form::row(
                "settings-id-key-row",
                "Signing key",
                "Used for commits and tags.",
                signing_key,
                !settings.sign_commits,
            ),
        ],
    ));

    page = page.child(
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .mb(px(20.))
            .max_w(px(760.))
            .child(
                div()
                    .text_size(ui::metrics::FS_XS)
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("HOSTING ACCOUNTS"),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(4.))
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child("Accounts for GitHub, GitLab and other services now live in")
                    .child(
                        div()
                            .id("settings-identity-accounts-link")
                            .text_color(colors.accent)
                            .cursor_pointer()
                            .hover(|style| style.underline())
                            .on_click({
                                let entity = cx.entity();
                                move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                                    entity.update(cx, |view, cx| {
                                        view.set_page(super::Page::Accounts, window, cx)
                                    });
                                }
                            })
                            .child("Settings → Accounts"),
                    )
                    .child("."),
            ),
    );
    page.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_addresses_only() {
        assert!(valid_email("mira@kovalenko.dev"));
        assert!(valid_email("mira.k@users.noreply.github.com"));
        assert!(!valid_email(""));
        assert!(!valid_email("mira"));
        assert!(!valid_email("mira@localhost"));
        assert!(!valid_email("mira@.dev"));
        assert!(!valid_email("@kovalenko.dev"));
    }
}
