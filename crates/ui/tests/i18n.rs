//! Catalog consistency and call-site guards for `ui::i18n`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use ui::i18n::{plural_categories, plural_category, CATALOGS, LANGUAGES};

const PLURAL_SUFFIXES: [&str; 4] = ["one", "few", "many", "other"];

fn locales_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("locales")
}

/// `lang -> area -> flat catalog`, read from disk and validated as flat string objects.
fn load_all() -> BTreeMap<String, BTreeMap<String, BTreeMap<String, String>>> {
    let mut all = BTreeMap::new();
    for lang in LANGUAGES {
        let dir = locales_dir().join(lang);
        let mut areas = BTreeMap::new();
        for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let text = fs::read_to_string(&path).unwrap();
            let value: serde_json::Value = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("{} is not valid JSON: {e}", path.display()));
            let object = value
                .as_object()
                .unwrap_or_else(|| panic!("{} is not a JSON object", path.display()));
            let mut map = BTreeMap::new();
            for (key, value) in object {
                let text = value
                    .as_str()
                    .unwrap_or_else(|| panic!("{}: {key} is not a string", path.display()));
                map.insert(key.clone(), text.to_owned());
            }
            let area = path.file_stem().unwrap().to_string_lossy().into_owned();
            areas.insert(area, map);
        }
        all.insert(lang.to_owned(), areas);
    }
    all
}

fn plural_base(key: &str) -> Option<&str> {
    let (base, suffix) = key.rsplit_once('.')?;
    PLURAL_SUFFIXES.contains(&suffix).then_some(base)
}

/// The keys a language must have, derived from the English key set.
fn expected_keys(en: &BTreeMap<String, String>, lang: &str) -> BTreeSet<String> {
    let plural_bases: BTreeSet<&str> = en
        .keys()
        .filter_map(|key| key.strip_suffix(".other"))
        .collect();
    let mut expected = BTreeSet::new();
    for key in en.keys() {
        match plural_base(key) {
            Some(base) if plural_bases.contains(base) => {
                for category in plural_categories(lang) {
                    expected.insert(format!("{base}.{category}"));
                }
            }
            _ => {
                expected.insert(key.clone());
            }
        }
    }
    expected
}

fn placeholders(text: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                found.insert(after[..close].to_owned());
                rest = &after[close + 1..];
            }
            None => break,
        }
    }
    found
}

#[test]
fn catalogs_are_valid_flat_json() {
    let all = load_all();
    assert_eq!(all.len(), LANGUAGES.len());
    for (lang, areas) in &all {
        for (area, map) in areas {
            for key in map.keys() {
                assert!(
                    key.starts_with(&format!("{area}.")),
                    "{lang}/{area}.json: key `{key}` is not namespaced by its area"
                );
            }
        }
    }
}

#[test]
fn every_catalog_file_is_embedded() {
    let all = load_all();
    for lang in LANGUAGES {
        let embedded = CATALOGS.iter().filter(|(l, _)| *l == lang).count();
        assert_eq!(
            embedded,
            all[lang].len(),
            "{lang}: CATALOGS in i18n.rs does not list every locales/{lang}/*.json file"
        );
    }
    let en_areas: BTreeSet<_> = all["en"].keys().collect();
    for lang in LANGUAGES {
        let areas: BTreeSet<_> = all[lang].keys().collect();
        assert_eq!(areas, en_areas, "{lang}: area files differ from en");
    }
}

#[test]
fn catalog_parity() {
    let all = load_all();
    for lang in LANGUAGES.iter().filter(|lang| **lang != "en") {
        for (area, en) in &all["en"] {
            let expected = expected_keys(en, lang);
            let actual: BTreeSet<String> = all[*lang][area].keys().cloned().collect();
            let missing: Vec<_> = expected.difference(&actual).collect();
            let extra: Vec<_> = actual.difference(&expected).collect();
            assert!(
                missing.is_empty() && extra.is_empty(),
                "{lang}/{area}.json: missing {missing:?}, extra {extra:?}"
            );
        }
    }
}

#[test]
fn placeholders_match_english() {
    let all = load_all();
    for lang in LANGUAGES.iter().filter(|lang| **lang != "en") {
        for (area, map) in &all[*lang] {
            for (key, text) in map {
                // A plural form is compared with the English `.other` (it always exists).
                let en_key = match plural_base(key) {
                    Some(base) => format!("{base}.other"),
                    None => key.clone(),
                };
                let Some(en_text) = all["en"][area].get(&en_key) else {
                    continue; // reported by catalog_parity
                };
                let mut want = placeholders(en_text);
                let mut got = placeholders(text);
                // Plural forms may spell the number out ("one", "1") instead of using {n}.
                if plural_base(key).is_some() {
                    want.remove("n");
                    got.remove("n");
                }
                assert_eq!(got, want, "{lang}: placeholders of `{key}` differ from en");
            }
        }
    }
}

#[test]
fn plural_rules() {
    for n in [1, 21, 31, 101] {
        assert_eq!(plural_category("ru", n), "one", "ru {n}");
    }
    for n in [2, 3, 4, 22, 23, 24, 102] {
        assert_eq!(plural_category("ru", n), "few", "ru {n}");
    }
    for n in [0, 5, 11, 12, 14, 20, 25, 111, 112] {
        assert_eq!(plural_category("ru", n), "many", "ru {n}");
    }
    assert_eq!(plural_category("fr", 0), "one");
    assert_eq!(plural_category("fr", 1), "one");
    assert_eq!(plural_category("fr", 2), "other");
    for n in [0, 1, 2, 100] {
        assert_eq!(plural_category("ja", n), "other");
    }
    for lang in ["en", "de", "es"] {
        assert_eq!(plural_category(lang, 1), "one");
        assert_eq!(plural_category(lang, 0), "other");
        assert_eq!(plural_category(lang, 2), "other");
    }
}

#[test]
fn english_lookup_and_plurals() {
    assert_eq!(ui::i18n::t("kit.dialog.close"), "Close");
    assert_eq!(ui::i18n::t("no.such.key"), "no.such.key");
    assert_eq!(ui::i18n::t_count("format.weeks_ago", 1, &[]), "1 week ago");
    assert_eq!(ui::i18n::t_count("format.weeks_ago", 3, &[]), "3 weeks ago");
    assert_eq!(
        ui::i18n::t_count("kit.heatmap.commits", 5, &[("date", &"today")]),
        "5 commits on today"
    );
    assert_eq!(
        ui::i18n::t_count(
            "kit.heatmap.commits",
            5,
            &[("date", &"today"), ("n", &"5,000")]
        ),
        "5,000 commits on today"
    );
    assert_eq!(
        ui::t!("settings.git.too_old", found = "2.1", required = "2.31"),
        "git 2.1 is too old; GitWarp needs 2.31 or newer"
    );
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn is_key(text: &str) -> bool {
    text.contains('.')
        && !text.starts_with('.')
        && !text.ends_with('.')
        && text
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
}

/// Every key literal passed to `t(..)`, `t_args(..)`, `t_count(..)` or `t!(..)` in
/// `crates/**/src`, plus every string literal that looks like a key of a known area
/// (catalog keys kept in consts and tables).
#[test]
fn keys_used_in_source_exist_in_english() {
    let all = load_all();
    let en: BTreeSet<&String> = all["en"].values().flat_map(|map| map.keys()).collect();
    let areas: Vec<&String> = all["en"].keys().collect();
    let exists = |key: &str| en.contains(&key.to_owned()) || en.contains(&format!("{key}.other"));

    let crates_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for entry in fs::read_dir(&crates_dir).unwrap() {
        let src = entry.unwrap().path().join("src");
        if src.is_dir() {
            rust_files(&src, &mut files);
        }
    }
    assert!(!files.is_empty());

    let mut missing = Vec::new();
    let mut checked = 0;
    for file in files {
        if file.ends_with("ui/src/i18n.rs") {
            continue; // documentation examples
        }
        let text = fs::read_to_string(&file).unwrap();
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] != b'"' {
                i += 1;
                continue;
            }
            // Read the literal (skipping escapes); raw strings are not used for keys.
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != b'"' {
                j += if bytes[j] == b'\\' { 2 } else { 1 };
            }
            let literal = &text[start..j.min(text.len())];
            i = j + 1;
            let in_known_area = areas
                .iter()
                .any(|area| literal.starts_with(&format!("{area}.")));
            if is_key(literal) && in_known_area {
                checked += 1;
                // File names such as `settings.json.bak` share the `settings.` prefix.
                let is_file = [".json", ".bak", ".toml"]
                    .iter()
                    .any(|ext| literal.contains(ext));
                if !exists(literal) && !is_file {
                    missing.push(format!("{}: {literal}", file.display()));
                }
            }
        }
    }
    assert!(checked > 50, "the scan found only {checked} keys");
    assert!(
        missing.is_empty(),
        "keys missing from en:\n{}",
        missing.join("\n")
    );
}
