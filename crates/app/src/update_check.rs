//! Looks for a newer GitWarp release on GitHub and shows a toast about it.
//!
//! Notification only: nothing is downloaded or installed. The first check runs
//! shortly after the main window opens, then once a day while the app runs.

use std::cmp::Ordering;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::time::{Duration, SystemTime};

use forge::api::{Client, Release, ReleaseAsset};
use gpui::{AnyWindowHandle, App};
use ui::toast::{self, ToastAction, ToastBody, ToastId, ToastKind};

use crate::settings::UpdateChannel;
use crate::updater::{self, StagedUpdate};
use crate::workspace::Workspace;
use crate::AppSettings;

const OWNER: &str = "v1rus-dev";
const REPO: &str = "GitWarp";
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(10);
/// How often the schedule wakes up to see whether a check is due.
const TICK: Duration = Duration::from_secs(60);
/// Minimum time between two successful checks.
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// Minimum time between a failed check and the next attempt.
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(15 * 60);

/// One dot-separated pre-release identifier. The derived order is the SemVer
/// one: numeric identifiers (declared first) are lower than alphanumeric ones,
/// numbers compare numerically and text bytewise (ASCII).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Ident {
    Numeric(u64),
    Text(String),
}

/// A SemVer 2.0 version; build metadata is dropped on parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Vec<Ident>,
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                // A pre-release is lower than the same version without one.
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                // Vec ordering is dot-by-dot, a shorter list is lower when
                // all its identifiers are equal.
                (false, false) => self.pre.cmp(&other.pre),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn parse_number(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Parses `v1.2.3`, `1.2.3-rc.1` or `1.2.3+build`; `None` when it is not SemVer.
/// Only `[0-9A-Za-z.+-]` is accepted (after an optional lowercase `v`), so a
/// parsed tag is safe to put into a URL path.
pub fn parse_version(tag: &str) -> Option<Version> {
    if !tag
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'))
    {
        return None;
    }
    let text = tag.strip_prefix('v').unwrap_or(tag);
    // Build metadata does not take part in precedence.
    let text = text.split('+').next().unwrap_or(text);
    let (core, pre) = match text.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (text, None),
    };
    let mut numbers = core.split('.');
    let major = parse_number(numbers.next()?)?;
    let minor = parse_number(numbers.next()?)?;
    let patch = parse_number(numbers.next()?)?;
    if numbers.next().is_some() {
        return None;
    }
    let pre = match pre {
        None => Vec::new(),
        Some(pre) => pre
            .split('.')
            .map(|id| match parse_number(id) {
                Some(n) => Some(Ident::Numeric(n)),
                None if id.is_empty() => None,
                None => Some(Ident::Text(id.to_string())),
            })
            .collect::<Option<Vec<_>>>()?,
    };
    Some(Version {
        major,
        minor,
        patch,
        pre,
    })
}

/// Displays the version without a leading `v` and without build metadata.
impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        for (index, id) in self.pre.iter().enumerate() {
            f.write_str(if index == 0 { "-" } else { "." })?;
            match id {
                Ident::Numeric(n) => write!(f, "{n}")?,
                Ident::Text(t) => f.write_str(t)?,
            }
        }
        Ok(())
    }
}

/// The highest release of `channel` with its tag. Drafts and tags that are not
/// SemVer are skipped; Stable also skips pre-releases.
fn latest_on_channel(channel: UpdateChannel, releases: &[Release]) -> Option<(Version, String)> {
    releases
        .iter()
        .filter(|release| !release.draft)
        .filter(|release| channel == UpdateChannel::Beta || !release.prerelease)
        .filter_map(|release| match parse_version(&release.tag_name) {
            Some(version) => Some((version, release.tag_name.as_str())),
            None => {
                log::debug!("update check: skipping tag {:?}", release.tag_name);
                None
            }
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(version, tag)| (version, tag.to_string()))
}

/// What to announce: `latest` when it is strictly newer than the running
/// version and than the version an update toast was last shown for.
fn update_to_announce(
    running: &Version,
    last_notified: Option<&Version>,
    latest: Option<(Version, String)>,
) -> Option<(Version, String)> {
    latest.filter(|(version, _)| {
        version > running && last_notified.is_none_or(|notified| version > notified)
    })
}

/// The outcome of a manual check.
#[derive(Debug, PartialEq, Eq)]
enum ManualResult {
    /// No newer release on the channel.
    UpToDate,
    Newer {
        version: Version,
        tag: String,
    },
}

/// Like [`update_to_announce`] but without the "announced once" rule.
fn manual_result(running: &Version, channel: UpdateChannel, releases: &[Release]) -> ManualResult {
    match update_to_announce(running, None, latest_on_channel(channel, releases)) {
        Some((version, tag)) => ManualResult::Newer { version, tag },
        None => ManualResult::UpToDate,
    }
}

/// The release page of `tag`. Only a tag that passed `parse_version` is safe here.
fn release_url(tag: &str) -> String {
    format!("https://github.com/{OWNER}/{REPO}/releases/tag/{tag}")
}

/// The release notes page of the running version.
pub fn release_notes_url() -> String {
    release_url(&format!("v{}", env!("CARGO_PKG_VERSION")))
}

/// The issue tracker page.
pub fn issue_tracker_url() -> String {
    format!("https://github.com/{OWNER}/{REPO}/issues")
}

/// Set while a manual check runs, so repeated clicks do not stack requests.
static MANUAL_BUSY: AtomicBool = AtomicBool::new(false);

fn toast_in(window: AnyWindowHandle, cx: &mut App, kind: ToastKind, text: String) {
    let _ = window.update(cx, |_, window, cx| toast::show(kind, text, window, cx));
}

/// Checks for an update now, whatever the "check automatically" setting says
/// (the update channel is respected), and reports in a toast in `window`.
pub fn check_now(window: AnyWindowHandle, cx: &mut App) {
    let Some(running) = parse_version(env!("CARGO_PKG_VERSION")) else {
        return;
    };
    let Some(channel) = read_settings(cx).map(|s| s.channel) else {
        return;
    };
    if MANUAL_BUSY.swap(true, AtomicOrdering::SeqCst) {
        return;
    }
    cx.spawn(async move |cx| {
        let fetched = cx
            .background_executor()
            .spawn(async move { Client::new().list_releases(OWNER, REPO) })
            .await;
        let releases = match fetched {
            Ok(releases) => releases,
            Err(e) => {
                log::warn!("manual update check failed: {e}");
                MANUAL_BUSY.store(false, AtomicOrdering::SeqCst);
                cx.update(|cx| {
                    let text = ui::t!("app.update.check_failed", reason = e.to_string());
                    toast_in(window, cx, ToastKind::Error, text.to_string());
                });
                return;
            }
        };
        match manual_result(&running, channel, &releases) {
            ManualResult::UpToDate => {
                MANUAL_BUSY.store(false, AtomicOrdering::SeqCst);
                cx.update(|cx| {
                    let text = ui::t!("app.update.up_to_date", version = running.to_string());
                    toast_in(window, cx, ToastKind::Ok, text.to_string());
                });
            }
            ManualResult::Newer { version, tag } => {
                let assets = releases
                    .iter()
                    .find(|r| r.tag_name == tag)
                    .map(|release| release.assets.clone())
                    .unwrap_or_default();
                let staged = cx
                    .background_executor()
                    .spawn(async { updater::find_staged() })
                    .await
                    .filter(|s| parse_version(&s.version).as_ref() == Some(&version));
                let ready = staged.is_some();
                let toast_id =
                    cx.update(|cx| show_update_toast(cx, window, None, staged, &version, &tag));
                if !ready {
                    let kind = updater::detect_install_kind();
                    if updater::can_apply(kind)
                        && updater::asset_for(kind, &version.to_string(), &assets).is_some()
                    {
                        let stage_tag = tag.clone();
                        let result = cx
                            .background_executor()
                            .spawn(async move {
                                let cancel = AtomicBool::new(false);
                                updater::stage(&stage_tag, &assets, kind, |_, _| {}, &cancel)
                            })
                            .await;
                        match result {
                            Ok(staged) => {
                                cx.update(|cx| {
                                    let previous = toast_id.map(|id| (window, id));
                                    show_update_toast(
                                        cx,
                                        window,
                                        previous,
                                        Some(staged),
                                        &version,
                                        &tag,
                                    );
                                });
                            }
                            Err(e) => log::warn!("staging update {version} failed: {e}"),
                        }
                    }
                }
                MANUAL_BUSY.store(false, AtomicOrdering::SeqCst);
            }
        }
    })
    .detach();
}

/// A check that failed: when, and on which channel.
type Failure = (UpdateChannel, SystemTime);

/// Whether a check is due. After a failure on this channel: once
/// [`RETRY_AFTER_FAILURE`] passed. Otherwise: none succeeded yet, the channel
/// changed since the last success, or 24 hours passed. `SystemTime` is wall
/// clock, so time spent suspended counts; a clock that went backwards counts
/// as due.
fn check_due(
    last_success: Option<(UpdateChannel, SystemTime)>,
    last_failure: Option<Failure>,
    channel: UpdateChannel,
    now: SystemTime,
) -> bool {
    let elapsed_at_least = |at: SystemTime, wait: Duration| {
        now.duration_since(at)
            .map_or(true, |elapsed| elapsed >= wait)
    };
    if let Some((failed_channel, at)) = last_failure {
        if failed_channel == channel {
            return elapsed_at_least(at, RETRY_AFTER_FAILURE);
        }
    }
    let Some((last_channel, at)) = last_success else {
        return true;
    };
    last_channel != channel || elapsed_at_least(at, CHECK_INTERVAL)
}

/// The settings the schedule depends on.
struct UpdateSettings {
    enabled: bool,
    channel: UpdateChannel,
    /// The version an update toast was last shown for; an unparsable stored
    /// value counts as none.
    notified: Option<Version>,
}

fn read_settings(cx: &App) -> Option<UpdateSettings> {
    let settings = &cx.try_global::<AppSettings>()?.settings;
    Some(UpdateSettings {
        enabled: settings.check_updates,
        channel: settings.update_channel,
        notified: settings
            .update_notified_version
            .as_deref()
            .and_then(parse_version),
    })
}

/// The "Restart to update" toast button: applies `staged` off the UI thread,
/// then quits (the installer helper or the new AppImage starts the app again).
fn restart_action(staged: StagedUpdate) -> ToastAction {
    ToastAction::new(ui::t!("app.update.restart"), move |_, cx| {
        let staged = staged.clone();
        cx.spawn(async move |cx| {
            let result = cx
                .background_executor()
                .spawn(async move { updater::apply(&staged, true) })
                .await;
            cx.update(|cx| match result {
                Ok(()) => cx.quit(),
                Err(e) => {
                    log::warn!("applying the update failed: {e}");
                    let text = ui::t!("app.update.apply_failed", reason = e.to_string());
                    if let Some(window) = live_workspace_window(cx) {
                        let _ = window.update(cx, |_, window, cx| {
                            toast::show_with(ToastKind::Error, text, None, window, cx)
                        });
                    }
                }
            });
        })
        .detach();
    })
}

/// Shows the update toast in `window`, replacing `previous` (the earlier update
/// toast; its window may be gone). With `staged` it is the "downloaded, ready to
/// install" variant, which offers "Restart to update" when this installation can
/// apply the update (the toast has room for one button, so "View release" is
/// left out then; the earlier "available" toast had it). Returns the new toast.
fn show_update_toast(
    cx: &mut App,
    window: AnyWindowHandle,
    previous: Option<(AnyWindowHandle, ToastId)>,
    staged: Option<StagedUpdate>,
    version: &Version,
    tag: &str,
) -> Option<ToastId> {
    if let Some((old_window, old_id)) = previous {
        // Done before `window.update`: GPUI takes the window out of its slot
        // while it is updated, so a nested update of the same window would fail
        // and skip the dismissal.
        let _ = old_window.update(cx, |_, window, cx| {
            toast::dismiss(old_id, window, cx);
        });
    }
    // Built here, not taken from the API response: only a tag that passed
    // `parse_version` is put into the URL.
    let url = release_url(tag);
    let shown_version = format!("v{version}");
    let ready = staged.is_some();
    let apply = staged.filter(|staged| updater::can_apply(staged.kind));
    window
        .update(cx, |_, window, cx| {
            let action = match apply {
                Some(staged) => restart_action(staged),
                None => ToastAction::new(ui::t!("app.update.view_release"), move |_, cx| {
                    cx.open_url(&url);
                }),
            };
            let text = if ready {
                ui::t!("app.update.ready", version = shown_version)
            } else {
                ui::t!("app.update.available", version = shown_version)
            };
            toast::show_with(
                ToastKind::Info,
                text,
                Some(ToastBody {
                    lines: Vec::new(),
                    action: Some(action),
                    ..Default::default()
                }),
                window,
                cx,
            )
        })
        .ok()
}

/// A live `Workspace` window: the active one if it is a workspace, else the
/// first workspace window.
fn live_workspace_window(cx: &App) -> Option<AnyWindowHandle> {
    let active = cx
        .active_window()
        .filter(|window| window.downcast::<Workspace>().is_some());
    active.or_else(|| {
        cx.windows()
            .into_iter()
            .find(|window| window.downcast::<Workspace>().is_some())
    })
}

/// Applies a staged update when the app quits (if the settings allow it), so
/// the next start runs the new version. Called from the quit hook; the apply
/// itself is quick (a copy or spawning the installer helper).
pub fn apply_on_quit(cx: &mut App) {
    let Some(settings) = cx.try_global::<AppSettings>().map(|g| &g.settings) else {
        return;
    };
    if !(settings.check_updates && settings.auto_install_updates) {
        return;
    }
    match updater::apply_newest_staged() {
        Ok(Some(version)) => log::info!("update {version} handed over to be installed on quit"),
        Ok(None) => {}
        Err(e) => log::warn!("installing the update on quit failed: {e}"),
    }
}

/// Starts the update schedule for the lifetime of the app. The first tick is
/// ten seconds after the call, then it wakes up every minute. On each tick it
/// reads the current settings and does nothing when `check_updates` is off;
/// otherwise it checks when no check has succeeded yet this session, the
/// channel differs from the last successful check, or 24 hours (wall clock)
/// passed since it. A failed request is retried after 15 minutes (at once on
/// a channel change or after the checks were switched off and on). A newer
/// version than the one last announced (stored in the settings) is shown in a
/// live workspace window, waiting for one if there is none. A newer version
/// is downloaded and verified whenever none is staged for it yet (also when it
/// was announced in an earlier session) if this installation can apply it, and
/// an update staged by an earlier session is found at start; either gets a
/// "ready" toast once per launch.
pub fn start(cx: &mut App) {
    let Some(running) = parse_version(env!("CARGO_PKG_VERSION")) else {
        log::warn!("update check disabled: the running version is not SemVer");
        return;
    };
    cx.spawn(async move |cx| {
        let mut last_success: Option<(UpdateChannel, SystemTime)> = None;
        let mut last_failure: Option<Failure> = None;
        // An update to announce as soon as a window is available, with the
        // channel it was found on.
        let mut pending: Option<(UpdateChannel, Version, String)> = None;
        // The newest release on the channel to download, with its files.
        let mut to_stage: Option<(Version, String, Vec<ReleaseAsset>)> = None;
        // A staged update whose "ready" toast is still to be shown.
        let mut ready_pending: Option<StagedUpdate> = cx
            .background_executor()
            .spawn(async { updater::find_staged() })
            .await;
        // The version that is staged (found at start or downloaded), and the
        // one a "ready" toast was shown for in this launch.
        let mut staged_version: Option<Version> = ready_pending
            .as_ref()
            .and_then(|staged| parse_version(&staged.version));
        let mut ready_shown: Option<Version> = None;
        let mut last_toast: Option<(AnyWindowHandle, ToastId)> = None;
        let mut delay = FIRST_CHECK_DELAY;
        loop {
            cx.background_executor().timer(delay).await;
            delay = TICK;

            let Some(UpdateSettings {
                enabled: true,
                channel,
                notified,
            }) = cx.update(|cx| read_settings(cx))
            else {
                // Switching the checks back on retries a failed one at once.
                last_failure = None;
                continue;
            };

            if check_due(last_success, last_failure, channel, SystemTime::now()) {
                let fetched = cx
                    .background_executor()
                    .spawn(async move { Client::new().list_releases(OWNER, REPO) })
                    .await;
                let releases = match fetched {
                    Ok(releases) => releases,
                    Err(e) => {
                        log::warn!("update check failed: {e}");
                        last_failure = Some((channel, SystemTime::now()));
                        continue;
                    }
                };
                last_success = Some((channel, SystemTime::now()));
                last_failure = None;

                let latest = latest_on_channel(channel, &releases);
                log::debug!(
                    "update check: channel {channel:?}, running {running}, latest {}",
                    latest
                        .as_ref()
                        .map_or_else(|| "none".to_string(), |(v, _)| v.to_string())
                );
                let assets_of = |tag: &str| {
                    releases
                        .iter()
                        .find(|r| r.tag_name == tag)
                        .map(|release| release.assets.clone())
                        .unwrap_or_default()
                };
                to_stage = latest
                    .as_ref()
                    .filter(|(version, _)| {
                        *version > running && staged_version.as_ref() != Some(version)
                    })
                    .map(|(version, tag)| (version.clone(), tag.clone(), assets_of(tag)));
                pending = update_to_announce(&running, notified.as_ref(), latest)
                    .map(|(version, tag)| (channel, version, tag));
            }

            'announce: {
                let Some((pending_channel, version, tag)) = pending.clone() else {
                    break 'announce;
                };
                // Read again: the settings may have changed while the request ran.
                let Some(now) = cx.update(|cx| read_settings(cx)) else {
                    break 'announce;
                };
                if !now.enabled {
                    break 'announce;
                }
                if now.channel != pending_channel {
                    // Found on another channel; the next tick checks this one.
                    pending = None;
                    break 'announce;
                }
                let Some((version, tag)) =
                    update_to_announce(&running, now.notified.as_ref(), Some((version, tag)))
                else {
                    pending = None;
                    break 'announce;
                };

                let previous = last_toast;
                // A "ready" toast for this version already says more.
                let already_ready = ready_shown.as_ref() == Some(&version);
                let shown = cx.update(|cx| {
                    let window = live_workspace_window(cx)?;
                    let id = if already_ready {
                        None
                    } else {
                        Some(show_update_toast(
                            cx, window, previous, None, &version, &tag,
                        )?)
                    };
                    // Announced once: later launches skip this version.
                    let announced = version.to_string();
                    crate::update_settings(cx, |settings| {
                        settings.update_notified_version = Some(announced);
                    });
                    Some((window, id))
                });
                // With no live workspace window the update stays pending.
                if let Some((window, id)) = shown {
                    if let Some(id) = id {
                        last_toast = Some((window, id));
                    }
                    pending = None;
                }
            }

            // Download and verify the update when this installation can apply
            // one.
            if let Some((version, tag, assets)) = to_stage.take() {
                let kind = updater::detect_install_kind();
                if updater::can_apply(kind)
                    && updater::asset_for(kind, &version.to_string(), &assets).is_some()
                {
                    let staged = cx
                        .background_executor()
                        .spawn(async move {
                            let cancel = AtomicBool::new(false);
                            updater::stage(&tag, &assets, kind, |_, _| {}, &cancel)
                        })
                        .await;
                    match staged {
                        Ok(staged) => {
                            log::info!(
                                "update {} staged at {}",
                                staged.version,
                                staged.path.display()
                            );
                            staged_version = Some(version);
                            ready_pending = Some(staged);
                        }
                        // The "available" toast stays.
                        Err(e) => log::warn!("staging update {version} failed: {e}"),
                    }
                }
            }

            // The "ready" toast, once per version and launch.
            if let Some(staged) = ready_pending.take() {
                match parse_version(&staged.version) {
                    Some(version) if ready_shown.as_ref() != Some(&version) => {
                        let previous = last_toast;
                        let tag = format!("v{version}");
                        let shown = cx.update(|cx| {
                            let window = live_workspace_window(cx)?;
                            let id = show_update_toast(
                                cx,
                                window,
                                previous,
                                Some(staged.clone()),
                                &version,
                                &tag,
                            )?;
                            Some((window, id))
                        });
                        match shown {
                            Some(toast) => {
                                last_toast = Some(toast);
                                ready_shown = Some(version);
                            }
                            // No live window yet: try again on the next tick.
                            None => ready_pending = Some(staged),
                        }
                    }
                    _ => {}
                }
            }
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        parse_version(text).unwrap_or_else(|| panic!("{text} should parse"))
    }

    fn release(tag: &str, prerelease: bool, draft: bool) -> Release {
        Release {
            tag_name: tag.to_string(),
            prerelease,
            draft,
            assets: Vec::new(),
        }
    }

    /// What `start` decides after a check: the channel's latest release, then
    /// the announce rule.
    fn announce(
        running: &str,
        last_notified: Option<&str>,
        channel: UpdateChannel,
        releases: &[Release],
    ) -> Option<(Version, String)> {
        let last_notified = last_notified.map(v);
        update_to_announce(
            &v(running),
            last_notified.as_ref(),
            latest_on_channel(channel, releases),
        )
    }

    #[test]
    fn parses_with_and_without_v_and_ignores_build_metadata() {
        assert_eq!(v("v0.2.0"), v("0.2.0"));
        assert_eq!(v("1.2.3+build.5"), v("1.2.3"));
        assert_eq!(v("v0.2.0-rc.1").to_string(), "0.2.0-rc.1");
        assert!(parse_version("nightly").is_none());
        assert!(parse_version("v1.2").is_none());
        assert!(parse_version("v1.2.3.4").is_none());
        assert!(parse_version("v1.2.x").is_none());
        assert!(parse_version("v1.2.3-").is_none());
    }

    #[test]
    fn tags_with_url_unsafe_characters_are_rejected() {
        for tag in [
            "v1.0.0/../x",
            "v1.0.0?x=1",
            "v1.0.0#f",
            "v1.0.0 ",
            "v1.0.0%2f",
            "v1.0.0-rc_1",
            "v1.0.0+b/c",
            "vv1.0.0",
            "V1.0.0",
        ] {
            assert!(parse_version(tag).is_none(), "{tag:?}");
        }
    }

    #[test]
    fn semver_precedence_chain() {
        let chain = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
        ];
        for pair in chain.windows(2) {
            assert!(v(pair[0]) < v(pair[1]), "{} < {}", pair[0], pair[1]);
        }
        assert!(v("0.10.0") > v("0.9.0"));
    }

    #[test]
    fn check_is_due_on_first_run_channel_change_and_after_a_day() {
        use UpdateChannel::{Beta, Stable};
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        assert!(check_due(None, None, Stable, t0));
        let last = Some((Stable, t0));
        assert!(!check_due(
            last,
            None,
            Stable,
            t0 + Duration::from_secs(3600)
        ));
        assert!(check_due(last, None, Beta, t0 + Duration::from_secs(3600)));
        assert!(!check_due(
            last,
            None,
            Stable,
            t0 + CHECK_INTERVAL - Duration::from_secs(1)
        ));
        assert!(check_due(last, None, Stable, t0 + CHECK_INTERVAL));
        // A clock that went backwards is treated as due.
        assert!(check_due(last, None, Stable, t0 - Duration::from_secs(60)));
    }

    #[test]
    fn a_failed_check_is_retried_after_fifteen_minutes() {
        use UpdateChannel::{Beta, Stable};
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let failed = Some((Stable, t0));
        assert!(!check_due(
            None,
            failed,
            Stable,
            t0 + Duration::from_secs(60)
        ));
        assert!(!check_due(
            None,
            failed,
            Stable,
            t0 + RETRY_AFTER_FAILURE - Duration::from_secs(1)
        ));
        assert!(check_due(None, failed, Stable, t0 + RETRY_AFTER_FAILURE));
        // Another channel is checked at the next tick.
        assert!(check_due(None, failed, Beta, t0 + Duration::from_secs(60)));
    }

    fn manual(running: &str, channel: UpdateChannel, releases: &[Release]) -> ManualResult {
        manual_result(&v(running), channel, releases)
    }

    #[test]
    fn manual_result_table() {
        use UpdateChannel::{Beta, Stable};
        let newer = |version: &str, tag: &str| ManualResult::Newer {
            version: v(version),
            tag: tag.to_string(),
        };
        // Empty list, equal and older-only releases are up to date.
        assert_eq!(manual("0.4.0", Stable, &[]), ManualResult::UpToDate);
        let equal = [release("v0.4.0", false, false)];
        assert_eq!(manual("0.4.0", Stable, &equal), ManualResult::UpToDate);
        let older = [release("v0.3.0", false, false)];
        assert_eq!(manual("0.4.0", Stable, &older), ManualResult::UpToDate);
        // Newer on stable.
        let releases = [
            release("v0.4.0", false, false),
            release("v0.5.0", false, false),
        ];
        assert_eq!(manual("0.4.0", Stable, &releases), newer("0.5.0", "v0.5.0"));
        // A pre-release is hidden on stable and visible on beta.
        let pre = [release("v0.5.0-rc.1", true, false)];
        assert_eq!(manual("0.4.0", Stable, &pre), ManualResult::UpToDate);
        assert_eq!(
            manual("0.4.0", Beta, &pre),
            newer("0.5.0-rc.1", "v0.5.0-rc.1")
        );
    }

    #[test]
    fn help_urls() {
        assert_eq!(
            release_url("v1.2.3"),
            "https://github.com/v1rus-dev/GitWarp/releases/tag/v1.2.3"
        );
        assert_eq!(
            release_notes_url(),
            format!(
                "https://github.com/v1rus-dev/GitWarp/releases/tag/v{}",
                env!("CARGO_PKG_VERSION")
            )
        );
        assert_eq!(
            issue_tracker_url(),
            "https://github.com/v1rus-dev/GitWarp/issues"
        );
    }

    #[test]
    fn same_version_is_not_an_update() {
        let releases = [release("v0.1.0", false, false)];
        assert!(announce("0.1.0", None, UpdateChannel::Stable, &releases).is_none());
    }

    #[test]
    fn rc_running_gets_the_final_release_on_stable() {
        let releases = [release("v0.2.0", false, false)];
        assert_eq!(
            announce("0.2.0-rc.1", None, UpdateChannel::Stable, &releases),
            Some((v("0.2.0"), "v0.2.0".to_string()))
        );
        let older = [release("v0.1.0", false, false)];
        assert!(announce("0.2.0-rc.1", None, UpdateChannel::Stable, &older).is_none());
    }

    #[test]
    fn invalid_tags_and_drafts_are_skipped() {
        let releases = [
            release("nightly", false, false),
            release("v9.0.0", false, true),
            release("v0.3.0", false, false),
        ];
        let (version, tag) = announce("0.1.0", None, UpdateChannel::Stable, &releases).unwrap();
        assert_eq!(version, v("0.3.0"));
        assert_eq!(tag, "v0.3.0");
    }

    #[test]
    fn stable_ignores_prereleases_and_beta_takes_them() {
        let releases = [
            release("v0.2.0", false, false),
            release("v0.3.0-rc.1", true, false),
        ];
        let (stable, _) = announce("0.1.0", None, UpdateChannel::Stable, &releases).unwrap();
        assert_eq!(stable, v("0.2.0"));
        let (beta, tag) = announce("0.1.0", None, UpdateChannel::Beta, &releases).unwrap();
        assert_eq!(beta, v("0.3.0-rc.1"));
        assert_eq!(tag, "v0.3.0-rc.1");
    }

    #[test]
    fn only_versions_newer_than_the_last_announced_are_announced() {
        let releases = [release("v0.2.0", false, false)];
        let stable = UpdateChannel::Stable;
        assert!(announce("0.1.0", Some("0.2.0"), stable, &releases).is_none());
        assert!(announce("0.1.0", Some("0.3.0"), stable, &releases).is_none());
        assert!(announce("0.1.0", Some("0.1.5"), stable, &releases).is_some());
    }
}
