//! Percent-encoding for URL path segments and query values.

/// Encodes one path segment: everything except RFC 3986 unreserved
/// characters becomes `%XX` over the UTF-8 bytes.
pub(crate) fn encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Encodes a `/`-separated name (branch, tag, file path): each segment is
/// encoded, the separating `/` stays.
pub(crate) fn encode_path(s: &str) -> String {
    s.split('/')
        .map(encode_segment)
        .collect::<Vec<_>>()
        .join("/")
}
