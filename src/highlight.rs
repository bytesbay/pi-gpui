//! Syntax highlighting for code blocks in answers: syntect with bat's syntax
//! set (two-face), pure Rust regexes. Colors come from the theme's
//! `SyntaxColors`, mapped from TextMate scopes the way editor themes do.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::str::FromStr;
use std::sync::OnceLock;

use gpui::{Hsla, Rgba};
use syntect::easy::HighlightLines;
use syntect::highlighting::{
    Color as SyntectColor, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings,
};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use crate::ui::SyntaxColors;

/// Larger blocks are shown plain; highlighting them would cost frames.
const MAX_HIGHLIGHTED_BYTES: usize = 100_000;
const CACHE_LIMIT: usize = 256;

pub type Highlights = Rc<Vec<(Range<usize>, Hsla)>>;

fn syntaxes() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(two_face::syntax::extra_newlines)
}

/// Loads the syntax set on a background thread, so the first code block
/// does not wait for it.
pub fn preload() {
    std::thread::spawn(|| {
        syntaxes();
    });
}

thread_local! {
    static CACHE: RefCell<HashMap<u64, Highlights>> = RefCell::new(HashMap::new());
}

/// Colored ranges for `code`, or `None` for unknown languages. Results are
/// cached by content, so a block re-rendered every frame is highlighted once
/// (and once per change while it streams).
pub fn highlight(code: &str, language: Option<&str>, colors: &SyntaxColors) -> Option<Highlights> {
    let language = language?;
    if code.len() > MAX_HIGHLIGHTED_BYTES {
        return None;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (code, language, colors.keyword.to_string()).hash(&mut hasher);
    let key = hasher.finish();
    if let Some(cached) = CACHE.with(|cache| cache.borrow().get(&key).cloned()) {
        return Some(cached);
    }
    let highlights = Rc::new(compute(code, language, colors)?);
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() >= CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(key, highlights.clone());
    });
    Some(highlights)
}

fn compute(code: &str, language: &str, colors: &SyntaxColors) -> Option<Vec<(Range<usize>, Hsla)>> {
    let set = syntaxes();
    let syntax = set
        .find_syntax_by_token(language)
        .or_else(|| set.find_syntax_by_extension(language))?;
    let theme = theme(colors);
    let mut highlighter = HighlightLines::new(syntax, &theme);
    let mut highlights = Vec::new();
    let mut offset = 0;
    for line in LinesWithEndings::from(code) {
        for (style, text) in highlighter.highlight_line(line, set).ok()? {
            let range = offset..offset + text.len();
            offset = range.end;
            if style.foreground != DEFAULT && !text.trim().is_empty() {
                highlights.push((range, to_hsla(style.foreground)));
            }
        }
    }
    Some(highlights)
}

/// Marks "no rule matched": such text keeps the block's text color.
const DEFAULT: SyntectColor = SyntectColor {
    r: 1,
    g: 2,
    b: 3,
    a: 255,
};

/// A syntect theme from the app's syntax colors, with the scope groups most
/// editor themes use.
fn theme(colors: &SyntaxColors) -> Theme {
    let rules: [(&str, Option<Hsla>); 13] = [
        (
            "comment, punctuation.definition.comment",
            Some(colors.comment),
        ),
        (
            "string, punctuation.definition.string, string.quoted",
            Some(colors.string),
        ),
        ("constant.numeric", Some(colors.number)),
        (
            "constant.language, constant.character, constant.other, support.constant",
            Some(colors.constant),
        ),
        (
            "keyword, storage.type, storage.modifier, keyword.control, variable.language",
            Some(colors.keyword),
        ),
        ("keyword.operator", Some(colors.operator)),
        (
            "entity.name.function, support.function, variable.function, meta.function-call.identifier",
            Some(colors.function),
        ),
        (
            "entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, entity.name.trait, support.type, support.class, entity.other.inherited-class",
            Some(colors.r#type),
        ),
        (
            "variable.other.member, variable.other.property, meta.property-name, support.type.property-name, meta.object-literal.key",
            Some(colors.property),
        ),
        ("entity.name.tag", Some(colors.tag)),
        (
            "entity.other.attribute-name, meta.attribute, meta.annotation",
            Some(colors.attribute),
        ),
        ("punctuation", colors.punctuation),
        ("markup.heading, entity.name.section", Some(colors.function)),
    ];
    let scopes = rules
        .into_iter()
        .filter_map(|(selector, color)| {
            Some(ThemeItem {
                scope: ScopeSelectors::from_str(selector).ok()?,
                style: StyleModifier {
                    foreground: Some(to_syntect(color?)),
                    background: None,
                    font_style: None,
                },
            })
        })
        .collect();
    Theme {
        name: None,
        author: None,
        settings: ThemeSettings {
            foreground: Some(DEFAULT),
            ..ThemeSettings::default()
        },
        scopes,
    }
}

fn to_syntect(color: Hsla) -> SyntectColor {
    let rgba = Rgba::from(color);
    let channel = |value: f32| (value.clamp(0., 1.) * 255.).round() as u8;
    SyntectColor {
        r: channel(rgba.r),
        g: channel(rgba.g),
        b: channel(rgba.b),
        a: channel(rgba.a),
    }
}

fn to_hsla(color: SyntectColor) -> Hsla {
    Rgba {
        r: f32::from(color.r) / 255.,
        g: f32::from(color.g) / 255.,
        b: f32::from(color.b) / 255.,
        a: f32::from(color.a) / 255.,
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_keywords_strings_and_comments_get_colors() {
        let colors = SyntaxColors::github_dark();
        let code = "// note\nfn main() { let s = \"hi\"; }\n";
        let highlights = highlight(code, Some("rust"), &colors).expect("rust is known");
        let colored = |needle: &str| {
            let start = code.find(needle).unwrap();
            highlights
                .iter()
                .find(|(range, _)| range.start <= start && start < range.end)
                .map(|(_, color)| *color)
        };
        assert_eq!(colored("note"), Some(colors.comment));
        assert_eq!(colored("fn"), Some(colors.keyword));
        assert_eq!(colored("hi"), Some(colors.string));
        assert_eq!(colored("main"), Some(colors.function));
        for language in ["ts", "tsx", "toml", "swift", "sh", "py", "json", "diff"] {
            assert!(
                highlight("x = 1\n", Some(language), &colors).is_some(),
                "{language} should be known"
            );
        }
        assert!(highlight(code, Some("no-such-language"), &colors).is_none());
        assert!(highlight(code, None, &colors).is_none());
    }
}
