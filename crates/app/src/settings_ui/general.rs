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

const INTERVALS: [(u16, &str); 4] = [
    (1, "Every minute"),
    (5, "Every 5 minutes"),
    (15, "Every 15 minutes"),
    (60, "Every hour"),
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
        "General",
        "Startup, updates and safety prompts. Applies to every repository.",
    ));
    if view.language_changed {
        page = page.child(
            div().max_w(px(760.)).mb(px(12.)).child(
                Banner::new(BannerKind::Info, "Restart required")
                    .body("The interface language changes the next time GitWarp starts."),
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
            ["Last repository", "Welcome", "Nothing"],
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
            ["Stable", "Beta"],
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
            INTERVALS.iter().map(|(_, label)| *label),
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
        "Startup",
        vec![
            form::row(
                "settings-language-row",
                "Language",
                "Interface language.",
                language,
                false,
            ),
            form::row(
                "settings-launch-row",
                "On launch",
                "What opens when GitWarp starts.",
                launch,
                false,
            ),
        ],
    ));
    page = page.child(group(
        "Updates",
        vec![
            form::row(
                "settings-updates-row",
                "Check for updates automatically",
                "GitWarp downloads updates in the background and asks before installing.",
                update_switch,
                false,
            ),
            form::row(
                "settings-channel-row",
                "Update channel",
                if settings.check_updates {
                    "Beta builds ship weekly and may contain regressions."
                } else {
                    "Turn on automatic updates to pick a channel."
                },
                channel,
                !settings.check_updates,
            ),
        ],
    ));
    page = page.child(group(
        "Background fetch",
        vec![
            form::row(
                "settings-fetch-row",
                "Fetch remotes automatically",
                "Keeps ahead/behind counts and the graph current without touching your working tree.",
                fetch_switch,
                false,
            ),
            form::row(
                "settings-interval-row",
                "Interval",
                if settings.background_fetch {
                    "Fetch pauses while the window is in the background."
                } else {
                    "Automatic fetch is off."
                },
                interval,
                !settings.background_fetch,
            ),
        ],
    ));

    let mut confirmations = Vec::new();
    for (id, title, description, value) in [
        (
            "settings-confirm-force",
            "Force push",
            "Rewrites remote history; always shows which commits will be lost.",
            settings.confirm_force_push,
        ),
        (
            "settings-confirm-discard",
            "Discarding changes",
            "Discarded edits cannot be recovered.",
            settings.confirm_discard,
        ),
        (
            "settings-confirm-branch",
            "Deleting a branch",
            "Unmerged commits stay reachable from the reflog for 30 days.",
            settings.confirm_delete_branch,
        ),
    ] {
        let entity = cx.entity();
        let control = form::checkbox(
            format!("{id}-check").into(),
            "Confirm",
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
    page = page.child(group("Ask before…", confirmations));

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
        "Privacy",
        vec![form::row(
            "settings-telemetry-row",
            "Send anonymous usage statistics",
            "Counts of features used and crash reports. Never repository names, paths, commit messages or diffs.",
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
            .child("Settings file: ")
            .child(div().font_family(ui::metrics::FONT_MONO).child(path)),
    )
    .into_any_element()
}
