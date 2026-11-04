//! Looks for a newer GitWarp release on GitHub and shows a toast about it.
//!
//! Notification only: nothing is downloaded or installed. The first check runs
//! shortly after the main window opens, then once a day while the app runs.

use std::cmp::Ordering;
use std::time::Duration;

use forge::api::{Client, Release};
use gpui::{App, WindowHandle};
use ui::toast::{self, ToastAction, ToastBody, ToastKind};

use crate::settings::UpdateChannel;
use crate::workspace::Workspace;
use crate::AppSettings;

const OWNER: &str = "v1rus-dev";
const REPO: &str = "GitWarp";
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(10);
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// One dot-separated pre-release identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Starts the update schedule for `window`: the first check after ten
/// seconds, then every 24 hours until the window is gone. Each round reads the
/// current `check_updates` / `update_channel` settings and does nothing when
/// checks are off.
pub fn start(window: WindowHandle<Workspace>, cx: &mut App) {
    cx.spawn(async move |cx| {
        let mut notified: Option<Version> = None;
        let mut delay = FIRST_CHECK_DELAY;
        loop {
            cx.background_executor().timer(delay).await;
            delay = CHECK_INTERVAL;

            let settings = cx.update(|cx| {
                cx.try_global::<AppSettings>()
                    .map(|state| (state.settings.check_updates, state.settings.update_channel))
            });
            let Some((true, channel)) = settings else {
                continue;
            };
            let Some(running) = parse_version(env!("CARGO_PKG_VERSION")) else {
                log::warn!("update check: running version is not SemVer");
                return;
            };

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
            let latest = latest_on_channel(channel, &releases);
            log::debug!(
                "update check: channel {channel:?}, running {running}, latest {}",
                latest
                    .as_ref()
                    .map_or_else(|| "none".to_string(), |(v, _)| v.to_string())
            );
            let Some((version, url)) = pick_update(&running, channel, &releases) else {
                continue;
            };
            if notified.as_ref() == Some(&version) {
                continue;
            }

            let shown = window.update(cx, |_, window, cx| {
                let action = ToastAction::new(ui::t!("app.update.view_release"), move |_, cx| {
                    cx.open_url(&url);
                });
                toast::show_persistent(
                    ToastKind::Info,
                    ui::t!("app.update.available", version = version.to_string()),
                    Some(ToastBody {
                        lines: Vec::new(),
                        action: Some(action),
                    }),
                    window,
                    cx,
                );
            });
            if shown.is_err() {
                // The window is gone: nothing left to notify.
                return;
            }
            notified = Some(version);
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
