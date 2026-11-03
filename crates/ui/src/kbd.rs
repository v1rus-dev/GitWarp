//! Platform notation for shortcut labels.
//!
//! Bindings written as `secondary-…` are Cmd on macOS and Ctrl elsewhere, but
//! the labels are authored as `Ctrl+…`. [`platform_label`] renders such a
//! label in Mac notation on macOS and leaves it untouched elsewhere. Labels of
//! bindings that name a literal Ctrl (`ctrl-alt-w`) must not go through it.

use gpui::SharedString;

/// The label of a `secondary-` binding as the current platform writes it.
pub fn platform_label(label: &str) -> SharedString {
    convert(label, cfg!(target_os = "macos")).into()
}

/// `platform_label` with the platform as a parameter, so it is testable on any OS.
///
/// On macOS `Ctrl` (the `secondary` key) becomes `⌘`, `Shift` `⇧`, `Alt` `⌥`,
/// in Apple's order (⌥ ⇧ ⌘, then the key) without separators. Labels without a
/// `Ctrl` part (`Enter`, `Alt+1–9`, `›`) are returned unchanged.
fn convert(label: &str, mac: bool) -> String {
    if !mac {
        return label.to_owned();
    }
    let mut parts: Vec<&str> = label.split('+').collect();
    let Some(key) = parts.pop().filter(|key| !key.is_empty()) else {
        return label.to_owned();
    };
    if !parts.contains(&"Ctrl")
        || parts
            .iter()
            .any(|p| !matches!(*p, "Ctrl" | "Shift" | "Alt"))
    {
        return label.to_owned();
    }
    let mut out = String::new();
    if parts.contains(&"Alt") {
        out.push('⌥');
    }
    if parts.contains(&"Shift") {
        out.push('⇧');
    }
    out.push('⌘');
    match key {
        "Enter" => out.push('↩'),
        key if key.chars().count() == 1 => out.push_str(&key.to_uppercase()),
        key => out.push_str(key),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::convert;

    #[test]
    fn mac_notation() {
        assert_eq!(convert("Ctrl+O", true), "⌘O");
        assert_eq!(convert("Ctrl+Shift+O", true), "⇧⌘O");
        assert_eq!(convert("Ctrl+,", true), "⌘,");
        assert_eq!(convert("Ctrl+Enter", true), "⌘↩");
        assert_eq!(convert("Ctrl+Shift+F", true), "⇧⌘F");
        assert_eq!(convert("Ctrl+Alt+W", true), "⌥⌘W");
    }

    #[test]
    fn labels_without_ctrl_are_unchanged() {
        for label in ["Enter", "Del", "Alt+1–9", "›", "HEAD", "J"] {
            assert_eq!(convert(label, true), label);
        }
    }

    #[test]
    fn other_platforms_are_unchanged() {
        for label in ["Ctrl+O", "Ctrl+Shift+O", "Ctrl+,", "Ctrl+Enter", "Enter"] {
            assert_eq!(convert(label, false), label);
        }
    }
}
