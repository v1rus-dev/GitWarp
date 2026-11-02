//! Runtime translation of user-visible text.
//!
//! # Catalogs
//! `crates/ui/locales/<lang>/<area>.json` hold flat `{"area.key": "text"}` objects for
//! `lang` in `en de fr es ru ja`. Keys are namespaced by area (`kit.*`, `format.*`,
//! `settings.*`, `app.*`, ...). Every file of a language is merged. All files are embedded
//! through the single [`CATALOGS`] list below; to add an area, create
//! `locales/<lang>/<area>.json` for every language (`{}` where untranslated) and add its
//! name to the area list in the `embed_catalogs!` call that defines [`CATALOGS`].
//!
//! # Placeholders and plurals
//! `{name}` in a text is replaced by [`t_args`] / [`t_count`]. Plural keys carry a CLDR
//! suffix: `.one`, `.few`, `.many`, `.other`. A key is treated as plural when the English
//! catalog has `<key>.other`, so never end an ordinary key in `.one/.few/.many/.other`.
//! Required categories per language: en/de/es `one`+`other`, fr `one`+`other`,
//! ru `one`+`few`+`many`, ja `other`. Counts are integers, so Russian never needs `other`.
//!
//! # Call sites
//! Keys MUST be string literals at call sites (`t("kit.dialog.close")`,
//! `t_args("..", ..)`, `t_count("..", ..)`, `t!("..")`): the guard test in
//! `tests/i18n.rs` scans `crates/**/src` for them and checks each exists in English.
//!
//! # Lookup and state
//! Active language, then English, then the key itself (no logging, no panics).
//! The active language is a process-wide static set once by [`set_language`]; it defaults to
//! English, so tests need no setup. Results are [`SharedString`] clones of the parsed
//! catalog entries (cheap `Arc` clone).

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::{OnceLock, RwLock};

use gpui::prelude::*;
use gpui::{div, px, AnyElement, Div, SharedString};

/// Embeds `locales/<lang>/<area>.json` for every `lang` x `area` as `(lang, JSON text)`.
macro_rules! embed_catalogs {
    (@acc [$($acc:tt)*] [] $areas:tt) => {
        &[$($acc)*]
    };
    (@acc [$($acc:tt)*] [$lang:literal $($rest:literal)*] [$($area:literal)+]) => {
        embed_catalogs!(@acc [
            $($acc)*
            $( ($lang, include_str!(concat!("../locales/", $lang, "/", $area, ".json"))), )+
        ] [$($rest)*] [$($area)+])
    };
    ([$($lang:literal)+] $areas:tt) => {
        embed_catalogs!(@acc [] [$($lang)+] $areas)
    };
}

/// Every embedded catalog file as `(language code, JSON text)`.
pub const CATALOGS: &[(&str, &str)] = embed_catalogs!(
    ["en" "de" "fr" "es" "ru" "ja"]
    ["kit" "format" "settings" "app" "history" "changes" "repo" "sidebar" "palette"
     "activity" "welcome" "dialogs" "network" "worktrees"]
);

/// Supported language codes; `en` is the fallback.
pub const LANGUAGES: [&str; 6] = ["en", "de", "fr", "es", "ru", "ja"];

type Catalog = HashMap<String, SharedString>;

static ACTIVE: RwLock<usize> = RwLock::new(0);
static LOADED: OnceLock<Vec<Catalog>> = OnceLock::new();

fn lang_index(code: &str) -> usize {
    LANGUAGES.iter().position(|l| *l == code).unwrap_or(0)
}

fn catalogs() -> &'static [Catalog] {
    LOADED.get_or_init(|| {
        let mut all: Vec<Catalog> = LANGUAGES.iter().map(|_| Catalog::new()).collect();
        for (lang, json) in CATALOGS {
            let map: HashMap<String, String> = serde_json::from_str(json)
                .unwrap_or_else(|err| panic!("invalid {lang} catalog: {err}"));
            all[lang_index(lang)].extend(map.into_iter().map(|(k, v)| (k, v.into())));
        }
        all
    })
}

/// Selects the active language; call once at startup. An unknown code selects English.
///
/// ```
/// ui::i18n::set_language("de");
/// ui::i18n::set_language("xx"); // -> en
/// ```
pub fn set_language(code: &str) {
    *ACTIVE.write().unwrap_or_else(|e| e.into_inner()) = lang_index(code);
}

fn active() -> usize {
    *ACTIVE.read().unwrap_or_else(|e| e.into_inner())
}

/// The code of the active language (one of [`LANGUAGES`]).
pub fn language() -> &'static str {
    LANGUAGES[active()]
}

fn lookup(lang: usize, key: &str) -> Option<SharedString> {
    let all = catalogs();
    all[lang].get(key).or_else(|| all[0].get(key)).cloned()
}

/// [`lookup`], falling back to the key itself.
fn lookup_or_key(lang: usize, key: &str) -> SharedString {
    lookup(lang, key).unwrap_or_else(|| SharedString::from(key.to_owned()))
}

/// The plural template of `key` for `n` in `lang` (the `.other` form when the category's is
/// missing, the key itself when neither exists).
fn plural_template(lang: usize, key: &str, n: u64) -> SharedString {
    let category = plural_category(LANGUAGES[lang], n);
    lookup(lang, &format!("{key}.{category}"))
        .or_else(|| lookup(lang, &format!("{key}.other")))
        .unwrap_or_else(|| SharedString::from(key.to_owned()))
}

/// Translates `key`; falls back to English, then to the key itself.
///
/// ```
/// let text = ui::i18n::t("kit.dialog.close"); // "Close"
/// ```
pub fn t(key: &str) -> SharedString {
    lookup_or_key(active(), key)
}

/// Translates `key` into `lang` (a code from [`LANGUAGES`]; unknown means English) without
/// touching the active language. For measuring a text in another language.
pub fn t_in(lang: &str, key: &str) -> SharedString {
    lookup_or_key(lang_index(lang), key)
}

/// Translates `key` and replaces each `{name}` with the matching argument.
///
/// ```
/// let text = ui::i18n::t_args("format.example", &[("name", &"main")]);
/// ```
pub fn t_args(key: &str, args: &[(&str, &dyn Display)]) -> SharedString {
    substitute(&t(key), args).into()
}

/// Translates the plural form of `key` for `n` (`key.one`, `key.other`, ...); `{n}` is `n`
/// unless `args` provides its own `n` (use that to pass a separator-formatted count).
///
/// ```
/// let text = ui::i18n::t_count("format.days_ago", 3, &[]); // "3 days ago"
/// ```
pub fn t_count(key: &str, n: u64, args: &[(&str, &dyn Display)]) -> SharedString {
    let template = plural_template(active(), key, n);
    let mut all: Vec<(&str, &dyn Display)> = vec![("n", &n)];
    all.extend_from_slice(args);
    substitute(&template, &all).into()
}

/// One stretch of a translated sentence: literal text, or the position of a styled slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    Text(String),
    /// Index into the `slots` list passed to [`split_template`].
    Slot(usize),
}

/// Splits `template` at the `{name}` placeholders listed in `slots`. Other `{..}` text stays
/// literal. Lets a caller render one translated sentence with styled runs (mono, bold) while
/// the translator controls the word order.
pub fn split_template(template: &str, slots: &[&str]) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut rest = template;
    while let Some((at, index)) = slots
        .iter()
        .enumerate()
        .filter_map(|(index, name)| rest.find(&format!("{{{name}}}")).map(|at| (at, index)))
        .min_by_key(|(at, _)| *at)
    {
        if at > 0 {
            pieces.push(Piece::Text(rest[..at].to_owned()));
        }
        pieces.push(Piece::Slot(index));
        rest = &rest[at + slots[index].len() + 2..];
    }
    if !rest.is_empty() {
        pieces.push(Piece::Text(rest.to_owned()));
    }
    pieces
}

/// [`t_args`] for a sentence with styled slots: `args` are substituted, the `{name}` of each
/// entry in `slots` is left for [`split_template`].
pub fn t_pieces(key: &str, args: &[(&str, &dyn Display)], slots: &[&str]) -> Vec<Piece> {
    split_template(&substitute(&t(key), args), slots)
}

/// [`t_count`] for a sentence with styled slots (`"n"` may be a slot to style the count).
pub fn t_count_pieces(
    key: &str,
    n: u64,
    args: &[(&str, &dyn Display)],
    slots: &[&str],
) -> Vec<Piece> {
    let template = plural_template(active(), key, n);
    let mut all: Vec<(&str, &dyn Display)> = Vec::new();
    if !slots.contains(&"n") {
        all.push(("n", &n));
    }
    all.extend_from_slice(args);
    split_template(&substitute(&template, &all), slots)
}

/// Turns pieces into row children: text pieces and the `slots` elements in template order.
///
/// Spaces at a text piece's edge become a 4 px margin instead of depending on how a flex
/// item lays out edge whitespace; CJK text has none, so it joins its slot directly. The row
/// itself must have no `gap`.
pub fn pieces_children(pieces: Vec<Piece>, slots: Vec<AnyElement>) -> Vec<AnyElement> {
    let mut slots: Vec<Option<AnyElement>> = slots.into_iter().map(Some).collect();
    pieces
        .into_iter()
        .filter_map(|piece| match piece {
            Piece::Slot(index) => slots.get_mut(index).and_then(Option::take),
            Piece::Text(text) => {
                let trimmed = text.trim();
                let lead = text.starts_with(char::is_whitespace);
                let trail = text.ends_with(char::is_whitespace);
                Some(
                    div()
                        .flex_none()
                        .when(lead, |this| this.pl(px(4.)))
                        .when(trail && !trimmed.is_empty(), |this| this.pr(px(4.)))
                        .child(SharedString::from(trimmed.to_owned()))
                        .into_any_element(),
                )
            }
        })
        .collect()
}

/// One translated sentence as a row: [`pieces_children`] in a wrapping, vertically centred
/// flex row. Callers must not add a `gap` (the sentence supplies its own spaces).
pub fn sentence(pieces: Vec<Piece>, slots: Vec<AnyElement>) -> Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .children(pieces_children(pieces, slots))
}

/// `t!("key")` is [`t`]; `t!("key", name = value, ...)` is [`t_args`].
#[macro_export]
macro_rules! t {
    ($key:literal) => {
        $crate::i18n::t($key)
    };
    ($key:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::t_args($key, &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),+])
    };
}

/// The CLDR plural category (`one`, `few`, `many`, `other`) of `n` in `lang`.
pub fn plural_category(lang: &str, n: u64) -> &'static str {
    match lang {
        "ru" => {
            if n % 10 == 1 && n % 100 != 11 {
                "one"
            } else if (2..=4).contains(&(n % 10)) && !(12..=14).contains(&(n % 100)) {
                "few"
            } else {
                "many"
            }
        }
        "fr" => {
            if n <= 1 {
                "one"
            } else {
                "other"
            }
        }
        "ja" => "other",
        _ => {
            if n == 1 {
                "one"
            } else {
                "other"
            }
        }
    }
}

/// The plural categories every key of `lang` must provide.
pub fn plural_categories(lang: &str) -> &'static [&'static str] {
    match lang {
        "ru" => &["one", "few", "many"],
        "ja" => &["other"],
        _ => &["one", "other"],
    }
}

fn substitute(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let replaced = after.find('}').and_then(|close| {
            let name = &after[..close];
            // Later arguments win, so `t_count` callers can override `{n}`.
            let value = args.iter().rev().find(|(k, _)| *k == name)?.1;
            Some((value.to_string(), close))
        });
        match replaced {
            Some((value, close)) => {
                out.push_str(&value);
                rest = &after[close + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_named_placeholders() {
        let out = substitute("a {x} b {y} {z}", &[("x", &1), ("y", &"two")]);
        assert_eq!(out, "a 1 b two {z}");
    }

    #[test]
    fn split_template_orders_slots_by_position() {
        assert_eq!(
            split_template("a {y} b {x}{x}", &["x", "y"]),
            vec![
                Piece::Text("a ".into()),
                Piece::Slot(1),
                Piece::Text(" b ".into()),
                Piece::Slot(0),
                Piece::Slot(0),
            ]
        );
        assert_eq!(
            split_template("{z}", &["x"]),
            vec![Piece::Text("{z}".into())]
        );
    }

    #[test]
    fn unknown_key_returns_the_key() {
        assert_eq!(t("nope.missing"), "nope.missing");
    }
}
