//! Small URL-splitting helpers shared by instance and remote parsing.

/// Splits `scheme://rest`; without a scheme returns `(None, input)`.
pub(crate) fn split_scheme(s: &str) -> (Option<&str>, &str) {
    match s.split_once("://") {
        Some((scheme, rest)) => (Some(scheme), rest),
        None => (None, s),
    }
}

/// Splits `authority/path` at the first `/`; the path keeps its leading `/`.
pub(crate) fn split_host_path(rest: &str) -> (&str, &str) {
    match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    }
}

/// Parses `[user@]host[:port]` into a lowercase host and an optional port.
pub(crate) fn parse_authority(authority: &str) -> Option<(String, Option<u16>)> {
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let (host, port) = match host_port.rsplit_once(':') {
        Some((h, p)) => (h, Some(p.parse::<u16>().ok()?)),
        None => (host_port, None),
    };
    valid_host(host).then(|| (host.to_ascii_lowercase(), port))
}

fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && !host.starts_with('.')
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
}
