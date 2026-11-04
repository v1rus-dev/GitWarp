//! Looks for a newer GitWarp release on GitHub and shows a toast about it.
//!
//! Notification only: nothing is downloaded or installed. The first check runs
//! shortly after the main window opens, then once a day while the app runs.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::time::{Duration, SystemTime};

use forge::api::{Client, Release};
use gpui::{AnyWindowHandle, App};
use ui::toast::{self, ToastAction, ToastBody, ToastId, ToastKind};

use crate::settings::UpdateChannel;
use crate::workspace::Workspace;
use crate::AppSettings;

const OWNER: &str = "v1rus-dev";
const REPO: &str = "GitWarp";
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(10);
/// How often the schedule wakes up to see whether a check is due.
const TICK: Duration = Duration::from_secs(60 * 60);
/// Minimum time between two successful checks.
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// One dot-separated pre-release identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Ident {
    Numeric(u64),
    Text(String),
}

impl Ord for Ident {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Ident::Numeric(a), Ident::Numeric(b)) => a.cmp(b),
            // Numeric identifiers are lower than alphanumeric ones.
            (Ident::Numeric(_), Ident::Text(_)) => Ordering::Less,
            (Ident::Text(_), Ident::Numeric(_)) => Ordering::Greater,
            (Ident::Text(a), Ident::Text(b)) => a.as_bytes().cmp(b.as_bytes()),
        }
    }
}

impl PartialOrd for Ident {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A SemVer 2.0 version; build metadata is dropped on parsing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
pub fn parse_version(tag: &str) -> Option<Version> {
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
            .map(|id| {
                if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
                    None
                } else if id.bytes().all(|b| b.is_ascii_digit()) {
                    parse_number(id).map(Ident::Numeric)
                } else {
                    Some(Ident::Text(id.to_string()))
                }
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

/// The highest release of `channel` that is strictly newer than `running`,
/// with its page URL. Drafts and tags that are not SemVer are skipped.
// `start` needs the latest release even when it is not newer (for the debug
// log), so it composes `latest_on_channel` itself; this pure form is the
// tested statement of the rule.
#[cfg(test)]
pub fn pick_update(
    running: &Version,
    channel: UpdateChannel,
    releases: &[Release],
) -> Option<(Version, String)> {
    let latest = latest_on_channel(channel, releases)?;
    (latest.0 > *running).then_some(latest)
}

fn latest_on_channel(channel: UpdateChannel, releases: &[Release]) -> Option<(Version, String)> {
    releases
        .iter()
        .filter(|release| !release.draft)
        .filter(|release| channel == UpdateChannel::Beta || !release.prerelease)
        .filter_map(|release| match parse_version(&release.tag_name) {
            Some(version) => Some((version, release.html_url.clone())),
            None => {
                log::debug!("update check: skipping tag {:?}", release.tag_name);
                None
            }
        })
        .max_by(|a, b| a.0.cmp(&b.0))
}

/// Whether a check is due: none succeeded yet, the channel changed since the
/// last success, or 24 hours passed. `SystemTime` is wall clock, so time spent
/// suspended counts; a clock that went backwards counts as due.
fn check_due(
    last: Option<(UpdateChannel, SystemTime)>,
    channel: UpdateChannel,
    now: SystemTime,
) -> bool {
    let Some((last_channel, at)) = last else {
        return true;
    };
    last_channel != channel
        || now
            .duration_since(at)
            .map_or(true, |elapsed| elapsed >= CHECK_INTERVAL)
}

/// The current `(check_updates, update_channel)` settings, `None` when the
/// settings global is not set.
fn read_settings(cx: &App) -> Option<(bool, UpdateChannel)> {
    cx.try_global::<AppSettings>()
        .map(|state| (state.settings.check_updates, state.settings.update_channel))
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

/// Starts the update schedule for the lifetime of the app. The first tick is
/// ten seconds after the call, then it wakes up every hour. On each tick it
/// reads the current settings and does nothing when `check_updates` is off;
/// otherwise it checks when no check has succeeded yet this session, the
/// channel differs from the last successful check, or 24 hours (wall clock)
/// passed since it. A failed request changes nothing, so the next tick
/// retries. The toast is shown in a live workspace window, once per version.
pub fn start(cx: &mut App) {
    let Some(running) = parse_version(env!("CARGO_PKG_VERSION")) else {
        log::warn!("update check disabled: the running version is not SemVer");
        return;
    };
    cx.spawn(async move |cx| {
        let mut last_success: Option<(UpdateChannel, SystemTime)> = None;
        let mut notified: HashSet<Version> = HashSet::new();
        let mut last_toast: Option<(AnyWindowHandle, ToastId)> = None;
        let mut delay = FIRST_CHECK_DELAY;
        loop {
            cx.background_executor().timer(delay).await;
            delay = TICK;

            let Some((true, channel)) = cx.update(|cx| read_settings(cx)) else {
                continue;
            };
            if !check_due(last_success, channel, SystemTime::now()) {
                continue;
            }

            let fetched = cx
                .background_executor()
                .spawn(async move { Client::new().list_releases(OWNER, REPO) })
                .await;
            let releases = match fetched {
                Ok(releases) => releases,
                Err(e) => {
                    log::warn!("update check failed: {e}");
                    continue;
                }
            };
            last_success = Some((channel, SystemTime::now()));

            let latest = latest_on_channel(channel, &releases);
            log::debug!(
                "update check: channel {channel:?}, running {running}, latest {}",
                latest
                    .as_ref()
                    .map_or_else(|| "none".to_string(), |(v, _)| v.to_string())
            );
            let Some((version, url)) = latest.filter(|(v, _)| *v > running) else {
                continue;
            };
            if notified.contains(&version) {
                continue;
            }

            // The settings may have changed while the request ran.
            if cx.update(|cx| read_settings(cx)) != Some((true, channel)) {
                continue;
            }
            let previous = last_toast;
            let shown = cx.update(|cx| {
                let window = live_workspace_window(cx)?;
                let mut id = None;
                if let Some((old_window, old_id)) = previous {
                    // Replace the earlier update toast; its window may be gone.
                    // Done before `window.update`: GPUI takes the window out of
                    // its slot while it is updated, so a nested update of the
                    // same window would fail and skip the dismissal.
                    let _ = old_window.update(cx, |_, window, cx| {
                        toast::dismiss(old_id, window, cx);
                    });
                }
                let result = window.update(cx, |_, window, cx| {
                    let action =
                        ToastAction::new(ui::t!("app.update.view_release"), move |_, cx| {
                            cx.open_url(&url);
                        });
                    id = Some(toast::show_persistent(
                        ToastKind::Info,
                        ui::t!("app.update.available", version = version.to_string()),
                        Some(ToastBody {
                            lines: Vec::new(),
                            action: Some(action),
                        }),
                        window,
                        cx,
                    ));
                });
                result.ok()?;
                id.map(|id| (window, id))
            });
            match shown {
                Some(toast) => {
                    last_toast = Some(toast);
                    notified.insert(version);
                }
                // No live workspace window: try again on the next tick.
                None => last_success = None,
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
            html_url: format!("https://example.test/{tag}"),
            prerelease,
            draft,
        }
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
        assert!(check_due(None, Stable, t0));
        let last = Some((Stable, t0));
        assert!(!check_due(last, Stable, t0 + Duration::from_secs(3600)));
        assert!(check_due(last, Beta, t0 + Duration::from_secs(3600)));
        assert!(!check_due(
            last,
            Stable,
            t0 + CHECK_INTERVAL - Duration::from_secs(1)
        ));
        assert!(check_due(last, Stable, t0 + CHECK_INTERVAL));
        // A clock that went backwards is treated as due.
        assert!(check_due(last, Stable, t0 - Duration::from_secs(60)));
    }

    #[test]
    fn same_version_is_not_an_update() {
        let releases = [release("v0.1.0", false, false)];
        assert!(pick_update(&v("0.1.0"), UpdateChannel::Stable, &releases).is_none());
    }

    #[test]
    fn rc_running_gets_the_final_release_on_stable() {
        let releases = [release("v0.2.0", false, false)];
        let picked = pick_update(&v("0.2.0-rc.1"), UpdateChannel::Stable, &releases);
        assert_eq!(
            picked,
            Some((v("0.2.0"), "https://example.test/v0.2.0".to_string()))
        );
        let older = [release("v0.1.0", false, false)];
        assert!(pick_update(&v("0.2.0-rc.1"), UpdateChannel::Stable, &older).is_none());
    }

    #[test]
    fn invalid_tags_and_drafts_are_skipped() {
        let releases = [
            release("nightly", false, false),
            release("v9.0.0", false, true),
            release("v0.3.0", false, false),
        ];
        let (version, _) = pick_update(&v("0.1.0"), UpdateChannel::Stable, &releases).unwrap();
        assert_eq!(version, v("0.3.0"));
    }

    #[test]
    fn stable_ignores_prereleases_and_beta_takes_them() {
        let releases = [
            release("v0.2.0", false, false),
            release("v0.3.0-rc.1", true, false),
        ];
        let (stable, _) = pick_update(&v("0.1.0"), UpdateChannel::Stable, &releases).unwrap();
        assert_eq!(stable, v("0.2.0"));
        let (beta, url) = pick_update(&v("0.1.0"), UpdateChannel::Beta, &releases).unwrap();
        assert_eq!(beta, v("0.3.0-rc.1"));
        assert_eq!(url, "https://example.test/v0.3.0-rc.1");
    }
}
