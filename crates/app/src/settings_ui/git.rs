//! Settings → Git: which executable to run, what was detected for each source
//! and the warnings when the chosen one cannot be used.

use std::path::PathBuf;

use gpui::prelude::FluentBuilder;
use gpui::{div, px, AnyElement, App, Context, IntoElement, ParentElement, Styled, Window};
use gpui::{InteractiveElement, StatefulInteractiveElement};

use git_core::{Error, ErrorClass, Git, GitSource, GitVersion};
use ui::metrics::{FONT_MONO, FS_BASE, FS_SM};
use ui::theme::ActiveTheme;
use ui::{Banner, BannerKind, Button, ButtonSize, Icon, IconName, Spinner};

use super::{heading, SettingsView};
use crate::settings::GitMode;

/// One executable that was probed.
#[derive(Clone, Debug)]
pub struct Probe {
    pub path: PathBuf,
    /// Present when the executable ran and reported its version.
    pub version: Option<GitVersion>,
    pub source: GitSource,
    pub error: Option<String>,
}

impl Probe {
    fn ok(&self) -> bool {
        self.error.is_none() && self.version.is_some()
    }

    fn too_old(&self) -> bool {
        self.version.is_some() && self.error.is_some()
    }

    fn label(&self) -> String {
        match (self.version, &self.error) {
            (Some(version), None) => version.to_string(),
            (Some(version), Some(_)) => format!("{version}"),
            _ => "—".to_owned(),
        }
    }

    fn display_path(&self) -> String {
        if self.path.as_os_str().is_empty() {
            "not shipped with this build".to_owned()
        } else {
            self.path.display().to_string()
        }
    }
}

/// Detection results for the three sources.
#[derive(Default)]
pub struct GitDetection {
    pub loading: bool,
    pub bundled: Option<Probe>,
    pub system: Option<Probe>,
    pub custom: Option<Probe>,
}

fn probe(path: PathBuf, source: GitSource) -> Probe {
    let display = path.clone();
    match Git::probe_at(path, source) {
        Ok(git) => Probe {
            path: git.path().to_path_buf(),
            version: Some(git.version()),
            source: git.source(),
            error: None,
        },
        Err(error) => {
            let (version, message) = describe(&error);
            Probe {
                path: display,
                version,
                source,
                error: Some(message),
            }
        }
    }
}

fn describe(error: &Error) -> (Option<GitVersion>, String) {
    match error.class() {
        ErrorClass::GitTooOld {
            found, required, ..
        } => (
            Some(found),
            format!("git {found} is too old; GitWarp needs {required} or newer"),
        ),
        ErrorClass::GitNotFound => (None, "binary is missing (file not found)".to_owned()),
        _ => (None, error.to_string()),
    }
}

fn bundled_missing() -> Probe {
    Probe {
        path: PathBuf::new(),
        version: None,
        source: GitSource::Bundled,
        error: Some("no bundled git is shipped with this build".to_owned()),
    }
}

fn system_missing() -> Probe {
    Probe {
        path: PathBuf::new(),
        version: None,
        source: GitSource::System,
        error: Some("“git” was not found in PATH".to_owned()),
    }
}

impl SettingsView {
    pub(super) fn start_git_detection(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.git_generation = self.git_generation.wrapping_add(1);
        let generation = self.git_generation;
        let custom_path = self.settings(cx).git_path;
        self.git_state.loading = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let bundled = match Git::bundled_path() {
                        Some(path) => probe(path, GitSource::Bundled),
                        None => bundled_missing(),
                    };
                    let system = match Git::system_path() {
                        Some(path) => probe(path, GitSource::System),
                        None => system_missing(),
                    };
                    let custom = (!custom_path.trim().is_empty())
                        .then(|| probe(PathBuf::from(custom_path.trim()), GitSource::Custom));
                    (bundled, system, custom)
                })
                .await;
            this.update(cx, |view, cx| {
                if generation != view.git_generation {
                    return;
                }
                view.git_state.loading = false;
                view.git_state.bundled = Some(result.0);
                view.git_state.system = Some(result.1);
                view.git_state.custom = result.2;
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn validate_custom(&mut self, cx: &mut Context<Self>) {
        let path = self.settings(cx).git_path;
        let path = PathBuf::from(path.trim());
        if path.as_os_str().is_empty() {
            self.git_state.custom = Some(Probe {
                path,
                version: None,
                source: GitSource::Custom,
                error: Some("Enter or browse to a git executable.".to_owned()),
            });
            cx.notify();
            return;
        }
        self.git_state.loading = true;
        cx.spawn(async move |this, cx| {
            let probe = cx
                .background_executor()
                .spawn(async move { probe(path, GitSource::Custom) })
                .await;
            this.update(cx, |view, cx| {
                view.git_state.loading = false;
                view.git_state.custom = Some(probe);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

fn version_label(probe: Option<&Probe>, colors: ui::ThemeColors) -> AnyElement {
    match probe {
        Some(probe) if probe.ok() => div()
            .font_family(FONT_MONO)
            .text_size(FS_SM)
            .child(probe.label())
            .into_any_element(),
        Some(probe) if probe.too_old() => div()
            .flex()
            .items_center()
            .gap(px(4.))
            .font_family(FONT_MONO)
            .text_size(FS_SM)
            .text_color(colors.error)
            .child(Icon::new(IconName::Warn).size(px(12.)).color(colors.error))
            .child(probe.label())
            .into_any_element(),
        Some(_) => div()
            .flex()
            .items_center()
            .gap(px(4.))
            .text_size(FS_SM)
            .text_color(colors.error)
            .child(Icon::new(IconName::Err).size(px(12.)).color(colors.error))
            .child("unusable")
            .into_any_element(),
        None => div()
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child("—")
            .into_any_element(),
    }
}

fn option_card(
    mode: GitMode,
    selected: bool,
    title: &'static str,
    description: AnyElement,
    version: AnyElement,
    extra: Option<AnyElement>,
    cx: &mut Context<SettingsView>,
) -> AnyElement {
    let colors = cx.theme().colors;
    let entity = cx.entity();
    div()
        .id(format!("settings-git-{title}"))
        .w_full()
        .p(px(10.))
        .px(px(12.))
        .rounded(super::R_SM)
        .border_1()
        .border_color(if selected {
            colors.accent
        } else {
            colors.border
        })
        .when(selected, |this| this.bg(colors.accent_soft))
        .cursor_pointer()
        .hover(move |style| style.border_color(colors.border_strong))
        .on_click(move |_, _, cx| {
            entity.update(cx, |view, cx| {
                view.change(cx, |settings| settings.git_mode = mode);
            });
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(
                    div()
                        .w(px(14.))
                        .h(px(14.))
                        .rounded(px(7.))
                        .border_1()
                        .border_color(if selected {
                            colors.accent
                        } else {
                            colors.border_strong
                        })
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(selected, |this| {
                            this.child(div().w(px(6.)).h(px(6.)).rounded(px(3.)).bg(colors.accent))
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(FS_BASE)
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(title),
                )
                .child(version),
        )
        .child(
            div()
                .pl(px(24.))
                .pt(px(4.))
                .text_size(FS_SM)
                .child(description),
        )
        .when_some(extra, |this, extra| this.child(extra))
        .into_any_element()
}

pub(super) fn render(
    view: &mut SettingsView,
    _window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> AnyElement {
    let settings = view.settings(cx);
    let colors = cx.theme().colors;
    let state = &view.git_state;
    let bundled = state.bundled.clone();
    let system = state.system.clone();
    let custom = state.custom.clone();
    let loading = state.loading;

    let active: Option<&Probe> = match settings.git_mode {
        GitMode::Auto => match (&bundled, &system) {
            (Some(b), _) if b.ok() => Some(b),
            (_, Some(s)) if s.ok() => Some(s),
            _ => None,
        },
        GitMode::Bundled => bundled.as_ref().filter(|probe| probe.ok()),
        GitMode::System => system.as_ref().filter(|probe| probe.ok()),
        GitMode::Custom => custom.as_ref().filter(|probe| probe.ok()),
    };

    let mut banners: Vec<AnyElement> = Vec::new();
    match settings.git_mode {
        GitMode::Auto => {
            let bundled_ok = bundled.as_ref().is_some_and(Probe::ok);
            let system_ok = system.as_ref().is_some_and(Probe::ok);
            let bundle_shipped = bundled
                .as_ref()
                .is_some_and(|probe| !probe.path.as_os_str().is_empty());
            if !bundled_ok && system_ok && bundle_shipped {
                let (path, version) = system
                    .as_ref()
                    .map(|probe| (probe.display_path(), probe.label()))
                    .unwrap_or_default();
                banners.push(
                    Banner::new(BannerKind::Warn, "Bundled git is broken — using system git")
                        .body(format!(
                            "Auto fell back to {path} ({version}). Bundled git is unusable: {}",
                            bundled
                                .as_ref()
                                .and_then(|probe| probe.error.clone())
                                .unwrap_or_else(|| "not available".to_owned())
                        ))
                        .into_any_element(),
                );
            } else if !bundled_ok && !system_ok {
                banners.push(
                    Banner::new(BannerKind::Error, "No usable git found")
                        .body("Both bundled and system git are unusable.")
                        .list([
                            format!(
                                "Bundled ({}): {}",
                                bundled
                                    .as_ref()
                                    .map(Probe::display_path)
                                    .unwrap_or_else(|| "—".to_owned()),
                                bundled
                                    .as_ref()
                                    .and_then(|probe| probe.error.clone())
                                    .unwrap_or_default()
                            ),
                            format!(
                                "System (PATH): {}",
                                system
                                    .as_ref()
                                    .and_then(|probe| probe.error.clone())
                                    .unwrap_or_default()
                            ),
                        ])
                        .into_any_element(),
                );
            }
        }
        GitMode::Bundled => {
            if let Some(probe) = &bundled {
                if !probe.ok() {
                    banners.push(
                        Banner::new(BannerKind::Error, "Bundled git is unusable")
                            .body(format!(
                                "{}: {}",
                                probe.display_path(),
                                probe.error.clone().unwrap_or_default()
                            ))
                            .into_any_element(),
                    );
                }
            }
        }
        GitMode::System => {
            if let Some(probe) = &system {
                if !probe.ok() {
                    banners.push(
                        Banner::new(BannerKind::Error, "System git is unusable")
                            .body(probe.error.clone().unwrap_or_default())
                            .into_any_element(),
                    );
                }
            }
        }
        GitMode::Custom => {
            if settings.git_path.trim().is_empty() {
                banners.push(
                    Banner::new(BannerKind::Error, "No path chosen")
                        .body("Enter or browse to a git executable.")
                        .into_any_element(),
                );
            } else if let Some(probe) = &custom {
                if !probe.ok() {
                    banners.push(
                        Banner::new(BannerKind::Error, "This file cannot be used as git")
                            .body(format!(
                                "{}: {}",
                                probe.display_path(),
                                probe.error.clone().unwrap_or_default()
                            ))
                            .into_any_element(),
                    );
                }
            }
        }
    }

    let auto_version = version_label(
        bundled
            .as_ref()
            .filter(|probe| probe.ok())
            .or_else(|| system.as_ref().filter(|probe| probe.ok())),
        colors,
    );
    let bundled_version = version_label(bundled.as_ref(), colors);
    let system_version = version_label(system.as_ref(), colors);
    let custom_version = version_label(custom.as_ref(), colors);

    let custom_extra = (settings.git_mode == GitMode::Custom).then(|| {
        let entity = cx.entity();
        let validate = {
            let entity = entity.clone();
            move |_: &gpui::ClickEvent, _window: &mut Window, cx: &mut App| {
                entity.update(cx, |view, cx| view.validate_custom(cx));
            }
        };
        let reference = &view.custom_git;
        div()
            .w_full()
            .pl(px(24.))
            .pt(px(8.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().flex_1().min_w_0().h(px(26.)).child(reference.clone()))
                    .child(
                        Button::new("settings-git-validate")
                            .size(ButtonSize::Small)
                            .label("Validate")
                            .on_click(validate),
                    ),
            )
            .child(custom_status(custom.as_ref(), cx))
            .into_any_element()
    });

    let options = div()
        .w_full()
        .max_w(px(760.))
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(option_card(
            GitMode::Auto,
            settings.git_mode == GitMode::Auto,
            "Auto",
            div()
                .text_color(colors.muted)
                .child(
                    "Use the bundled git if present and working, otherwise the one from PATH. Recommended.",
                )
                .into_any_element(),
            auto_version,
            None,
            cx,
        ))
        .child(option_card(
            GitMode::Bundled,
            settings.git_mode == GitMode::Bundled,
            "Bundled",
            div()
                .text_color(colors.muted)
                .child("Shipped with GitWarp.")
                .when_some(bundled.as_ref(), |this, probe| {
                    this.child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(FS_SM)
                            .child(probe.display_path()),
                    )
                })
                .into_any_element(),
            bundled_version,
            None,
            cx,
        ))
        .child(option_card(
            GitMode::System,
            settings.git_mode == GitMode::System,
            "System",
            div()
                .text_color(colors.muted)
                .child("From PATH.")
                .when_some(system.as_ref(), |this, probe| {
                    this.child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(FS_SM)
                            .child(probe.display_path()),
                    )
                })
                .into_any_element(),
            system_version,
            None,
            cx,
        ))
        .child(option_card(
            GitMode::Custom,
            settings.git_mode == GitMode::Custom,
            "Custom path",
            div()
                .text_color(colors.muted)
                .child("Use a git you installed yourself.")
                .into_any_element(),
            custom_version,
            custom_extra,
            cx,
        ));

    let in_use = match active {
        Some(probe) => div()
            .w_full()
            .max_w(px(760.))
            .p(px(12.))
            .rounded(super::R_SM)
            .border_1()
            .border_color(colors.border)
            .flex()
            .flex_col()
            .gap(px(4.))
            .text_size(FS_SM)
            .child(in_use_line("Executable", probe.display_path(), colors))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().w(px(90.)).text_color(colors.muted).child("Version"))
                    .child(div().font_family(FONT_MONO).child(probe.label()))
                    .when(probe.too_old(), |this| {
                        this.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .text_color(colors.error)
                                .child(Icon::new(IconName::Err).size(px(12.)).color(colors.error))
                                .child("below 2.31"),
                        )
                    })
                    .when(probe.ok(), |this| {
                        this.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .text_color(colors.success)
                                .child(
                                    Icon::new(IconName::Check)
                                        .size(px(12.))
                                        .color(colors.success),
                                )
                                .child("supported"),
                        )
                    }),
            )
            .child(in_use_line(
                "Source",
                format!(
                    "{}{}",
                    probe.source,
                    if settings.git_mode == GitMode::Auto {
                        " (auto)"
                    } else {
                        ""
                    }
                ),
                colors,
            ))
            .into_any_element(),
        None => div()
            .w_full()
            .max_w(px(760.))
            .p(px(12.))
            .rounded(super::R_SM)
            .border_1()
            .border_color(colors.border)
            .text_size(FS_SM)
            .flex()
            .items_center()
            .gap(px(6.))
            .text_color(colors.error)
            .child(Icon::new(IconName::Err).size(px(12.)).color(colors.error))
            .child("none — repositories cannot be opened")
            .into_any_element(),
    };

    let redetect = {
        let entity = cx.entity();
        move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
            entity.update(cx, |view, cx| view.start_git_detection(window, cx));
        }
    };
    let status = match active {
        Some(probe) => format!("Status bar shows: git {} · {}", probe.label(), probe.source),
        None => "Status bar shows: git not found".to_owned(),
    };

    let mut page = div().flex().flex_col();
    page = page.child(heading(
        "Git",
        "Choose which git executable GitWarp runs. Changes apply to all repositories after a restart of background operations.",
    ));
    for banner in banners {
        page = page.child(div().max_w(px(760.)).mb(px(12.)).child(banner));
    }
    page = page.child(options);
    page = page.child(
        div()
            .w_full()
            .max_w(px(760.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .mt(px(16.))
            .mb(px(12.))
            .child(
                div()
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child("In use"),
            )
            .child(in_use),
    );
    page = page.child(
        div()
            .w_full()
            .max_w(px(760.))
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                Button::new("settings-git-redetect")
                    .icon(IconName::Fetch)
                    .label("Re-detect")
                    .disabled(loading)
                    .on_click(redetect),
            )
            .when(loading, |this| {
                this.child(Spinner::new("settings-git-spinner", px(14.)))
            })
            .child(div().flex_1())
            .child(
                div()
                    .text_size(FS_SM)
                    .text_color(colors.muted)
                    .child(status),
            ),
    );
    page.into_any_element()
}

fn in_use_line(label: &'static str, value: String, colors: ui::ThemeColors) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .child(div().w(px(90.)).text_color(colors.muted).child(label))
        .child(div().font_family(FONT_MONO).text_size(FS_SM).child(value))
        .into_any_element()
}

fn custom_status(probe: Option<&Probe>, cx: &Context<SettingsView>) -> AnyElement {
    let colors = cx.theme().colors;
    match probe {
        None => div()
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child("Pick the git executable (not a directory). It is validated by running git --version.")
            .into_any_element(),
        Some(probe) if probe.ok() => div()
            .flex()
            .items_center()
            .gap(px(4.))
            .text_size(FS_SM)
            .text_color(colors.success)
            .child(Icon::new(IconName::Check).size(px(12.)).color(colors.success))
            .child(format!("git version {} detected", probe.label()))
            .into_any_element(),
        Some(probe) => div()
            .flex()
            .items_center()
            .gap(px(4.))
            .text_size(FS_SM)
            .text_color(colors.error)
            .child(Icon::new(IconName::Err).size(px(12.)).color(colors.error))
            .child(probe.error.clone().unwrap_or_default())
            .into_any_element(),
    }
}
