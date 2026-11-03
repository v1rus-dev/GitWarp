//! Syntax highlighting for the diff view, on top of `tree-sitter-highlight`.
//!
//! The highlighter runs on the background executor over the whole old and new
//! blob text, then the spans are split per source line so a diff row only
//! looks at its own line. An unknown language, a missing grammar or any
//! highlighter error keeps the text plain: highlighting is never a reason to
//! fail a diff.

use std::collections::HashMap;
use std::ops::Range;

use git_core::FileTexts;
use tree_sitter::Language;
#[cfg(test)]
use tree_sitter::Parser;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

/// The six syntax colours the theme provides; everything else keeps the
/// default text colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token {
    Keyword,
    String,
    Number,
    Comment,
    Type,
    Function,
}

/// The capture names the configuration recognizes and the token each paints.
///
/// Grammars disagree on names (`number` here, `constant.numeric` there), so
/// the list carries aliases; `configure` picks the longest name whose parts
/// all appear in a capture, and the index selects the token from this list.
const RECOGNIZED: [(&str, Token); 22] = [
    ("keyword", Token::Keyword),
    ("keyword.control", Token::Keyword),
    ("conditional", Token::Keyword),
    ("repeat", Token::Keyword),
    ("include", Token::Keyword),
    ("exception", Token::Keyword),
    ("storage", Token::Keyword),
    ("boolean", Token::Keyword),
    ("variable.builtin", Token::Keyword),
    ("string", Token::String),
    ("character", Token::String),
    ("escape", Token::String),
    ("regexp", Token::String),
    ("number", Token::Number),
    ("numeric", Token::Number),
    ("integer", Token::Number),
    ("float", Token::Number),
    ("constant.numeric", Token::Number),
    ("constant.builtin", Token::Number),
    ("type", Token::Type),
    ("function", Token::Function),
    ("comment", Token::Comment),
];

const RECOGNIZED_NAMES: [&str; RECOGNIZED.len()] = {
    let mut names = [""; RECOGNIZED.len()];
    let mut index = 0;
    while index < RECOGNIZED.len() {
        names[index] = RECOGNIZED[index].0;
        index += 1;
    }
    names
};

impl Token {
    fn from_index(index: usize) -> Option<Self> {
        RECOGNIZED.get(index).map(|(_, token)| *token)
    }

    /// `constant.builtin` covers both numbers and booleans in several grammars;
    /// an all-alphabetic word there (`true`, `false`, `None`) is a keyword.
    fn for_text(self, text: &str) -> Self {
        if self == Self::Number && !text.is_empty() && text.chars().all(char::is_alphabetic) {
            Self::Keyword
        } else {
            self
        }
    }
}

/// One highlighted byte range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightSpan {
    pub range: Range<usize>,
    pub token: Token,
}

/// A file's syntax spans, grouped by 1-based source line.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Highlighted {
    pub old: HashMap<u32, Vec<HighlightSpan>>,
    pub new: HashMap<u32, Vec<HighlightSpan>>,
    /// The grammar's name, for tests and debugging; `None` when plain.
    pub language: Option<&'static str>,
}

impl Highlighted {
    /// The spans of one side's line; empty when nothing was highlighted.
    pub fn spans(&self, old: bool, line: Option<u32>) -> &[HighlightSpan] {
        let Some(line) = line else {
            return &[];
        };
        let map = if old { &self.old } else { &self.new };
        map.get(&line).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// Highlights the old and new text of one file.
pub fn highlight(path: &str, texts: &FileTexts) -> Highlighted {
    if texts.binary {
        return Highlighted::default();
    }
    let Some(language) = language_for(path) else {
        return Highlighted::default();
    };
    let Some(mut config) = language.config() else {
        log::warn!(
            "could not build the {} syntax configuration",
            language.name()
        );
        return Highlighted::default();
    };
    let old = texts
        .old
        .as_deref()
        .map(|text| highlight_text(&mut config, text))
        .unwrap_or_default();
    let new = texts
        .new
        .as_deref()
        .map(|text| highlight_text(&mut config, text))
        .unwrap_or_default();
    Highlighted {
        old,
        new,
        language: Some(language.name()),
    }
}

/// The grammar for a path's extension, if one is compiled in.
pub fn language_for(path: &str) -> Option<LanguageKind> {
    let name = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_ascii_lowercase();
    if matches!(
        name.as_str(),
        ".bashrc" | ".bash_profile" | ".bash_aliases" | ".zshrc" | ".profile"
    ) {
        return Some(LanguageKind::Bash);
    }
    let (_, extension) = name.rsplit_once('.')?;
    Some(match extension {
        "rs" => LanguageKind::Rust,
        "ts" | "mts" | "cts" => LanguageKind::TypeScript,
        "tsx" => LanguageKind::Tsx,
        "js" | "mjs" | "cjs" | "jsx" => LanguageKind::JavaScript,
        "py" | "pyi" => LanguageKind::Python,
        "go" => LanguageKind::Go,
        "java" => LanguageKind::Java,
        "kt" | "kts" => LanguageKind::Kotlin,
        "c" | "h" => LanguageKind::C,
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => LanguageKind::Cpp,
        "json" | "jsonc" => LanguageKind::Json,
        "toml" => LanguageKind::Toml,
        "yaml" | "yml" => LanguageKind::Yaml,
        "md" | "markdown" => LanguageKind::Markdown,
        "sh" | "bash" | "zsh" => LanguageKind::Bash,
        "css" => LanguageKind::Css,
        "html" | "htm" => LanguageKind::Html,
        _ => return None,
    })
}

/// A compiled-in tree-sitter grammar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LanguageKind {
    Rust,
    TypeScript,
    Tsx,
    JavaScript,
    Python,
    Go,
    Java,
    Kotlin,
    C,
    Cpp,
    Json,
    Toml,
    Yaml,
    Markdown,
    Bash,
    Css,
    Html,
}

impl LanguageKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
            Self::JavaScript => "javascript",
            Self::Python => "python",
            Self::Go => "go",
            Self::Java => "java",
            Self::Kotlin => "kotlin",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::Json => "json",
            Self::Toml => "toml",
            Self::Yaml => "yaml",
            Self::Markdown => "markdown",
            Self::Bash => "bash",
            Self::Css => "css",
            Self::Html => "html",
        }
    }

    fn language(self) -> Language {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::Go => tree_sitter_go::LANGUAGE.into(),
            Self::Java => tree_sitter_java::LANGUAGE.into(),
            Self::Kotlin => tree_sitter_kotlin_sg::LANGUAGE.into(),
            Self::C => tree_sitter_c::LANGUAGE.into(),
            Self::Cpp => tree_sitter_cpp::LANGUAGE.into(),
            Self::Json => tree_sitter_json::LANGUAGE.into(),
            Self::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
            Self::Yaml => tree_sitter_yaml::LANGUAGE.into(),
            Self::Markdown => tree_sitter_md::LANGUAGE.into(),
            Self::Bash => tree_sitter_bash::LANGUAGE.into(),
            Self::Css => tree_sitter_css::LANGUAGE.into(),
            Self::Html => tree_sitter_html::LANGUAGE.into(),
        }
    }

    /// The three queries a [`HighlightConfiguration`] needs.
    fn queries(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Rust => (
                tree_sitter_rust::HIGHLIGHTS_QUERY,
                tree_sitter_rust::INJECTIONS_QUERY,
                "",
            ),
            Self::TypeScript | Self::Tsx => (
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
                "",
                tree_sitter_typescript::LOCALS_QUERY,
            ),
            Self::JavaScript => (
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::INJECTIONS_QUERY,
                tree_sitter_javascript::LOCALS_QUERY,
            ),
            Self::Python => (tree_sitter_python::HIGHLIGHTS_QUERY, "", ""),
            Self::Go => (tree_sitter_go::HIGHLIGHTS_QUERY, "", ""),
            Self::Java => (tree_sitter_java::HIGHLIGHTS_QUERY, "", ""),
            Self::Kotlin => (tree_sitter_kotlin_sg::HIGHLIGHTS_QUERY, "", ""),
            Self::C => (tree_sitter_c::HIGHLIGHT_QUERY, "", ""),
            Self::Cpp => (tree_sitter_cpp::HIGHLIGHT_QUERY, "", ""),
            Self::Json => (tree_sitter_json::HIGHLIGHTS_QUERY, "", ""),
            Self::Toml => (tree_sitter_toml_ng::HIGHLIGHTS_QUERY, "", ""),
            Self::Yaml => (tree_sitter_yaml::HIGHLIGHTS_QUERY, "", ""),
            Self::Markdown => (tree_sitter_md::HIGHLIGHT_QUERY_BLOCK, "", ""),
            Self::Bash => (tree_sitter_bash::HIGHLIGHT_QUERY, "", ""),
            Self::Css => (tree_sitter_css::HIGHLIGHTS_QUERY, "", ""),
            Self::Html => (
                tree_sitter_html::HIGHLIGHTS_QUERY,
                tree_sitter_html::INJECTIONS_QUERY,
                "",
            ),
        }
    }

    fn config(self) -> Option<HighlightConfiguration> {
        let (highlights, injections, locals) = self.queries();
        let mut config = HighlightConfiguration::new(
            self.language(),
            self.name(),
            highlights,
            injections,
            locals,
        )
        .ok()?;
        config.configure(&RECOGNIZED_NAMES);
        Some(config)
    }
}

/// Parses and highlights one text, returning spans grouped per line.
fn highlight_text(
    config: &mut HighlightConfiguration,
    text: &str,
) -> HashMap<u32, Vec<HighlightSpan>> {
    let mut highlighter = Highlighter::new();
    let events = match highlighter.highlight(config, text.as_bytes(), None, |_| None) {
        Ok(events) => events,
        Err(error) => {
            log::warn!("syntax highlighting failed: {error}");
            return HashMap::new();
        }
    };
    let mut spans = Vec::new();
    let mut current: Option<Token> = None;
    for event in events {
        match event {
            Ok(HighlightEvent::HighlightStart(highlight)) => {
                current = Token::from_index(highlight.0);
            }
            Ok(HighlightEvent::HighlightEnd) => current = None,
            Ok(HighlightEvent::Source { start, end }) => {
                if let Some(token) = current {
                    if start < end && end <= text.len() {
                        let token = token.for_text(&text[start..end]);
                        spans.push(HighlightSpan {
                            range: start..end,
                            token,
                        });
                    }
                }
            }
            Err(error) => {
                log::warn!("syntax highlighting stopped: {error}");
                return HashMap::new();
            }
        }
    }
    spans_by_line(text, &spans)
}

/// Splits byte spans that may cross lines into per-line spans.
fn spans_by_line(text: &str, spans: &[HighlightSpan]) -> HashMap<u32, Vec<HighlightSpan>> {
    let bytes = text.as_bytes();
    let mut out: HashMap<u32, Vec<HighlightSpan>> = HashMap::new();
    let mut line = 1u32;
    let mut line_start = 0usize;
    let mut scanned = 0usize;
    for span in spans {
        let start_limit = span.range.start.min(bytes.len());
        let end_limit = span.range.end.min(bytes.len());
        if start_limit >= end_limit {
            continue;
        }
        // Count the newlines between the previous span and this one.
        while scanned < start_limit {
            if bytes[scanned] == b'\n' {
                line += 1;
                line_start = scanned + 1;
            }
            scanned += 1;
        }
        let mut start = start_limit;
        while start < end_limit {
            let newline = text[start..end_limit]
                .find('\n')
                .map(|offset| start + offset);
            let end = newline.unwrap_or(end_limit);
            if end > start && text.is_char_boundary(start) && text.is_char_boundary(end) {
                // Diff rows have their tabs expanded, so the line-local byte
                // offsets must be too, or the colours drift after a tab.
                let local = &text[line_start..end];
                let expanded_start = expanded_offset(local, start - line_start);
                let expanded_end = expanded_offset(local, end - line_start);
                out.entry(line).or_default().push(HighlightSpan {
                    range: expanded_start..expanded_end,
                    token: span.token,
                });
            }
            match newline {
                Some(newline) => {
                    line += 1;
                    line_start = newline + 1;
                    scanned = newline + 1;
                    start = newline + 1;
                }
                None => {
                    scanned = end_limit;
                    break;
                }
            }
        }
    }
    out
}

/// The byte offset in a tab-expanded line for `offset` bytes of the raw line.
///
/// Mirrors [`crate::diff_model::expand_tabs`]: a tab advances to the next
/// multiple of four columns.
fn expanded_offset(raw: &str, offset: usize) -> usize {
    let mut column = 0;
    let mut out = 0;
    for (index, ch) in raw.char_indices() {
        if index >= offset {
            break;
        }
        if ch == '\t' {
            let spaces = 4 - column % 4;
            out += spaces;
            column += spaces;
        } else {
            out += ch.len_utf8();
            column += 1;
        }
    }
    out
}

/// One styled stretch of a line: its syntax token, if any, and whether the
/// word diff marks it as changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub range: Range<usize>,
    pub token: Option<Token>,
    /// Normalized again by [`Self::range`].
    pub changed: bool,
}

/// Splits `text` at every syntax or word-diff boundary, merging neighbours
/// with the same style.
pub fn line_segments(
    text: &str,
    spans: &[HighlightSpan],
    emphasis: &[Range<usize>],
) -> Vec<Segment> {
    let mut cuts = vec![0, text.len()];
    for range in spans.iter().map(|span| &span.range).chain(emphasis) {
        let start = range.start.min(text.len());
        let end = range.end.min(text.len());
        if text.is_char_boundary(start) && text.is_char_boundary(end) {
            cuts.push(start);
            cuts.push(end);
        }
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut segments: Vec<Segment> = Vec::new();
    for pair in cuts.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        if start >= end {
            continue;
        }
        let token = spans
            .iter()
            .find(|span| span.range.start <= start && start < span.range.end)
            .map(|span| span.token);
        let changed = emphasis
            .iter()
            .any(|range| range.start <= start && start < range.end);
        match segments.last_mut() {
            Some(last) if last.token == token && last.changed == changed => {
                last.range.end = end;
            }
            _ => segments.push(Segment {
                range: start..end,
                token,
                changed,
            }),
        }
    }
    segments
}

/// A parser for the language, for the API-shape test below.
#[cfg(test)]
fn parse(language: LanguageKind, text: &str) -> tree_sitter::Tree {
    let mut parser = Parser::new();
    parser
        .set_language(&language.language())
        .expect("the grammar loads");
    parser.parse(text, None).expect("the text parses")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_file_extensions_to_grammars() {
        assert_eq!(language_for("src/main.rs"), Some(LanguageKind::Rust));
        assert_eq!(language_for("a/b/app.tsx"), Some(LanguageKind::Tsx));
        assert_eq!(language_for("index.js"), Some(LanguageKind::JavaScript));
        assert_eq!(language_for("Cargo.toml"), Some(LanguageKind::Toml));
        assert_eq!(language_for("README.md"), Some(LanguageKind::Markdown));
        assert_eq!(language_for("script"), None);
        assert_eq!(language_for("image.png"), None);
        assert_eq!(language_for(".bashrc"), Some(LanguageKind::Bash));
    }

    #[test]
    fn syntax_spans_stay_inside_their_lines() {
        let text = "let x = 1;\n// a comment\nfn main() {}\n";
        let spans = vec![
            HighlightSpan {
                range: 0..3,
                token: Token::Keyword,
            },
            HighlightSpan {
                range: 11..23,
                token: Token::Comment,
            },
            HighlightSpan {
                range: 27..31,
                token: Token::Function,
            },
        ];
        let lines = spans_by_line(text, &spans);
        assert_eq!(lines[&1][0].range, 0..3);
        assert_eq!(lines[&2][0].range, 0..12);
        assert_eq!(lines[&3][0].range, 3..7);
        assert!(!lines.contains_key(&4));
    }

    #[test]
    fn a_span_across_lines_is_split_at_each_newline() {
        let text = "/* one\ntwo */";
        let spans = vec![HighlightSpan {
            range: 0..14,
            token: Token::Comment,
        }];
        let lines = spans_by_line(text, &spans);
        assert_eq!(lines[&1][0].range, 0..6);
        assert_eq!(lines[&2][0].range, 0..6);
    }

    #[test]
    fn tab_expansion_moves_span_offsets_to_the_expanded_line() {
        let text = "\tlet x = 1;\n";
        let spans = vec![
            HighlightSpan {
                range: 1..4,
                token: Token::Keyword,
            },
            HighlightSpan {
                range: 9..10,
                token: Token::Number,
            },
        ];
        let lines = spans_by_line(text, &spans);
        assert_eq!(lines[&1][0].range, 4..7, "`let` starts after four spaces");
        assert_eq!(lines[&1][1].range, 12..13, "`1` moved by the tab");
        assert_eq!(&crate::diff_model::expand_tabs("\tlet x = 1;")[12..13], "1");
    }

    #[test]
    fn line_segments_merge_neighbours_and_mark_changed_stretches() {
        let text = "let value = old + 1;";
        let spans = vec![
            HighlightSpan {
                range: 0..3,
                token: Token::Keyword,
            },
            HighlightSpan {
                range: 12..15,
                token: Token::Function,
            },
        ];
        let emphasis: std::vec::Vec<std::ops::Range<usize>> = std::iter::once(12..15).collect();
        let segments = line_segments(text, &spans, &emphasis);
        assert_eq!(segments[0].range, 0..3);
        assert_eq!(segments[0].token, Some(Token::Keyword));
        assert!(!segments[0].changed);
        let changed = segments
            .iter()
            .find(|segment| segment.changed)
            .expect("the changed word is marked");
        assert_eq!(changed.range, 12..15);
        assert_eq!(changed.token, Some(Token::Function));
        // Neighbours with the same style are one segment.
        assert!(segments.iter().filter(|segment| !segment.changed).count() >= 2);
        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.range.len())
                .sum::<usize>(),
            text.len()
        );
    }

    #[test]
    fn a_builtin_constant_word_is_a_keyword_not_a_number() {
        assert_eq!(Token::Number.for_text("42"), Token::Number);
        assert_eq!(Token::Number.for_text("3.14"), Token::Number);
        assert_eq!(Token::Number.for_text("true"), Token::Keyword);
        assert_eq!(Token::Number.for_text("None"), Token::Keyword);
        assert_eq!(Token::String.for_text("hello"), Token::String);
    }

    #[test]
    fn an_unknown_language_stays_plain() {
        let texts = FileTexts {
            old: None,
            new: Some("some text\n".to_string()),
            binary: false,
        };
        let highlighted = highlight("notes.unknown", &texts);
        assert_eq!(highlighted.language, None);
        assert!(highlighted.new.is_empty());
    }

    #[test]
    fn a_real_rust_file_gets_keyword_string_number_and_function_spans() {
        let source = "fn main() {\n    let text = \"hello\";\n    let n = 42;\n}\n";
        let Some(mut config) = LanguageKind::Rust.config() else {
            panic!("the Rust configuration builds");
        };
        let lines = highlight_text(&mut config, source);
        let tokens: Vec<Token> = lines
            .values()
            .flat_map(|spans| spans.iter().map(|span| span.token))
            .collect();
        assert!(tokens.contains(&Token::Keyword), "{tokens:?}");
        assert!(tokens.contains(&Token::Function), "{tokens:?}");
        assert!(tokens.contains(&Token::String), "{tokens:?}");
        assert!(tokens.contains(&Token::Number), "{tokens:?}");
        let line_2 = &lines[&2];
        assert!(line_2.iter().any(|span| span.token == Token::String
            && &source.lines().nth(1).unwrap()[span.range.clone()] == "\"hello\""));
    }

    #[test]
    fn every_compiled_grammar_builds_a_configuration() {
        for language in [
            LanguageKind::Rust,
            LanguageKind::TypeScript,
            LanguageKind::Tsx,
            LanguageKind::JavaScript,
            LanguageKind::Python,
            LanguageKind::Go,
            LanguageKind::Java,
            LanguageKind::Kotlin,
            LanguageKind::C,
            LanguageKind::Cpp,
            LanguageKind::Json,
            LanguageKind::Toml,
            LanguageKind::Yaml,
            LanguageKind::Markdown,
            LanguageKind::Bash,
            LanguageKind::Css,
            LanguageKind::Html,
        ] {
            assert!(
                language.config().is_some(),
                "{} has no usable configuration",
                language.name()
            );
        }
    }

    #[test]
    fn the_parser_api_matches_the_grammar_crates() {
        let tree = parse(LanguageKind::Rust, "fn main() {}");
        assert!(!tree.root_node().has_error());
    }
}
