//! Avatars with the design's deterministic fallback colours.

use gpui::{
    div, hsla, white, App, FontWeight, IntoElement, ParentElement, Pixels, RenderOnce,
    SharedString, Styled, Window,
};

/// A round avatar showing the author's initials.
#[derive(IntoElement)]
pub struct Avatar {
    email: SharedString,
    size: Pixels,
    name: Option<SharedString>,
}

impl Avatar {
    pub fn new(email: impl Into<SharedString>, size: Pixels) -> Self {
        Self {
            email: email.into(),
            size,
            name: None,
        }
    }

    /// Use the author's name for the initials; the colour still comes from the
    /// e-mail.
    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = Some(name.into());
        self
    }
}

impl RenderOnce for Avatar {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div()
            .w(self.size)
            .h(self.size)
            .flex_none()
            .rounded(self.size / 2.)
            .flex()
            .items_center()
            .justify_center()
            .bg(fallback_color(&self.email))
            .text_color(white())
            .text_size(self.size * 0.42)
            .font_weight(FontWeight::SEMIBOLD)
            .child(initials_for(self.name.as_deref(), &self.email))
    }
}

/// `h = h * 31 + char` over the lowercased email (wrapping u32).
pub fn hash_email(email: &str) -> u32 {
    email.to_lowercase().chars().fold(0u32, |hash, ch| {
        hash.wrapping_mul(31).wrapping_add(ch as u32)
    })
}

/// The design's initials: the first letter of the author's first and last
/// words, upper-cased ("Mira Okonkwo" → "MO"). A single word gives one letter.
///
/// Names without a letter (and a missing name) fall back to the e-mail rule of
/// [`initials`].
pub fn initials_for(name: Option<&str>, email: &str) -> SharedString {
    let Some(name) = name.filter(|name| name.chars().any(char::is_alphabetic)) else {
        return initials(email);
    };
    let words: Vec<&str> = name
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    let first = words.first().and_then(|word| word.chars().next());
    let last = match words.len() {
        0 | 1 => None,
        _ => words.last().and_then(|word| word.chars().next()),
    };
    let text: String = first
        .into_iter()
        .chain(last)
        .flat_map(|ch| ch.to_uppercase())
        .collect();
    if text.is_empty() {
        initials(email)
    } else {
        text.into()
    }
}

/// Up to two initials from the local part of the address.
pub fn initials(email: &str) -> SharedString {
    let local = email.split('@').next().unwrap_or(email);
    let mut parts = local
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|part| !part.is_empty());
    let first = parts.next();
    let second = parts.next();
    let text: String = first
        .into_iter()
        .chain(second)
        .filter_map(|part| part.chars().next())
        .flat_map(|ch| ch.to_uppercase())
        .collect();
    if text.is_empty() {
        "?".into()
    } else {
        text.into()
    }
}

fn fallback_color(email: &str) -> gpui::Hsla {
    let hue = (hash_email(email) % 360) as f32 / 360.;
    hsla(hue, 0.40, 0.34, 1.)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_matches_the_design_formula() {
        assert_eq!(hash_email("mira@example.com"), 1_393_384_200);
        assert_eq!(hash_email("MIRA@EXAMPLE.COM"), 1_393_384_200);
    }

    #[test]
    fn initials_come_from_the_local_part() {
        assert_eq!(initials("mira@example.com"), "M");
        assert_eq!(initials("john.doe@example.com"), "JD");
        assert_eq!(initials("feat/lane@example.com"), "FL");
        assert_eq!(initials("@example.com"), "?");
    }

    #[test]
    fn initials_come_from_the_author_name() {
        assert_eq!(initials_for(Some("Mira Okonkwo"), "mira@example.com"), "MO");
        assert_eq!(initials_for(Some("Lin Zhao"), "lin@example.com"), "LZ");
        assert_eq!(initials_for(Some("cher"), "cher@example.com"), "C");
        // Extra whitespace and separators do not invent words.
        assert_eq!(
            initials_for(Some("  Mira   Okonkwo "), "x@example.com"),
            "MO"
        );
        // A name without letters falls back to the e-mail rule.
        assert_eq!(initials_for(Some("123"), "john.doe@example.com"), "JD");
        assert_eq!(initials_for(None, "john.doe@example.com"), "JD");
    }
}
