//! Settings → General: start-up, updates, background fetch, confirmations and
//! privacy. Everything here is stored even when the feature it configures is
//! not implemented yet, so the file stays forward-compatible.

use gpui::{div, px, AnyElement, Context, IntoElement, ParentElement, Styled, Window};

use ui::{Banner, BannerKind};

use super::form;
use super::{group, heading, SettingsView};
use crate::settings::{Language, OnLaunch, UpdateChannel, FETCH_INTERVALS};

const LANGUAGES: [Language; 6] = [
    Language::En,
    Language::De,
    Language::Fr,
    Language::Es,
    Language::Ru,
    Language::Ja,
];

/// Fetch interval in minutes with the catalog key of its label.
const INTERVALS: [(u16, &str); 4] = [
    (1, "settings.general.interval.1"),
    (5, "settings.general.interval.5"),
    (15, "settings.general.interval.15"),
    (60, "settings.general.interval.60"),
];

fn language_index(language: Language) -> usize {
    LANGUAGES
        .iter()
        .position(|candidate| *candidate == language)
        .unwrap_or(0)
}

fn interval_index(minutes: u16) -> usize {
    FETCH_INTERVALS
        .iter()
        .position(|candidate| *candidate == minutes)
        .unwrap_or(1)
}

fn launch_index(launch: OnLaunch) -> usize {
    match launch {
        OnLaunch::Last => 0,
        OnLaunch::Welcome => 1,
        OnLaunch::Nothing => 2,
    }
}

pub(super) fn render(
    view: &mut SettingsView,
    _window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> AnyElement {
    let settings = view.settings(cx);
    let mut page = div().flex().flex_col();

    page = page.child(heading(
        ui::t!("settings.page.general"),
        ui::t!("settings.general.subtitle"),
    ));
    if view.language_changed {
        page = page.child(
            div().max_w(px(760.)).mb(px(12.)).child(
                Banner::new(BannerKind::Info, ui::t!("settings.general.restart_title"))
                    .body(ui::t!("settings.general.restart_body")),
            ),
        );
    }

    let language = {
        let entity = cx.entity();
        form::select(
            "settings-language".into(),
            LANGUAGES.iter().map(|language| language.label()),
            language_index(settings.language),
            false,
            move |index, _, cx| {
                let language = LANGUAGES[index.min(LANGUAGES.len() - 1)];
                entity.update(cx, |view, cx| {
                    view.language_changed = true;
                    view.change(cx, |settings| settings.language = language);
                });
            },
        )
    };
    let launch = {
        let entity = cx.entity();
        form::segmented(
            "settings-launch".into(),
            [
                ui::t!("settings.general.launch.last"),
                ui::t!("settings.general.launch.welcome"),
                ui::t!("settings.general.launch.nothing"),
            ],
            launch_index(settings.on_launch),
            false,
            move |index, _, cx| {
                let launch = match index {
                    1 => OnLaunch::Welcome,
                    2 => OnLaunch::Nothing,
                    _ => OnLaunch::Last,
                };
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| settings.on_launch = launch);
                });
            },
        )
    };

    let update_switch = {
        let entity = cx.entity();
        form::switch(
            "settings-updates".into(),
            settings.check_updates,
            false,
            move |checked, _, cx| {
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| settings.check_updates = checked);
                });
            },
        )
    };
    let channel = {
        let entity = cx.entity();
        form::segmented(
            "settings-channel".into(),
            [
                ui::t!("settings.general.channel.stable"),
                ui::t!("settings.general.channel.beta"),
            ],
            usize::from(settings.update_channel == UpdateChannel::Beta),
            !settings.check_updates,
            move |index, _, cx| {
                let channel = if index == 1 {
                    UpdateChannel::Beta
                } else {
                    UpdateChannel::Stable
                };
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| settings.update_channel = channel);
                });
            },
        )
    };

    let fetch_switch = {
        let entity = cx.entity();
        form::switch(
            "settings-fetch-auto".into(),
            settings.background_fetch,
            false,
            move |checked, _, cx| {
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| settings.background_fetch = checked);
                });
            },
        )
    };
    let interval = {
        let entity = cx.entity();
        form::select(
            "settings-fetch-interval".into(),
            INTERVALS.iter().map(|(_, key)| ui::i18n::t(key)),
            interval_index(settings.fetch_interval_minutes),
            !settings.background_fetch,
            move |index, _, cx| {
                let minutes = INTERVALS[index.min(INTERVALS.len() - 1)].0;
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| settings.fetch_interval_minutes = minutes);
                });
            },
        )
    };

    page = page.child(group(
        ui::t!("settings.general.group.startup"),
        vec![
            form::row(
                "settings-language-row",
                ui::t!("settings.general.language.title"),
                ui::t!("settings.general.language.desc"),
                language,
                false,
            ),
            form::row(
                "settings-launch-row",
                ui::t!("settings.general.launch.title"),
                ui::t!("settings.general.launch.desc"),
                launch,
                false,
            ),
        ],
    ));
    page = page.child(group(
        ui::t!("settings.general.group.updates"),
        vec![
            form::row(
                "settings-updates-row",
                ui::t!("settings.general.updates.title"),
                ui::t!("settings.general.updates.desc"),
                update_switch,
                false,
            ),
            form::row(
                "settings-channel-row",
                ui::t!("settings.general.channel.title"),
                if settings.check_updates {
                    ui::t!("settings.general.channel.desc_on")
                } else {
                    ui::t!("settings.general.channel.desc_off")
                },
                channel,
                !settings.check_updates,
            ),
        ],
    ));
    page = page.child(group(
        ui::t!("settings.general.group.fetch"),
        vec![
            form::row(
                "settings-fetch-row",
                ui::t!("settings.general.fetch.title"),
                ui::t!("settings.general.fetch.desc"),
                fetch_switch,
                false,
            ),
            form::row(
                "settings-interval-row",
                ui::t!("settings.general.interval.title"),
                if settings.background_fetch {
                    ui::t!("settings.general.interval.desc_on")
                } else {
                    ui::t!("settings.general.interval.desc_off")
                },
                interval,
                !settings.background_fetch,
            ),
        ],
    ));

    let activity_switch = {
        let entity = cx.entity();
        form::switch(
            "settings-show-activity".into(),
            settings.show_activity,
            false,
            move |checked, _, cx| {
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| settings.show_activity = checked);
                });
            },
        )
    };
    page = page.child(group(
        ui::t!("settings.general.group.activity"),
        vec![form::row(
            "settings-show-activity-row",
            ui::t!("settings.general.activity.title"),
            ui::t!("settings.general.activity.desc"),
            activity_switch,
            false,
        )],
    ));

    let mut confirmations = Vec::new();
    for (id, title, description, value) in [
        (
            "settings-confirm-force",
            ui::t!("settings.general.confirm.force.title"),
            ui::t!("settings.general.confirm.force.desc"),
            settings.confirm_force_push,
        ),
        (
            "settings-confirm-discard",
            ui::t!("settings.general.confirm.discard.title"),
            ui::t!("settings.general.confirm.discard.desc"),
            settings.confirm_discard,
        ),
        (
            "settings-confirm-branch",
            ui::t!("settings.general.confirm.branch.title"),
            ui::t!("settings.general.confirm.branch.desc"),
            settings.confirm_delete_branch,
        ),
    ] {
        let entity = cx.entity();
        let control = form::checkbox(
            format!("{id}-check").into(),
            ui::t!("settings.general.confirm.label"),
            value,
            false,
            move |checked, _, cx| {
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| match id {
                        "settings-confirm-force" => settings.confirm_force_push = checked,
                        "settings-confirm-discard" => settings.confirm_discard = checked,
                        _ => settings.confirm_delete_branch = checked,
                    });
                });
            },
        );
        confirmations.push(form::row(
            format!("{id}-row"),
            title,
            description,
            control,
            false,
        ));
    }
    page = page.child(group(
        ui::t!("settings.general.group.confirm"),
        confirmations,
    ));

    let telemetry = {
        let entity = cx.entity();
        form::switch(
            "settings-telemetry".into(),
            settings.telemetry,
            false,
            move |checked, _, cx| {
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| settings.telemetry = checked);
                });
            },
        )
    };
    page = page.child(group(
        ui::t!("settings.general.group.privacy"),
        vec![form::row(
            "settings-telemetry-row",
            ui::t!("settings.general.telemetry.title"),
            ui::t!("settings.general.telemetry.desc"),
            telemetry,
            false,
        )],
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
