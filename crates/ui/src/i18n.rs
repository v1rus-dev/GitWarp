//! Runtime translation of user-visible text.
//!
//! # Catalogs
//! `crates/ui/locales/<lang>/<area>.json` hold flat `{"area.key": "text"}` objects for
//! `lang` in `en de fr es ru ja`. Keys are namespaced by area (`kit.*`, `format.*`,
//! `settings.*`, `app.*`, ...). Every file of a language is merged. All files are embedded
//! through the single [`CATALOGS`] list below; to add an area, create
//! `locales/<lang>/<area>.json` for every language (`{}` where untranslated) and append six
//! lines to [`CATALOGS`].
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

use gpui::SharedString;

/// Every embedded catalog file as `(language code, JSON text)`.
pub const CATALOGS: &[(&str, &str)] = &[
    ("en", include_str!("../locales/en/kit.json")),
    ("en", include_str!("../locales/en/format.json")),
    ("en", include_str!("../locales/en/settings.json")),
    ("en", include_str!("../locales/en/app.json")),
    ("de", include_str!("../locales/de/kit.json")),
    ("de", include_str!("../locales/de/format.json")),
    ("de", include_str!("../locales/de/settings.json")),
    ("de", include_str!("../locales/de/app.json")),
    ("fr", include_str!("../locales/fr/kit.json")),
    ("fr", include_str!("../locales/fr/format.json")),
    ("fr", include_str!("../locales/fr/settings.json")),
    ("fr", include_str!("../locales/fr/app.json")),
    ("es", include_str!("../locales/es/kit.json")),
    ("es", include_str!("../locales/es/format.json")),
    ("es", include_str!("../locales/es/settings.json")),
    ("es", include_str!("../locales/es/app.json")),
    ("ru", include_str!("../locales/ru/kit.json")),
    ("ru", include_str!("../locales/ru/format.json")),
    ("ru", include_str!("../locales/ru/settings.json")),
    ("ru", include_str!("../locales/ru/app.json")),
    ("ja", include_str!("../locales/ja/kit.json")),
    ("ja", include_str!("../locales/ja/format.json")),
    ("ja", include_str!("../locales/ja/settings.json")),
    ("ja", include_str!("../locales/ja/app.json")),
];

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

fn lookup(lang: usize, key: &str) -> Option<SharedString> {
    let all = catalogs();
    all[lang].get(key).or_else(|| all[0].get(key)).cloned()
}

/// Translates `key`; falls back to English, then to the key itself.
///
/// ```
/// let text = ui::i18n::t("kit.dialog.close"); // "Close"
/// ```
pub fn t(key: &str) -> SharedString {
    lookup(active(), key).unwrap_or_else(|| SharedString::from(key.to_owned()))
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
    let lang = active();
    let category = plural_category(LANGUAGES[lang], n);
    let template = lookup(lang, &format!("{key}.{category}"))
        .or_else(|| lookup(lang, &format!("{key}.other")))
        .unwrap_or_else(|| SharedString::from(key.to_owned()));
    let mut all: Vec<(&str, &dyn Display)> = vec![("n", &n)];
    all.extend_from_slice(args);
    substitute(&template, &all).into()
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
    fn unknown_key_returns_the_key() {
        assert_eq!(t("nope.missing"), "nope.missing");
    }
}
