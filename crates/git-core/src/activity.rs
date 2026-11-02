//! Commit activity per calendar day, for the heatmap.
//!
//! Counts come from one streamed `git log --all`; git itself buckets the
//! author date by the LOCAL calendar day (`--date=format-local`), so this
//! module carries no timezone code. Everything here is synchronous: callers
//! run it on a background executor.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::Stdio;

use crate::{decode_bytes, Error, Repo, Result};

/// Commits on one local calendar day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DayCount {
    /// Days since the Unix epoch of the local calendar date.
    pub day: i32,
    /// Commits whose author email is the requested one.
    pub mine: u32,
    /// All commits.
    pub all: u32,
}

/// Per-day commit counts over a time range.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Activity {
    /// Sorted by day; only days that have commits.
    pub days: Vec<DayCount>,
    /// The email `mine` was counted for.
    pub me: Option<String>,
}

const MIN_GIT_TIMESTAMP: i64 = 100_000_000;

/// The repository's effective `user.email` (local, then global).
pub fn my_email(repo: &Repo) -> Result<Option<String>> {
    let out = repo
        .command()
        .args(["config", "--get", "user.email"])
        .stdin(Stdio::null())
        .output()
        .map_err(Error::Spawn)?;
    if out.status.success() {
        let email = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        return Ok((!email.is_empty()).then_some(email));
    }
    // `--get` exits 1 when the key is absent; anything else is a real failure.
    match out.status.code() {
        Some(1) => Ok(None),
        code => Err(Error::Git {
            command: "config",
            code,
            stderr: decode_bytes(out.stderr),
        }),
    }
}

/// Counts commits per local calendar day between two Unix timestamps.
///
/// Merges count too, like GitHub. `--since`/`--until` filter on the committer
/// date while the day bucket is the author date, exactly as in `git log`; the
/// two differ only for rebased or amended commits near the range edges. Git
/// also stops walking a branch at a commit older than `since`, so a history
/// whose dates go backwards by more than a few commits can lose older-looking
/// ones near the lower bound.
pub fn activity(
    repo: &Repo,
    since_unix: i64,
    until_unix: i64,
    me: Option<&str>,
) -> Result<Activity> {
    activity_in(repo, since_unix, until_unix, me, None)
}

/// [`activity`] with an optional `TZ` override for git, so tests can pin the
/// local timezone without touching the process environment.
pub(crate) fn activity_in(
    repo: &Repo,
    since_unix: i64,
    until_unix: i64,
    me: Option<&str>,
    tz: Option<&str>,
) -> Result<Activity> {
    // Git's date parser only takes `@<n>` as a timestamp from 100_000_000 (1973)
    // up; a smaller number matches nothing at all, so clamp rather than return
    // an empty history for `since = 0`.
    let since_unix = since_unix.max(MIN_GIT_TIMESTAMP);
    let until_unix = until_unix.max(MIN_GIT_TIMESTAMP);
    let mut cmd = repo.command();
    // Stash entries are commits under refs/stash; they are not authored work.
    cmd.arg("log")
        .arg("--exclude=refs/stash")
        .arg("--all")
        .arg(format!("--since=@{since_unix}"))
        .arg(format!("--until=@{until_unix}"))
        .arg("--date=format-local:%Y-%m-%d")
        .arg("--format=%ad%x09%aE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(tz) = tz {
        cmd.env("TZ", tz);
    }
    let mut child = cmd.spawn().map_err(Error::Spawn)?;
    let stdout = child.stdout.take().expect("stdout is piped");

    let me_lower = me.map(str::to_lowercase);
    let mut counts: HashMap<i32, (u32, u32)> = HashMap::new();
    let mut reader = BufReader::new(stdout);
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line).map_err(Error::Io)? == 0 {
            break;
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end();
        let Some((date, email)) = text.split_once('\t') else {
            continue;
        };
        let Some(day) = parse_day(date) else {
            continue;
        };
        let entry = counts.entry(day).or_default();
        entry.1 += 1;
        if me_lower.as_deref() == Some(email.to_lowercase().as_str()) {
            entry.0 += 1;
        }
    }
    drop(reader);
    let out = child.wait_with_output().map_err(Error::Io)?;
    if !out.status.success() {
        return Err(Error::Git {
            command: "log",
            code: out.status.code(),
            stderr: decode_bytes(out.stderr),
        });
    }

    let mut days: Vec<DayCount> = counts
        .into_iter()
        .map(|(day, (mine, all))| DayCount { day, mine, all })
        .collect();
    days.sort_by_key(|d| d.day);
    Ok(Activity {
        days,
        me: me.map(str::to_owned),
    })
}

/// `YYYY-MM-DD` to days since the Unix epoch.
fn parse_day(date: &str) -> Option<i32> {
    let mut parts = date.splitn(3, '-');
    let year: i32 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month, day))
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Hinnant's algorithm).
fn days_from_civil(year: i32, month: u32, day: u32) -> i32 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400); // [0, 399]
    let mp = (month as i32 + 9) % 12; // March = 0
    let doy = (153 * mp + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Heat thresholds for the non-zero counts in view: a count `c > 0` is level
/// `i + 1` for the first `i` with `c <= thresholds[i]`, and level 4 above all.
///
/// Normally the quartiles (25/50/75/100 %) of the non-zero counts; with at
/// most four distinct non-zero values the scale is linear, `ceil(4 * n / max)`,
/// so a repository with one commit a day is not drawn as all-hot. No non-zero
/// count gives `[0; 4]`.
pub fn levels(counts: &[u32]) -> [u32; 4] {
    let mut nonzero: Vec<u32> = counts.iter().copied().filter(|&c| c > 0).collect();
    nonzero.sort_unstable();
    let Some(&max) = nonzero.last() else {
        return [0; 4];
    };
    let mut distinct = nonzero.clone();
    distinct.dedup();
    if distinct.len() <= 4 {
        // `ceil(4c / max) <= l` is `c <= floor(l * max / 4)`.
        return [max / 4, max / 2, 3 * max / 4, max];
    }
    let len = nonzero.len();
    // Nearest-rank percentile: the value at index `ceil(p * len) - 1`.
    let at = |rank: usize| nonzero[rank.min(len) - 1];
    [
        at(len.div_ceil(4)),
        at(len.div_ceil(2)),
        at((3 * len).div_ceil(4)),
        max,
    ]
}

/// The heat level of `count`: 0 for no commits, otherwise 1 to 4.
pub fn level(count: u32, thresholds: &[u32; 4]) -> u8 {
    if count == 0 {
        return 0;
    }
    match thresholds[..3].iter().position(|&t| count <= t) {
        Some(index) => index as u8 + 1,
        None => 4,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;
    use std::sync::Arc;

    use super::*;
    use crate::{Git, GitChoice};

    #[test]
    fn civil_dates_convert_to_epoch_days() {
        assert_eq!(parse_day("1970-01-01"), Some(0));
        assert_eq!(parse_day("1970-01-02"), Some(1));
        assert_eq!(parse_day("1969-12-31"), Some(-1));
        assert_eq!(parse_day("2000-03-01"), Some(11_017));
        assert_eq!(parse_day("2026-10-14"), Some(20_740));
        assert_eq!(parse_day("2024-02-29"), Some(19_782));
        assert_eq!(parse_day("nonsense"), None);
        assert_eq!(parse_day("2026-13-01"), None);
    }

    #[test]
    fn quartile_levels_use_the_nonzero_counts() {
        // 8 distinct non-zero values; zeros are ignored.
        let counts = [0, 1, 2, 3, 4, 5, 6, 7, 8, 0, 0];
        let t = levels(&counts);
        assert_eq!(t, [2, 4, 6, 8]);
        let got: Vec<u8> = (0..=9).map(|c| level(c, &t)).collect();
        assert_eq!(got, [0, 1, 1, 2, 2, 3, 3, 4, 4, 4]);
    }

    #[test]
    fn few_distinct_values_scale_linearly() {
        // 1, 2, 4 are three distinct values: ceil(4 * n / 4).
        let t = levels(&[1, 1, 1, 2, 2, 4]);
        assert_eq!(t, [1, 2, 3, 4]);
        assert_eq!([1, 2, 3, 4].map(|c| level(c, &t)), [1, 2, 3, 4]);
        // A repository with one commit a day is level 4 everywhere... and 1 is
        // the maximum, so that is the linear rule, not a bug.
        let t = levels(&[1, 1, 1]);
        assert_eq!(level(1, &t), 4);
        // max 10, values 1 and 10: ceil(4 * 1 / 10) = 1.
        let t = levels(&[1, 10]);
        assert_eq!((level(1, &t), level(10, &t)), (1, 4));
        assert_eq!(level(5, &t), 2);
        assert_eq!(level(6, &t), 3);
    }

    #[test]
    fn zero_is_level_zero_and_empty_input_has_no_thresholds() {
        assert_eq!(level(0, &[1, 2, 3, 4]), 0);
        assert_eq!(levels(&[]), [0; 4]);
        assert_eq!(levels(&[0, 0]), [0; 4]);
        assert_eq!(level(0, &levels(&[])), 0);
    }

    fn git(dir: &Path, args: &[&str], date: Option<&str>, email: &str) {
        let mut cmd = Command::new("git");
        cmd.arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=Test",
                "-c",
                &format!("user.email={email}"),
                "-c",
                "commit.gpgsign=false",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1");
        if let Some(date) = date {
            cmd.env("GIT_AUTHOR_DATE", date)
                .env("GIT_COMMITTER_DATE", date);
        }
        assert!(cmd.status().expect("git is installed").success());
    }

    fn open(path: &Path) -> Repo {
        Repo::open(Arc::new(Git::resolve(GitChoice::System).unwrap().git), path).unwrap()
    }

    #[test]
    fn buckets_by_the_local_day_across_midnight() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        git(d, &["init", "-q"], None, "a@x.io");
        // 20:30Z and 22:30Z on 10 March: with TZ = UTC+3 (POSIX "XXX-3") they
        // are 23:30 on the 10th and 01:30 on the 11th.
        git(
            d,
            &["commit", "-q", "--allow-empty", "-m", "a"],
            Some("2026-03-10T20:30:00Z"),
            "a@x.io",
        );
        git(
            d,
            &["commit", "-q", "--allow-empty", "-m", "b"],
            Some("2026-03-10T22:30:00Z"),
            "b@x.io",
        );
        let repo = open(d);
        let act = activity_in(&repo, 0, 4_000_000_000, Some("A@X.io"), Some("XXX-3")).unwrap();
        let tenth = parse_day("2026-03-10").unwrap();
        assert_eq!(
            act.days,
            vec![
                DayCount {
                    day: tenth,
                    mine: 1,
                    all: 1
                },
                DayCount {
                    day: tenth + 1,
                    mine: 0,
                    all: 1
                },
            ]
        );
        // In UTC both land on the 10th.
        let act = activity_in(&repo, 0, 4_000_000_000, None, Some("UTC0")).unwrap();
        assert_eq!(
            act.days,
            vec![DayCount {
                day: tenth,
                mine: 0,
                all: 2
            }]
        );
        assert_eq!(act.me, None);
    }
}
