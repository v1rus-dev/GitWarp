//! Small formatting helpers shared by the views: counts, relative times and
//! `~`-shortened paths.

use std::path::{Path, PathBuf};

/// Group `count` with thousands separators.
pub fn format_count(count: usize) -> String {
    let digits = count.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// How long ago `then` was, for the recent repositories list.
///
/// `now` and `then` are unix seconds. A timestamp in the future (a clock that
/// went backwards) reads as `just now` instead of a negative age.
pub fn relative_time(now: u64, then: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    const WEEK: u64 = 7 * DAY;
    const MONTH: u64 = 30 * DAY;
    const YEAR: u64 = 365 * DAY;

    let age = now.saturating_sub(then);
    if age < MINUTE {
        "just now".to_owned()
    } else if age < HOUR {
        format!("{} min ago", age / MINUTE)
    } else if age < DAY {
        format!("{} h ago", age / HOUR)
    } else if age < 2 * DAY {
        "yesterday".to_owned()
    } else if age < WEEK {
        format!("{} d ago", age / DAY)
    } else if age < MONTH {
        let weeks = age / WEEK;
        if weeks == 1 {
            "1 week ago".to_owned()
        } else {
            format!("{weeks} weeks ago")
        }
    } else if age < YEAR {
        format!("{} mo ago", age / MONTH)
    } else {
        format!("{} y ago", age / YEAR)
    }
}

/// `YYYY-MM-DD HH:MM` in UTC for a unix timestamp.
///
/// The details pane uses the same value the design prototype shows; no
/// external date crate is needed for it.
pub fn absolute_time(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let days = seconds / 86_400;
    let rest = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        rest / 3_600,
        (rest % 3_600) / 60
    )
}

/// Civil date for days since 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (
        if month <= 2 { year + 1 } else { year },
        month as u32,
        day as u32,
    )
}

/// The user's home directory, from the environment.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// `path` with the home directory shortened to `~`.
pub fn display_path(path: &Path, home: Option<&Path>) -> String {
    let Some(home) = home else {
        return path.display().to_string();
    };
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Expand a leading `~` in a path typed by the user.
///
/// Only `~` alone and `~/…` are expanded; `~user` is left as typed.
pub fn expand_home(input: &str, home: Option<&Path>) -> PathBuf {
    let Some(home) = home else {
        return PathBuf::from(input);
    };
    if input == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = input
        .strip_prefix("~/")
        .or_else(|| input.strip_prefix("~\\"))
    {
        return home.join(rest);
    }
    PathBuf::from(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_thousands() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1_000), "1,000");
        assert_eq!(format_count(12_400), "12,400");
        assert_eq!(format_count(1_234_567), "1,234,567");
    }

    #[test]
    fn absolute_time_is_utc_yyyy_mm_dd_hh_mm() {
        assert_eq!(absolute_time(0), "1970-01-01 00:00");
        assert_eq!(absolute_time(1_793_282_880), "2026-10-29 14:08");
        assert_eq!(absolute_time(1_700_000_000), "2023-11-14 22:13");
        assert_eq!(absolute_time(-5), "1970-01-01 00:00");
    }

    #[test]
    fn relative_time_hits_every_boundary() {
        let now = 2_000_000_000;
        let ago = |seconds: u64| relative_time(now, now - seconds);

        assert_eq!(ago(0), "just now");
        assert_eq!(ago(59), "just now");
        assert_eq!(ago(60), "1 min ago");
        assert_eq!(ago(3_599), "59 min ago");
        assert_eq!(ago(3_600), "1 h ago");
        assert_eq!(ago(86_399), "23 h ago");
        assert_eq!(ago(86_400), "yesterday");
        assert_eq!(ago(172_799), "yesterday");
        assert_eq!(ago(172_800), "2 d ago");
        assert_eq!(ago(604_799), "6 d ago");
        assert_eq!(ago(604_800), "1 week ago");
        assert_eq!(ago(1_209_600), "2 weeks ago");
        assert_eq!(ago(2_591_999), "4 weeks ago");
        assert_eq!(ago(2_592_000), "1 mo ago");
        assert_eq!(ago(31_535_999), "12 mo ago");
        assert_eq!(ago(31_536_000), "1 y ago");
        assert_eq!(ago(63_072_000), "2 y ago");
    }

    #[test]
    fn a_future_timestamp_is_just_now() {
        assert_eq!(relative_time(1_000, 2_000), "just now");
    }

    #[test]
    fn display_path_shortens_only_the_home_directory() {
        let home = Path::new("/home/user");
        assert_eq!(display_path(Path::new("/home/user"), Some(home)), "~");
        assert_eq!(
            display_path(Path::new("/home/user/dev/gitwarp"), Some(home)),
            format!(
                "~{}dev{}gitwarp",
                std::path::MAIN_SEPARATOR,
                std::path::MAIN_SEPARATOR
            )
        );
        assert_eq!(
            display_path(Path::new("/home/user2/x"), Some(home)),
            "/home/user2/x"
        );
        assert_eq!(
            display_path(Path::new("/mnt/data"), Some(home)),
            "/mnt/data"
        );
        assert_eq!(display_path(Path::new("/mnt/data"), None), "/mnt/data");
    }

    #[test]
    fn expand_home_handles_tilde_forms() {
        let home = Path::new("/home/user");
        assert_eq!(expand_home("~", Some(home)), PathBuf::from("/home/user"));
        assert_eq!(
            expand_home("~/dev/x", Some(home)),
            PathBuf::from("/home/user").join("dev").join("x")
        );
        assert_eq!(expand_home("/abs/x", Some(home)), PathBuf::from("/abs/x"));
        assert_eq!(
            expand_home("~other/x", Some(home)),
            PathBuf::from("~other/x")
        );
        assert_eq!(
            expand_home("relative", Some(home)),
            PathBuf::from("relative")
        );
        assert_eq!(expand_home("~/dev", None), PathBuf::from("~/dev"));
    }
}
